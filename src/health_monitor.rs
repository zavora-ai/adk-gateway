//! Health Monitor — periodic health checks for gateway components.
//!
//! Implements a state machine that tracks per-component health status,
//! emits alerts after consecutive failures reach a threshold, and emits
//! recovery notifications when a failing component returns to healthy.
//!
//! Integrates with the existing `/health` endpoint to expose per-component
//! breakdown, and supports alerting via webhook POST or Telegram message.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::config::HealthMonitorConfig;

// ── Health Status ──────────────────────────────────────────────────

/// The health status of a monitored component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded { reason: String },
    Unhealthy { reason: String },
}

impl HealthStatus {
    /// Returns true if the status represents a healthy state.
    pub fn is_healthy(&self) -> bool {
        matches!(self, HealthStatus::Healthy)
    }
}

// ── Component Health ───────────────────────────────────────────────

/// Per-component health state tracked by the monitor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    pub name: String,
    pub status: HealthStatus,
    pub consecutive_failures: u32,
    pub last_check: DateTime<Utc>,
    /// Whether an alert has been emitted for the current failure streak.
    /// Used to prevent duplicate alerts.
    #[serde(skip)]
    pub alerted: bool,
}

// ── Health Events ──────────────────────────────────────────────────

/// Events emitted by the health monitor state machine.
#[derive(Debug, Clone, PartialEq)]
pub enum HealthEvent {
    /// Alert: component has reached the failure threshold.
    Alert { component: String, failures: u32 },
    /// Recovery: component transitioned from alerted (failed) state to healthy.
    Recovery { component: String },
}

// ── Health Monitor ─────────────────────────────────────────────────

/// The health monitor tracks per-component health and emits events
/// based on state transitions.
pub struct HealthMonitor {
    components: DashMap<String, ComponentHealth>,
    config: HealthMonitorConfig,
}

impl HealthMonitor {
    /// Create a new health monitor with the given configuration.
    pub fn new(config: HealthMonitorConfig) -> Self {
        Self {
            components: DashMap::new(),
            config,
        }
    }

    /// Record a health check result for a component.
    ///
    /// Returns `Some(HealthEvent)` if the state transition warrants an alert
    /// or recovery notification. Returns `None` if no event should be emitted.
    ///
    /// State machine rules:
    /// - Alert emitted when consecutive_failures reaches failure_threshold (exactly once)
    /// - Recovery emitted when a component transitions from alerted state to healthy
    /// - No duplicate alerts for the same failure streak
    /// - No duplicate recoveries for the same recovery
    pub fn record_check(&self, component: &str, healthy: bool) -> Option<HealthEvent> {
        let now = Utc::now();
        let threshold = self.config.failure_threshold;

        let mut entry = self.components.entry(component.to_string()).or_insert_with(|| {
            ComponentHealth {
                name: component.to_string(),
                status: HealthStatus::Healthy,
                consecutive_failures: 0,
                last_check: now,
                alerted: false,
            }
        });

        let health = entry.value_mut();
        health.last_check = now;

        if healthy {
            // Component is healthy
            let was_alerted = health.alerted;
            health.status = HealthStatus::Healthy;
            health.consecutive_failures = 0;
            health.alerted = false;

            if was_alerted {
                // Transition from alerted (failed) state to healthy → recovery
                Some(HealthEvent::Recovery {
                    component: component.to_string(),
                })
            } else {
                None
            }
        } else {
            // Component failed
            health.consecutive_failures += 1;
            let failures = health.consecutive_failures;

            if failures >= threshold {
                health.status = HealthStatus::Unhealthy {
                    reason: format!("{} consecutive failures", failures),
                };
            } else {
                health.status = HealthStatus::Degraded {
                    reason: format!("{}/{} failures", failures, threshold),
                };
            }

            // Emit alert exactly once when threshold is reached
            if failures >= threshold && !health.alerted {
                health.alerted = true;
                Some(HealthEvent::Alert {
                    component: component.to_string(),
                    failures,
                })
            } else {
                None
            }
        }
    }

    /// Record a health check with a specific reason for failure.
    pub fn record_check_with_reason(
        &self,
        component: &str,
        healthy: bool,
        _reason: Option<&str>,
    ) -> Option<HealthEvent> {
        self.record_check(component, healthy)
    }

    /// Get current health status for all monitored components.
    pub fn status(&self) -> Vec<ComponentHealth> {
        self.components
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get health status for a specific component.
    pub fn component_status(&self, component: &str) -> Option<ComponentHealth> {
        self.components.get(component).map(|entry| entry.value().clone())
    }

    /// Get the configuration.
    pub fn config(&self) -> &HealthMonitorConfig {
        &self.config
    }

    /// Get the number of monitored components.
    pub fn component_count(&self) -> usize {
        self.components.len()
    }
}

// ── Alerting ───────────────────────────────────────────────────────

/// Alert payload sent via webhook or Telegram.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertPayload {
    pub component: String,
    pub status: String,
    pub failure_count: u32,
    pub timestamp: DateTime<Utc>,
    pub event_type: String,
}

/// Send an alert via webhook (POST JSON).
pub async fn send_webhook_alert(url: &str, event: &HealthEvent) -> Result<(), anyhow::Error> {
    let payload = match event {
        HealthEvent::Alert {
            component,
            failures,
        } => AlertPayload {
            component: component.clone(),
            status: "unhealthy".to_string(),
            failure_count: *failures,
            timestamp: Utc::now(),
            event_type: "alert".to_string(),
        },
        HealthEvent::Recovery { component } => AlertPayload {
            component: component.clone(),
            status: "healthy".to_string(),
            failure_count: 0,
            timestamp: Utc::now(),
            event_type: "recovery".to_string(),
        },
    };

    let client = reqwest::Client::new();
    let response = client.post(url).json(&payload).send().await?;

    if !response.status().is_success() {
        warn!(
            url = url,
            status = %response.status(),
            "Webhook alert delivery failed"
        );
        anyhow::bail!(
            "Webhook returned non-success status: {}",
            response.status()
        );
    }

    info!(
        component = %payload.component,
        event_type = %payload.event_type,
        "Webhook alert delivered successfully"
    );
    Ok(())
}

/// Send an alert via Telegram to the configured admin user.
pub async fn send_telegram_alert(
    bot_token: &str,
    admin_chat_id: &str,
    event: &HealthEvent,
) -> Result<(), anyhow::Error> {
    let message = match event {
        HealthEvent::Alert {
            component,
            failures,
        } => {
            format!(
                "🚨 *Health Alert*\n\nComponent: `{}`\nStatus: Unhealthy\nConsecutive failures: {}\nTime: {}",
                component, failures, Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
            )
        }
        HealthEvent::Recovery { component } => {
            format!(
                "✅ *Recovery*\n\nComponent: `{}`\nStatus: Healthy\nTime: {}",
                component,
                Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
            )
        }
    };

    let url = format!(
        "https://api.telegram.org/bot{}/sendMessage",
        bot_token
    );

    let client = reqwest::Client::new();
    let response = client
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": admin_chat_id,
            "text": message,
            "parse_mode": "Markdown",
        }))
        .send()
        .await?;

    if !response.status().is_success() {
        error!(
            admin_chat_id = admin_chat_id,
            status = %response.status(),
            "Telegram alert delivery failed"
        );
        anyhow::bail!(
            "Telegram API returned non-success status: {}",
            response.status()
        );
    }

    info!(
        admin_chat_id = admin_chat_id,
        "Telegram alert delivered successfully"
    );
    Ok(())
}

// ── Periodic Check Runner ──────────────────────────────────────────

/// Component check function type.
pub type CheckFn = Arc<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = bool> + Send>> + Send + Sync>;

/// Start the periodic health check loop.
///
/// This spawns a tokio task that runs health checks at the configured interval.
/// It checks channel connectivity, model reachability, and session store availability.
pub async fn run_periodic_checks(
    monitor: Arc<HealthMonitor>,
    checks: Vec<(String, CheckFn)>,
    cancel: tokio_util::sync::CancellationToken,
) {
    let interval_secs = monitor.config().check_interval_secs;
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(interval_secs));

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                info!("Health monitor shutting down");
                break;
            }
            _ = interval.tick() => {
                for (component_name, check_fn) in &checks {
                    let healthy = check_fn().await;
                    if let Some(event) = monitor.record_check(component_name, healthy) {
                        // Dispatch alert/recovery
                        dispatch_event(&monitor, &event).await;
                    }
                }
            }
        }
    }
}

/// Dispatch a health event to configured alerting channels.
async fn dispatch_event(monitor: &HealthMonitor, event: &HealthEvent) {
    let config = monitor.config();

    // Log the event
    match event {
        HealthEvent::Alert { component, failures } => {
            error!(
                component = %component,
                failures = failures,
                "Health alert: component unhealthy"
            );
        }
        HealthEvent::Recovery { component } => {
            info!(
                component = %component,
                "Health recovery: component restored"
            );
        }
    }

    // Webhook alerting
    if let Some(ref webhook_url) = config.alert_webhook_url {
        if let Err(e) = send_webhook_alert(webhook_url, event).await {
            error!(error = %e, "Failed to send webhook alert");
        }
    }

    // Telegram alerting
    if let Some(ref admin_chat_id) = config.alert_telegram_admin {
        // For Telegram alerting, we need a bot token from the environment
        // or config. For now, we check the TELEGRAM_BOT_TOKEN env var.
        if let Ok(bot_token) = std::env::var("TELEGRAM_BOT_TOKEN") {
            if let Err(e) = send_telegram_alert(&bot_token, admin_chat_id, event).await {
                error!(error = %e, "Failed to send Telegram alert");
            }
        } else {
            warn!("Telegram alerting configured but TELEGRAM_BOT_TOKEN not set");
        }
    }
}

// ── Health Endpoint Response ───────────────────────────────────────

/// Build the per-component health breakdown for the `/health` endpoint.
pub fn build_health_response(monitor: &HealthMonitor) -> serde_json::Value {
    let components: Vec<serde_json::Value> = monitor
        .status()
        .into_iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name,
                "status": match &c.status {
                    HealthStatus::Healthy => "healthy".to_string(),
                    HealthStatus::Degraded { reason } => format!("degraded: {}", reason),
                    HealthStatus::Unhealthy { reason } => format!("unhealthy: {}", reason),
                },
                "consecutive_failures": c.consecutive_failures,
                "last_check": c.last_check.to_rfc3339(),
            })
        })
        .collect();

    serde_json::json!({
        "components": components,
    })
}

// ── Unit Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> HealthMonitorConfig {
        HealthMonitorConfig {
            check_interval_secs: 60,
            failure_threshold: 3,
            alert_webhook_url: None,
            alert_telegram_admin: None,
        }
    }

    #[test]
    fn test_healthy_check_no_event() {
        let monitor = HealthMonitor::new(default_config());
        let event = monitor.record_check("channel", true);
        assert_eq!(event, None);
    }

    #[test]
    fn test_single_failure_no_alert() {
        let monitor = HealthMonitor::new(default_config());
        let event = monitor.record_check("channel", false);
        assert_eq!(event, None);
    }

    #[test]
    fn test_two_failures_no_alert() {
        let monitor = HealthMonitor::new(default_config());
        monitor.record_check("channel", false);
        let event = monitor.record_check("channel", false);
        assert_eq!(event, None);
    }

    #[test]
    fn test_three_failures_emits_alert() {
        let monitor = HealthMonitor::new(default_config());
        monitor.record_check("channel", false);
        monitor.record_check("channel", false);
        let event = monitor.record_check("channel", false);
        assert_eq!(
            event,
            Some(HealthEvent::Alert {
                component: "channel".to_string(),
                failures: 3,
            })
        );
    }

    #[test]
    fn test_no_duplicate_alert_after_threshold() {
        let monitor = HealthMonitor::new(default_config());
        // Reach threshold
        monitor.record_check("channel", false);
        monitor.record_check("channel", false);
        monitor.record_check("channel", false); // alert emitted here

        // Further failures should NOT emit another alert
        let event = monitor.record_check("channel", false);
        assert_eq!(event, None);

        let event = monitor.record_check("channel", false);
        assert_eq!(event, None);
    }

    #[test]
    fn test_recovery_after_alert() {
        let monitor = HealthMonitor::new(default_config());
        // Reach threshold
        monitor.record_check("channel", false);
        monitor.record_check("channel", false);
        monitor.record_check("channel", false); // alert

        // Recovery
        let event = monitor.record_check("channel", true);
        assert_eq!(
            event,
            Some(HealthEvent::Recovery {
                component: "channel".to_string(),
            })
        );
    }

    #[test]
    fn test_no_recovery_without_prior_alert() {
        let monitor = HealthMonitor::new(default_config());
        // Two failures (below threshold, no alert)
        monitor.record_check("channel", false);
        monitor.record_check("channel", false);

        // Recovery without having been alerted → no recovery event
        let event = monitor.record_check("channel", true);
        assert_eq!(event, None);
    }

    #[test]
    fn test_no_duplicate_recovery() {
        let monitor = HealthMonitor::new(default_config());
        // Alert
        monitor.record_check("channel", false);
        monitor.record_check("channel", false);
        monitor.record_check("channel", false);

        // First recovery
        let event = monitor.record_check("channel", true);
        assert_eq!(
            event,
            Some(HealthEvent::Recovery {
                component: "channel".to_string(),
            })
        );

        // Second healthy check → no duplicate recovery
        let event = monitor.record_check("channel", true);
        assert_eq!(event, None);
    }

    #[test]
    fn test_re_alert_after_recovery() {
        let monitor = HealthMonitor::new(default_config());
        // First alert cycle
        monitor.record_check("channel", false);
        monitor.record_check("channel", false);
        monitor.record_check("channel", false); // alert

        // Recovery
        monitor.record_check("channel", true); // recovery

        // Second alert cycle
        monitor.record_check("channel", false);
        monitor.record_check("channel", false);
        let event = monitor.record_check("channel", false);
        assert_eq!(
            event,
            Some(HealthEvent::Alert {
                component: "channel".to_string(),
                failures: 3,
            })
        );
    }

    #[test]
    fn test_multiple_components_independent() {
        let monitor = HealthMonitor::new(default_config());

        // Component A fails
        monitor.record_check("channel_a", false);
        monitor.record_check("channel_a", false);
        let event = monitor.record_check("channel_a", false);
        assert_eq!(
            event,
            Some(HealthEvent::Alert {
                component: "channel_a".to_string(),
                failures: 3,
            })
        );

        // Component B is healthy — no event
        let event = monitor.record_check("channel_b", true);
        assert_eq!(event, None);

        // Component B fails independently
        monitor.record_check("channel_b", false);
        monitor.record_check("channel_b", false);
        let event = monitor.record_check("channel_b", false);
        assert_eq!(
            event,
            Some(HealthEvent::Alert {
                component: "channel_b".to_string(),
                failures: 3,
            })
        );
    }

    #[test]
    fn test_status_returns_all_components() {
        let monitor = HealthMonitor::new(default_config());
        monitor.record_check("channel", true);
        monitor.record_check("model", false);
        monitor.record_check("session_store", true);

        let status = monitor.status();
        assert_eq!(status.len(), 3);
    }

    #[test]
    fn test_component_status() {
        let monitor = HealthMonitor::new(default_config());
        monitor.record_check("channel", false);
        monitor.record_check("channel", false);

        let status = monitor.component_status("channel").unwrap();
        assert_eq!(status.consecutive_failures, 2);
        assert!(matches!(status.status, HealthStatus::Degraded { .. }));
    }

    #[test]
    fn test_custom_threshold() {
        let config = HealthMonitorConfig {
            check_interval_secs: 60,
            failure_threshold: 5,
            alert_webhook_url: None,
            alert_telegram_admin: None,
        };
        let monitor = HealthMonitor::new(config);

        // 4 failures — no alert (threshold is 5)
        for _ in 0..4 {
            let event = monitor.record_check("channel", false);
            assert_eq!(event, None);
        }

        // 5th failure — alert
        let event = monitor.record_check("channel", false);
        assert_eq!(
            event,
            Some(HealthEvent::Alert {
                component: "channel".to_string(),
                failures: 5,
            })
        );
    }

    #[test]
    fn test_failure_reset_on_success() {
        let monitor = HealthMonitor::new(default_config());
        // Two failures
        monitor.record_check("channel", false);
        monitor.record_check("channel", false);

        // Success resets counter
        monitor.record_check("channel", true);

        // Need 3 more failures for alert
        monitor.record_check("channel", false);
        monitor.record_check("channel", false);
        let event = monitor.record_check("channel", false);
        assert_eq!(
            event,
            Some(HealthEvent::Alert {
                component: "channel".to_string(),
                failures: 3,
            })
        );
    }

    #[test]
    fn test_build_health_response() {
        let monitor = HealthMonitor::new(default_config());
        monitor.record_check("channel", true);
        monitor.record_check("model", false);

        let response = build_health_response(&monitor);
        let components = response["components"].as_array().unwrap();
        assert_eq!(components.len(), 2);
    }
}
