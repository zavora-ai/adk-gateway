//! Prometheus-compatible gateway metrics.
//!
//! Design: GatewayMetrics [R14.2, R14.4, R14.5]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::{Duration, Instant};

// ── Metric labels ──────────────────────────────────────────────────

/// Labels for a recorded message metric.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct MessageLabels {
    pub channel: String,
    pub status: MessageStatus,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum MessageStatus {
    Success,
    Failure,
}

impl std::fmt::Display for MessageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageStatus::Success => write!(f, "success"),
            MessageStatus::Failure => write!(f, "failure"),
        }
    }
}

/// Labels for token metrics.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct TokenLabels {
    pub model: String,
    pub direction: TokenDirection,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum TokenDirection {
    Input,
    Output,
}

impl std::fmt::Display for TokenDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenDirection::Input => write!(f, "input"),
            TokenDirection::Output => write!(f, "output"),
        }
    }
}

// ── Sliding window ─────────────────────────────────────────────────

/// A 5-minute sliding window for error rate detection.
#[derive(Debug)]
pub struct SlidingWindow {
    window_duration: Duration,
    entries: RwLock<Vec<WindowEntry>>,
}

#[derive(Debug, Clone)]
struct WindowEntry {
    timestamp: Instant,
    is_error: bool,
}

impl SlidingWindow {
    pub fn new(window_duration: Duration) -> Self {
        Self {
            window_duration,
            entries: RwLock::new(Vec::new()),
        }
    }

    pub fn record(&self, is_error: bool) {
        if let Ok(mut entries) = self.entries.write() {
            entries.push(WindowEntry {
                timestamp: Instant::now(),
                is_error,
            });
        }
    }

    /// Prune entries older than the window and return (total, errors).
    pub fn error_rate(&self) -> (usize, usize) {
        let now = Instant::now();
        let cutoff = now - self.window_duration;

        if let Ok(mut entries) = self.entries.write() {
            entries.retain(|e| e.timestamp >= cutoff);
            let total = entries.len();
            let errors = entries.iter().filter(|e| e.is_error).count();
            (total, errors)
        } else {
            (0, 0)
        }
    }

    /// Returns the error rate as a fraction [0.0, 1.0].
    pub fn error_rate_fraction(&self) -> f64 {
        let (total, errors) = self.error_rate();
        if total == 0 {
            0.0
        } else {
            errors as f64 / total as f64
        }
    }
}

// ── Histogram ──────────────────────────────────────────────────────

/// Simple histogram with configurable buckets for latency tracking.
#[derive(Debug)]
pub struct Histogram {
    buckets: Vec<f64>,
    counts: Vec<AtomicU64>,
    sum: AtomicU64, // stored as micros
    count: AtomicU64,
}

impl Histogram {
    pub fn new(buckets: Vec<f64>) -> Self {
        let counts = (0..buckets.len() + 1).map(|_| AtomicU64::new(0)).collect();
        Self {
            buckets,
            counts,
            sum: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    pub fn observe(&self, value: f64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.sum
            .fetch_add((value * 1_000_000.0) as u64, Ordering::Relaxed);

        for (i, &bucket) in self.buckets.iter().enumerate() {
            if value <= bucket {
                self.counts[i].fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        // +Inf bucket (always present since counts.len() == buckets.len() + 1)
        if let Some(inf_bucket) = self.counts.last() {
            inf_bucket.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[allow(dead_code)] // Used in tests; available for histogram statistics
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    #[allow(dead_code)] // Used in tests; available for histogram statistics
    pub fn sum_secs(&self) -> f64 {
        self.sum.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }
}

// ── GatewayMetrics ─────────────────────────────────────────────────

/// Central metrics collector for the gateway.
#[derive(Debug)]
pub struct GatewayMetrics {
    messages_total: RwLock<HashMap<MessageLabels, u64>>,
    message_latency: RwLock<HashMap<String, Histogram>>,
    tokens_total: RwLock<HashMap<TokenLabels, u64>>,
    active_sessions: AtomicU64,
    channel_status: RwLock<HashMap<String, i64>>,
    errors_total: RwLock<HashMap<String, u64>>,
    /// Per-channel sliding windows for error rate detection.
    error_windows: RwLock<HashMap<String, SlidingWindow>>,
}

impl GatewayMetrics {
    pub fn new() -> Self {
        Self {
            messages_total: RwLock::new(HashMap::new()),
            message_latency: RwLock::new(HashMap::new()),
            tokens_total: RwLock::new(HashMap::new()),
            active_sessions: AtomicU64::new(0),
            channel_status: RwLock::new(HashMap::new()),
            errors_total: RwLock::new(HashMap::new()),
            error_windows: RwLock::new(HashMap::new()),
        }
    }

    /// Record a processed message with its latency and status.
    pub fn record_message(
        &self,
        channel: &str,
        status: MessageStatus,
        latency: Duration,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        model: Option<&str>,
    ) {
        let is_error = matches!(status, MessageStatus::Failure);

        // messages_total
        {
            let labels = MessageLabels {
                channel: channel.to_string(),
                status,
            };
            if let Ok(mut map) = self.messages_total.write() {
                *map.entry(labels).or_insert(0) += 1;
            }
        }

        // message_latency histogram
        {
            if let Ok(mut map) = self.message_latency.write() {
                let hist = map.entry(channel.to_string()).or_insert_with(|| {
                    Histogram::new(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0])
                });
                hist.observe(latency.as_secs_f64());
            }
        }

        // tokens_total
        if let Some(model_name) = model {
            if let Ok(mut map) = self.tokens_total.write() {
                if let Some(input) = input_tokens {
                    let labels = TokenLabels {
                        model: model_name.to_string(),
                        direction: TokenDirection::Input,
                    };
                    *map.entry(labels).or_insert(0) += input;
                }
                if let Some(output) = output_tokens {
                    let labels = TokenLabels {
                        model: model_name.to_string(),
                        direction: TokenDirection::Output,
                    };
                    *map.entry(labels).or_insert(0) += output;
                }
            }
        }

        // errors_total
        if is_error {
            if let Ok(mut map) = self.errors_total.write() {
                *map.entry(channel.to_string()).or_insert(0) += 1;
            }
        }

        // Sliding window for error rate
        {
            if let Ok(mut windows) = self.error_windows.write() {
                let window = windows
                    .entry(channel.to_string())
                    .or_insert_with(|| SlidingWindow::new(Duration::from_secs(300)));
                window.record(is_error);
            }
        }

        // Check sustained error rate > 50% (R14.5)
        self.check_error_rate(channel);
    }

    fn check_error_rate(&self, channel: &str) {
        if let Ok(windows) = self.error_windows.read() {
            if let Some(window) = windows.get(channel) {
                let rate = window.error_rate_fraction();
                if rate > 0.5 {
                    let (total, errors) = window.error_rate();
                    tracing::warn!(
                        channel = channel,
                        error_rate = format!("{:.1}%", rate * 100.0),
                        total_messages = total,
                        errors = errors,
                        "sustained error rate above 50% on channel"
                    );
                }
            }
        }
    }

    /// Set the active session count.
    pub fn set_active_sessions(&self, count: u64) {
        self.active_sessions.store(count, Ordering::Relaxed);
    }

    /// Set channel status (1 = connected, 0 = disconnected, -1 = failed).
    pub fn set_channel_status(&self, channel: &str, status: i64) {
        if let Ok(mut map) = self.channel_status.write() {
            map.insert(channel.to_string(), status);
        }
    }

    /// Get the total message count for a given channel and status.
    pub fn get_messages_total(&self, channel: &str, status: &MessageStatus) -> u64 {
        let labels = MessageLabels {
            channel: channel.to_string(),
            status: status.clone(),
        };
        self.messages_total
            .read()
            .map(|m| m.get(&labels).copied().unwrap_or(0))
            .unwrap_or(0)
    }

    /// Get the error rate fraction for a channel over the sliding window.
    pub fn get_error_rate(&self, channel: &str) -> f64 {
        self.error_windows
            .read()
            .ok()
            .and_then(|w| w.get(channel).map(|sw| sw.error_rate_fraction()))
            .unwrap_or(0.0)
    }

    /// Render Prometheus-compatible text output.
    pub fn render_prometheus(&self) -> String {
        let mut out = String::new();

        // messages_total
        out.push_str("# HELP adk_gateway_messages_total Total messages processed\n");
        out.push_str("# TYPE adk_gateway_messages_total counter\n");
        if let Ok(map) = self.messages_total.read() {
            for (labels, count) in map.iter() {
                out.push_str(&format!(
                    "adk_gateway_messages_total{{channel=\"{}\",status=\"{}\"}} {}\n",
                    labels.channel, labels.status, count
                ));
            }
        }

        // active_sessions
        out.push_str("# HELP adk_gateway_active_sessions Current active sessions\n");
        out.push_str("# TYPE adk_gateway_active_sessions gauge\n");
        out.push_str(&format!(
            "adk_gateway_active_sessions {}\n",
            self.active_sessions.load(Ordering::Relaxed)
        ));

        // errors_total
        out.push_str("# HELP adk_gateway_errors_total Total errors by channel\n");
        out.push_str("# TYPE adk_gateway_errors_total counter\n");
        if let Ok(map) = self.errors_total.read() {
            for (channel, count) in map.iter() {
                out.push_str(&format!(
                    "adk_gateway_errors_total{{channel=\"{}\"}} {}\n",
                    channel, count
                ));
            }
        }

        // tokens_total
        out.push_str("# HELP adk_gateway_tokens_total Total tokens by model and direction\n");
        out.push_str("# TYPE adk_gateway_tokens_total counter\n");
        if let Ok(map) = self.tokens_total.read() {
            for (labels, count) in map.iter() {
                out.push_str(&format!(
                    "adk_gateway_tokens_total{{model=\"{}\",direction=\"{}\"}} {}\n",
                    labels.model, labels.direction, count
                ));
            }
        }

        // channel_status gauge (1=connected, 0=disconnected, -1=failed)
        out.push_str("# HELP adk_gateway_channel_status Channel connection status\n");
        out.push_str("# TYPE adk_gateway_channel_status gauge\n");
        if let Ok(map) = self.channel_status.read() {
            for (channel, status) in map.iter() {
                out.push_str(&format!(
                    "adk_gateway_channel_status{{channel=\"{}\"}} {}\n",
                    channel, status
                ));
            }
        }

        out
    }
}

impl Default for GatewayMetrics {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_success_message() {
        let metrics = GatewayMetrics::new();
        metrics.record_message(
            "telegram",
            MessageStatus::Success,
            Duration::from_millis(150),
            Some(100),
            Some(50),
            Some("gpt-4"),
        );

        assert_eq!(
            metrics.get_messages_total("telegram", &MessageStatus::Success),
            1
        );
        assert_eq!(
            metrics.get_messages_total("telegram", &MessageStatus::Failure),
            0
        );
    }

    #[test]
    fn test_record_failure_message() {
        let metrics = GatewayMetrics::new();
        metrics.record_message(
            "slack",
            MessageStatus::Failure,
            Duration::from_millis(500),
            None,
            None,
            None,
        );

        assert_eq!(
            metrics.get_messages_total("slack", &MessageStatus::Failure),
            1
        );
    }

    #[test]
    fn test_sliding_window_error_rate() {
        let window = SlidingWindow::new(Duration::from_secs(300));
        window.record(false);
        window.record(false);
        window.record(true);

        let (total, errors) = window.error_rate();
        assert_eq!(total, 3);
        assert_eq!(errors, 1);

        let rate = window.error_rate_fraction();
        assert!((rate - 1.0 / 3.0).abs() < 0.01);
    }

    #[test]
    fn test_sliding_window_empty() {
        let window = SlidingWindow::new(Duration::from_secs(300));
        assert_eq!(window.error_rate_fraction(), 0.0);
    }

    #[test]
    fn test_histogram_observe() {
        let hist = Histogram::new(vec![0.1, 0.5, 1.0]);
        hist.observe(0.05);
        hist.observe(0.3);
        hist.observe(2.0);

        assert_eq!(hist.count(), 3);
        assert!(hist.sum_secs() > 0.0);
    }

    #[test]
    fn test_active_sessions() {
        let metrics = GatewayMetrics::new();
        metrics.set_active_sessions(42);
        assert_eq!(metrics.active_sessions.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn test_channel_status() {
        let metrics = GatewayMetrics::new();
        metrics.set_channel_status("telegram", 1);
        let map = metrics.channel_status.read().unwrap();
        assert_eq!(map.get("telegram"), Some(&1));
    }

    #[test]
    fn test_prometheus_output() {
        let metrics = GatewayMetrics::new();
        metrics.record_message(
            "telegram",
            MessageStatus::Success,
            Duration::from_millis(100),
            None,
            None,
            None,
        );
        let output = metrics.render_prometheus();
        assert!(output.contains("adk_gateway_messages_total"));
        assert!(output.contains("adk_gateway_active_sessions"));
    }

    #[test]
    fn test_error_rate_detection() {
        let metrics = GatewayMetrics::new();
        // Record mostly errors to trigger > 50% rate
        for _ in 0..10 {
            metrics.record_message(
                "test_ch",
                MessageStatus::Failure,
                Duration::from_millis(100),
                None,
                None,
                None,
            );
        }
        for _ in 0..3 {
            metrics.record_message(
                "test_ch",
                MessageStatus::Success,
                Duration::from_millis(100),
                None,
                None,
                None,
            );
        }
        let rate = metrics.get_error_rate("test_ch");
        assert!(rate > 0.5);
    }

    #[test]
    fn test_multiple_channels() {
        let metrics = GatewayMetrics::new();
        metrics.record_message(
            "telegram",
            MessageStatus::Success,
            Duration::from_millis(50),
            None,
            None,
            None,
        );
        metrics.record_message(
            "slack",
            MessageStatus::Success,
            Duration::from_millis(100),
            None,
            None,
            None,
        );
        metrics.record_message(
            "slack",
            MessageStatus::Failure,
            Duration::from_millis(200),
            None,
            None,
            None,
        );

        assert_eq!(
            metrics.get_messages_total("telegram", &MessageStatus::Success),
            1
        );
        assert_eq!(
            metrics.get_messages_total("slack", &MessageStatus::Success),
            1
        );
        assert_eq!(
            metrics.get_messages_total("slack", &MessageStatus::Failure),
            1
        );
    }
}
