//! Background health monitor for coding agents.
//!
//! Periodically probes registered agents' ACP endpoints and updates their
//! connection status. Handles auto-connect on registration and reconnection
//! after transient failures.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::time::interval;
use tracing::{debug, info, warn};

use super::backend::ConfigDrivenBackend;
use super::backend::CodingAgentBackend;
use super::registry::CodingAgentRegistry;
use super::status::AgentConnectionStatus;

/// Configuration for the health monitor.
#[derive(Debug, Clone)]
pub struct HealthMonitorConfig {
    /// How often to probe agents (default: 30 seconds).
    pub check_interval: Duration,
    /// Consecutive failures before marking as Error (default: 3).
    pub failure_threshold: u32,
}

impl Default for HealthMonitorConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(30),
            failure_threshold: 3,
        }
    }
}

/// Tracks consecutive failure counts per agent.
struct FailureTracker {
    counts: dashmap::DashMap<String, u32>,
}

impl FailureTracker {
    fn new() -> Self {
        Self {
            counts: dashmap::DashMap::new(),
        }
    }

    fn record_success(&self, agent_id: &str) {
        self.counts.insert(agent_id.to_string(), 0);
    }

    fn record_failure(&self, agent_id: &str) -> u32 {
        let mut entry = self.counts.entry(agent_id.to_string()).or_insert(0);
        *entry += 1;
        *entry
    }
}

/// Spawns the background health monitor loop.
///
/// Returns a `JoinHandle` that can be used to abort the monitor on shutdown.
pub fn spawn_health_monitor(
    registry: Arc<CodingAgentRegistry>,
    config: HealthMonitorConfig,
) -> tokio::task::JoinHandle<()> {
    info!(
        interval_secs = config.check_interval.as_secs(),
        failure_threshold = config.failure_threshold,
        "coding agent health monitor started"
    );

    tokio::spawn(async move {
        let mut ticker = interval(config.check_interval);
        let tracker = FailureTracker::new();

        // Initial probe on startup (after a short delay to let agents register)
        tokio::time::sleep(Duration::from_secs(5)).await;

        loop {
            ticker.tick().await;

            let agents = registry.list_agents();
            if agents.is_empty() {
                debug!("health monitor: no agents registered, skipping");
                continue;
            }

            // Clean up failure tracking for agents that were unregistered
            let agent_ids: std::collections::HashSet<&str> =
                agents.iter().map(|a| a.id.as_str()).collect();
            tracker.counts.retain(|id, _| agent_ids.contains(id.as_str()));

            for agent in &agents {
                // Skip agents with stdio transport — they're managed by the process manager
                if agent.config.transport.as_ref().is_some_and(|t| matches!(t, super::config::AgentTransport::Stdio { .. })) {
                    continue;
                }

                // Skip agents with placeholder endpoints (acp:// scheme)
                if agent.endpoint.starts_with("acp://") || agent.endpoint.is_empty() {
                    continue;
                }

                let backend = ConfigDrivenBackend::new(
                    // We need the backend definition for the health check,
                    // but we can probe the endpoint directly
                    super::config::BackendDefinition {
                        agent_type: agent.backend_type.clone(),
                        display_name: String::new(),
                        cli_command: String::new(),
                        install_check_command: String::new(),
                        auth_method: super::config::AuthMethod::None,
                        capabilities: super::config::AgentCapabilities::default(),
                        install_instructions: String::new(),
                        install_instructions_windows: None,
                        install_instructions_linux: None,
                    },
                    Some(agent.endpoint.clone()),
                );

                let health = backend.health_check().await;
                registry.record_health_check(&agent.id);

                match health {
                    Ok(status) if status.reachable => {
                        tracker.record_success(&agent.id);
                        let current = &agent.status;
                        if !matches!(current, AgentConnectionStatus::Connected) {
                            info!(agent_id = %agent.id, "agent connected");
                            let _ = registry.update_status(
                                &agent.id,
                                AgentConnectionStatus::Connected,
                            );
                        }
                    }
                    Ok(_) | Err(_) => {
                        let failures = tracker.record_failure(&agent.id);
                        let current = &agent.status;

                        if failures >= config.failure_threshold {
                            if !matches!(current, AgentConnectionStatus::Error { .. }) {
                                warn!(
                                    agent_id = %agent.id,
                                    failures = failures,
                                    "agent unreachable, marking as error"
                                );
                                let _ = registry.update_status(
                                    &agent.id,
                                    AgentConnectionStatus::Error {
                                        message: format!(
                                            "Unreachable after {} consecutive health checks",
                                            failures
                                        ),
                                        since: Utc::now(),
                                    },
                                );
                            }
                        } else if !matches!(current, AgentConnectionStatus::Disconnected { .. }) {
                            debug!(
                                agent_id = %agent.id,
                                failures = failures,
                                threshold = config.failure_threshold,
                                "agent health check failed"
                            );
                            let _ = registry.update_status(
                                &agent.id,
                                AgentConnectionStatus::Disconnected { since: Utc::now() },
                            );
                        }
                    }
                }
            }
        }
    })
}
