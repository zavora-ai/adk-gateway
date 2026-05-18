//! Stale context detection — detects idle periods and builds welcome-back messages.
//!
//! On each incoming message, the detector checks the user's `last_activity` timestamp.
//! If the idle period exceeds the configured threshold (default: 4 hours), it builds
//! a welcome-back message summarizing pending tasks, heartbeat alerts, and time since
//! last interaction. If no pending items exist, a brief acknowledgment is sent instead.

use chrono::{DateTime, Duration, Utc};

use crate::config::StaleContextConfig;

// ── Data Types ─────────────────────────────────────────────────────

/// A pending task result that accumulated during the user's idle period.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingTaskResult {
    /// Human-readable description of the pending task.
    pub description: String,
}

/// A heartbeat alert generated during the user's idle period.
#[derive(Debug, Clone, PartialEq)]
pub struct HeartbeatAlert {
    /// Human-readable alert message.
    pub message: String,
}

// ── Stale Context Detector ─────────────────────────────────────────

/// Detects idle periods and builds context-refreshing welcome-back messages.
///
/// Integrated into the message processing pipeline: on each incoming message,
/// the detector checks whether the user has been idle beyond the configured
/// threshold and, if so, produces a summary of what happened while they were away.
#[derive(Debug, Clone)]
pub struct StaleContextDetector {
    config: StaleContextConfig,
}

impl StaleContextDetector {
    /// Create a new detector with the given configuration.
    pub fn new(config: StaleContextConfig) -> Self {
        Self { config }
    }

    /// Create a new detector with default configuration (4-hour threshold).
    pub fn with_defaults() -> Self {
        Self::new(StaleContextConfig::default())
    }

    /// Check if the user's session is stale based on last activity.
    ///
    /// Returns `true` if and only if `(now - last_activity) > threshold`.
    /// The comparison is strictly greater-than: exactly at the threshold is NOT stale.
    pub fn is_stale(&self, last_activity: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        let threshold = Duration::seconds(self.config.idle_threshold_secs as i64);
        let idle_duration = now.signed_duration_since(last_activity);
        idle_duration > threshold
    }

    /// Compute the idle duration between last activity and now.
    pub fn idle_duration(&self, last_activity: DateTime<Utc>, now: DateTime<Utc>) -> Duration {
        now.signed_duration_since(last_activity)
    }

    /// Build the welcome-back message content.
    ///
    /// When pending tasks or heartbeat alerts exist, the message includes:
    /// - Time since last interaction (human-readable)
    /// - Count of pending task results
    /// - Heartbeat alerts generated during the idle period
    ///
    /// When no pending items or alerts exist, returns a brief acknowledgment.
    pub fn build_welcome_back(
        &self,
        idle_duration: Duration,
        pending_tasks: &[PendingTaskResult],
        heartbeat_alerts: &[HeartbeatAlert],
    ) -> String {
        let has_pending = !pending_tasks.is_empty() || !heartbeat_alerts.is_empty();

        if !has_pending {
            return format!(
                "👋 Welcome back! You've been away for {}. Nothing pending — ready when you are.",
                format_duration(idle_duration)
            );
        }

        let mut parts = Vec::new();
        parts.push(format!(
            "👋 **Welcome back!** You've been away for {}.",
            format_duration(idle_duration)
        ));
        parts.push(String::new()); // blank line

        parts.push("Here's what happened while you were away:".to_string());

        if !pending_tasks.is_empty() {
            parts.push(format!("📋 **{} pending task(s)**", pending_tasks.len()));
            for task in pending_tasks {
                parts.push(format!("  • {}", task.description));
            }
        }

        if !heartbeat_alerts.is_empty() {
            parts.push(format!("🔔 **{} heartbeat alert(s)**", heartbeat_alerts.len()));
            for alert in heartbeat_alerts {
                parts.push(format!("  • {}", alert.message));
            }
        }

        parts.join("\n")
    }

    /// Get a reference to the current configuration.
    pub fn config(&self) -> &StaleContextConfig {
        &self.config
    }

    /// Get the idle threshold in seconds.
    pub fn idle_threshold_secs(&self) -> u64 {
        self.config.idle_threshold_secs
    }
}

// ── Helpers ────────────────────────────────────────────────────────

/// Format a chrono Duration into a human-readable string.
fn format_duration(d: Duration) -> String {
    let total_secs = d.num_seconds().max(0);
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;

    if hours > 0 && minutes > 0 {
        format!("{}h {}m", hours, minutes)
    } else if hours > 0 {
        format!("{}h", hours)
    } else if minutes > 0 {
        format!("{}m", minutes)
    } else {
        format!("{}s", total_secs)
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_detector() -> StaleContextDetector {
        StaleContextDetector::with_defaults()
    }

    fn detector_with_threshold(secs: u64) -> StaleContextDetector {
        StaleContextDetector::new(StaleContextConfig {
            idle_threshold_secs: secs,
        })
    }

    #[test]
    fn test_default_threshold_is_4_hours() {
        let detector = default_detector();
        assert_eq!(detector.idle_threshold_secs(), 14400);
    }

    #[test]
    fn test_custom_threshold() {
        let detector = detector_with_threshold(3600);
        assert_eq!(detector.idle_threshold_secs(), 3600);
    }

    #[test]
    fn test_is_stale_when_idle_exceeds_threshold() {
        let detector = detector_with_threshold(3600); // 1 hour
        let last_activity = Utc::now() - Duration::seconds(7200); // 2 hours ago
        let now = Utc::now();

        assert!(detector.is_stale(last_activity, now));
    }

    #[test]
    fn test_is_not_stale_when_idle_below_threshold() {
        let detector = detector_with_threshold(3600); // 1 hour
        let last_activity = Utc::now() - Duration::seconds(1800); // 30 min ago
        let now = Utc::now();

        assert!(!detector.is_stale(last_activity, now));
    }

    #[test]
    fn test_is_not_stale_at_exact_threshold() {
        // Exactly at threshold should NOT be stale (strictly greater-than)
        let detector = detector_with_threshold(3600);
        let now = Utc::now();
        let last_activity = now - Duration::seconds(3600);

        assert!(!detector.is_stale(last_activity, now));
    }

    #[test]
    fn test_is_stale_one_second_past_threshold() {
        let detector = detector_with_threshold(3600);
        let now = Utc::now();
        let last_activity = now - Duration::seconds(3601);

        assert!(detector.is_stale(last_activity, now));
    }

    #[test]
    fn test_build_welcome_back_with_pending_items() {
        let detector = default_detector();
        let idle = Duration::seconds(18000); // 5 hours

        let tasks = vec![
            PendingTaskResult {
                description: "Deploy v2.1 to staging".to_string(),
            },
            PendingTaskResult {
                description: "Review PR #42".to_string(),
            },
        ];
        let alerts = vec![HeartbeatAlert {
            message: "CI pipeline failed on main".to_string(),
        }];

        let msg = detector.build_welcome_back(idle, &tasks, &alerts);

        assert!(msg.contains("5h"));
        assert!(msg.contains("2 pending task(s)"));
        assert!(msg.contains("Deploy v2.1 to staging"));
        assert!(msg.contains("Review PR #42"));
        assert!(msg.contains("1 heartbeat alert(s)"));
        assert!(msg.contains("CI pipeline failed on main"));
    }

    #[test]
    fn test_build_welcome_back_no_pending_items() {
        let detector = default_detector();
        let idle = Duration::seconds(18000); // 5 hours

        let msg = detector.build_welcome_back(idle, &[], &[]);

        assert!(msg.contains("Welcome back"));
        assert!(msg.contains("5h"));
        assert!(msg.contains("Nothing pending"));
    }

    #[test]
    fn test_build_welcome_back_only_tasks_no_alerts() {
        let detector = default_detector();
        let idle = Duration::seconds(7200); // 2 hours

        let tasks = vec![PendingTaskResult {
            description: "Run tests".to_string(),
        }];

        let msg = detector.build_welcome_back(idle, &tasks, &[]);

        assert!(msg.contains("1 pending task(s)"));
        assert!(!msg.contains("heartbeat alert"));
    }

    #[test]
    fn test_build_welcome_back_only_alerts_no_tasks() {
        let detector = default_detector();
        let idle = Duration::seconds(7200);

        let alerts = vec![HeartbeatAlert {
            message: "Disk usage at 90%".to_string(),
        }];

        let msg = detector.build_welcome_back(idle, &[], &alerts);

        assert!(!msg.contains("pending task"));
        assert!(msg.contains("1 heartbeat alert(s)"));
        assert!(msg.contains("Disk usage at 90%"));
    }

    #[test]
    fn test_format_duration_hours_and_minutes() {
        assert_eq!(format_duration(Duration::seconds(5400)), "1h 30m");
    }

    #[test]
    fn test_format_duration_hours_only() {
        assert_eq!(format_duration(Duration::seconds(7200)), "2h");
    }

    #[test]
    fn test_format_duration_minutes_only() {
        assert_eq!(format_duration(Duration::seconds(900)), "15m");
    }

    #[test]
    fn test_format_duration_seconds_only() {
        assert_eq!(format_duration(Duration::seconds(45)), "45s");
    }

    #[test]
    fn test_is_stale_future_last_activity() {
        // If last_activity is in the future (clock skew), should not be stale
        let detector = detector_with_threshold(3600);
        let now = Utc::now();
        let last_activity = now + Duration::seconds(100);

        assert!(!detector.is_stale(last_activity, now));
    }

    #[test]
    fn test_idle_duration_calculation() {
        let detector = default_detector();
        let now = Utc::now();
        let last_activity = now - Duration::seconds(5000);

        let idle = detector.idle_duration(last_activity, now);
        assert_eq!(idle.num_seconds(), 5000);
    }
}
