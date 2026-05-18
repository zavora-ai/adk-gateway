//! Configuration hot-reload via file-system monitoring.
//!
//! Design: ConfigWatcher [R7.1–R7.5]

use crate::config::{self, GatewayConfig};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;

// ── Validation ─────────────────────────────────────────────────────

/// Validation errors for config reload.
#[derive(Debug, Clone)]
pub struct ConfigValidationError {
    pub message: String,
}

impl std::fmt::Display for ConfigValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "config validation error: {}", self.message)
    }
}

impl std::error::Error for ConfigValidationError {}

/// Validate a new config before applying it.
pub fn validate_config(config: &GatewayConfig) -> Result<(), ConfigValidationError> {
    // Port must be non-zero
    if config.gateway.port == 0 {
        return Err(ConfigValidationError {
            message: "gateway port must be non-zero".into(),
        });
    }

    // Validate cron job IDs are unique
    let mut cron_ids = std::collections::HashSet::new();
    for job in &config.cron.jobs {
        if !cron_ids.insert(&job.id) {
            return Err(ConfigValidationError {
                message: format!("duplicate cron job id: {}", job.id),
            });
        }
    }

    // Validate agent IDs are unique
    let mut agent_ids = std::collections::HashSet::new();
    for agent in &config.agents.list {
        if !agent_ids.insert(&agent.id) {
            return Err(ConfigValidationError {
                message: format!("duplicate agent id: {}", agent.id),
            });
        }
    }

    // Validate routing bindings reference known agents
    for binding in &config.routing.bindings {
        if !config.agents.list.is_empty()
            && !config.agents.list.iter().any(|a| a.id == binding.agent_id)
        {
            return Err(ConfigValidationError {
                message: format!(
                    "routing binding references unknown agent: {}",
                    binding.agent_id
                ),
            });
        }
    }

    // Validate RAG config if present
    if let Some(ref rag) = config.rag {
        match rag.embedding.provider.as_str() {
            "gemini" | "openai" => {}
            other => {
                return Err(ConfigValidationError {
                    message: format!("unknown RAG embedding provider: {other}"),
                });
            }
        }
    }

    // Validate memory config if present
    if let Some(ref mem) = config.memory {
        match mem.embedding.provider.as_str() {
            "gemini" | "openai" => {}
            other => {
                return Err(ConfigValidationError {
                    message: format!("unknown memory embedding provider: {other}"),
                });
            }
        }
    }

    // Validate Telegram config: bot_token is required when Telegram is enabled
    if let Some(ref tg) = config.channels.telegram {
        if tg.bot_token.is_empty() {
            return Err(ConfigValidationError {
                message: "channels.telegram.bot_token is required when Telegram is enabled".into(),
            });
        }
    }

    // Validate Slack config: bot_token and app_token are required when Slack is enabled
    if let Some(ref slack) = config.channels.slack {
        if slack.bot_token.is_empty() {
            return Err(ConfigValidationError {
                message: "channels.slack.bot_token is required when Slack is enabled".into(),
            });
        }
        if slack.app_token.is_empty() {
            return Err(ConfigValidationError {
                message: "channels.slack.app_token is required when Slack is enabled".into(),
            });
        }
    }

    // Validate drain timeout is reasonable
    if config.gateway.drain_timeout_secs == 0 {
        return Err(ConfigValidationError {
            message: "gateway.drain_timeout_secs must be > 0".into(),
        });
    }

    // Validate cron expressions are non-empty
    for job in &config.cron.jobs {
        if job.schedule.trim().is_empty() {
            return Err(ConfigValidationError {
                message: format!("cron job '{}' has empty schedule", job.id),
            });
        }
    }

    Ok(())
}

// ── ConfigDiff ─────────────────────────────────────────────────────

/// Describes what changed between two configs.
#[derive(Debug, Clone, Default)]
pub struct ConfigDiff {
    pub channels_changed: bool,
    pub routing_changed: bool,
    pub session_changed: bool,
    pub cron_changed: bool,
    pub plugins_changed: bool,
    pub auth_changed: bool,
    pub rag_changed: bool,
    pub memory_changed: bool,
    pub telemetry_changed: bool,
    pub conventions_changed: bool,
    pub hooks_changed: bool,
    pub coding_agents_changed: bool,
}

impl ConfigDiff {
    /// Compute the diff between old and new configs.
    pub fn compute(old: &GatewayConfig, new: &GatewayConfig) -> Self {
        Self {
            channels_changed: old.channels != new.channels,
            routing_changed: old.routing != new.routing,
            session_changed: old.session != new.session,
            cron_changed: old.cron != new.cron,
            plugins_changed: old.plugins != new.plugins,
            auth_changed: old.auth != new.auth,
            rag_changed: old.rag != new.rag,
            memory_changed: old.memory != new.memory,
            telemetry_changed: old.telemetry != new.telemetry,
            conventions_changed: old.conventions != new.conventions,
            hooks_changed: old.hooks != new.hooks,
            coding_agents_changed: old.coding_agents != new.coding_agents,
        }
    }

    /// Returns true if any field changed.
    pub fn has_changes(&self) -> bool {
        self.channels_changed
            || self.routing_changed
            || self.session_changed
            || self.cron_changed
            || self.plugins_changed
            || self.auth_changed
            || self.rag_changed
            || self.memory_changed
            || self.telemetry_changed
            || self.conventions_changed
            || self.hooks_changed
            || self.coding_agents_changed
    }
}

// ── ConfigWatcher ──────────────────────────────────────────────────

/// Watches the config file for changes and sends validated new configs.
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
    #[allow(dead_code)] // Stored for config_path() accessor used in diagnostics
    config_path: PathBuf,
}

impl ConfigWatcher {
    /// Start watching the config file. Sends validated new configs on the channel.
    pub fn start(
        config_path: PathBuf,
        current_config: Arc<arc_swap::ArcSwap<GatewayConfig>>,
    ) -> anyhow::Result<(Self, mpsc::Receiver<GatewayConfig>)> {
        let (tx, rx) = mpsc::channel::<GatewayConfig>(4);
        let path_clone = config_path.clone();
        let config_filename = config_path
            .file_name()
            .map(|f| f.to_os_string())
            .unwrap_or_default();

        // Debounce: track last reload time to avoid rapid-fire reloads
        let last_reload = std::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(10));

        let mut watcher =
            notify::recommended_watcher(move |res: Result<Event, notify::Error>| match res {
                Ok(event) => {
                    if matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
                        // Only react to changes to the actual config file
                        let is_config_file = event.paths.iter().any(|p| {
                            p.file_name().map(|f| f == config_filename).unwrap_or(false)
                        });
                        if !is_config_file {
                            return;
                        }

                        // Debounce: skip if last reload was less than 2 seconds ago
                        {
                            let mut last = last_reload.lock().unwrap();
                            if last.elapsed() < std::time::Duration::from_secs(2) {
                                tracing::debug!("config change debounced, skipping");
                                return;
                            }
                            *last = std::time::Instant::now();
                        }

                        tracing::debug!("config file change detected");
                        match config::load_config(&path_clone) {
                            Ok(new_config) => match validate_config(&new_config) {
                                Ok(()) => {
                                    let old = current_config.load();
                                    let diff = ConfigDiff::compute(&old, &new_config);
                                    if diff.has_changes() {
                                        tracing::info!("config validated, applying changes");
                                        current_config.store(Arc::new(new_config.clone()));
                                        let _ = tx.blocking_send(new_config);
                                    } else {
                                        tracing::debug!("config unchanged, skipping reload");
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "invalid config, keeping previous"
                                    );
                                }
                            },
                            Err(e) => {
                                tracing::warn!(
                                    error = %e,
                                    "failed to parse config file, keeping previous"
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::error!(error = %e, "file watcher error");
                }
            })?;

        // Watch the parent directory (some editors write to temp + rename)
        let watch_path = config_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_path_buf();
        watcher.watch(&watch_path, RecursiveMode::NonRecursive)?;

        tracing::info!(path = %config_path.display(), "config watcher started");

        Ok((
            Self {
                _watcher: watcher,
                config_path,
            },
            rx,
        ))
    }

    /// Get the path being watched.
    #[allow(dead_code)] // Available for diagnostic reporting; exposed via /status endpoint
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;

    #[test]
    fn test_validate_valid_config() {
        let config = GatewayConfig::default();
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn test_validate_zero_port() {
        let mut config = GatewayConfig::default();
        config.gateway.port = 0;
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_validate_duplicate_cron_ids() {
        let mut config = GatewayConfig::default();
        config.cron.jobs = vec![
            CronJob {
                id: "job1".into(),
                schedule: "* * * * *".into(),
                message: "hello".into(),
                deliver_to: None,
                suppress_keyword: None,
                target: None,
                workspace: None,
            },
            CronJob {
                id: "job1".into(),
                schedule: "0 * * * *".into(),
                message: "world".into(),
                deliver_to: None,
                suppress_keyword: None,
                target: None,
                workspace: None,
            },
        ];
        let err = validate_config(&config).unwrap_err();
        assert!(err.message.contains("duplicate cron job id"));
    }

    #[test]
    fn test_validate_duplicate_agent_ids() {
        let mut config = GatewayConfig::default();
        config.agents.list = vec![
            AgentEntry {
                id: "agent1".into(),
                default: true,
                workspace: None,
                model: None,
                skills: vec![],
                browser: None,
                tools: vec![],
                max_iterations: None,
                acp: None,
            },
            AgentEntry {
                id: "agent1".into(),
                default: false,
                workspace: None,
                model: None,
                skills: vec![],
                browser: None,
                tools: vec![],
                max_iterations: None,
                acp: None,
            },
        ];
        let err = validate_config(&config).unwrap_err();
        assert!(err.message.contains("duplicate agent id"));
    }

    #[test]
    fn test_validate_unknown_rag_provider() {
        let mut config = GatewayConfig::default();
        config.rag = Some(RagConfig {
            vector_store: VectorStoreBackend::InMemory,
            connection_string: None,
            embedding: EmbeddingConfig {
                provider: "unknown".into(),
                model: None,
            },
            chunking: ChunkingStrategy::default(),
            chunk_size: None,
            chunk_overlap: None,
            watch_dirs: vec![],
            ingest_webhook: None,
        });
        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn test_config_diff_no_changes() {
        let config = GatewayConfig::default();
        let diff = ConfigDiff::compute(&config, &config);
        assert!(!diff.has_changes());
    }

    #[test]
    fn test_config_diff_port_change() {
        let old = GatewayConfig::default();
        let mut new = old.clone();
        new.gateway.port = 9999;
        // Port is in gateway settings, not tracked separately — but channels etc. unchanged
        let diff = ConfigDiff::compute(&old, &new);
        // gateway settings aren't in the diff fields, so no changes detected
        // This is by design — only tracked subsystems trigger reconciliation
        assert!(!diff.has_changes());
    }

    #[test]
    fn test_config_diff_cron_change() {
        let old = GatewayConfig::default();
        let mut new = old.clone();
        new.cron.jobs.push(CronJob {
            id: "new_job".into(),
            schedule: "* * * * *".into(),
            message: "hello".into(),
            deliver_to: None,
            suppress_keyword: None,
            target: None,
            workspace: None,
        });
        let diff = ConfigDiff::compute(&old, &new);
        assert!(diff.has_changes());
        assert!(diff.cron_changed);
        assert!(!diff.channels_changed);
    }

    #[test]
    fn test_config_diff_routing_change() {
        let old = GatewayConfig::default();
        let mut new = old.clone();
        new.routing.bindings.push(RoutingBinding {
            agent_id: "agent1".into(),
            match_rule: RoutingMatch {
                channel: Some("telegram".into()),
                account_id: None,
                peer: None,
            },
        });
        let diff = ConfigDiff::compute(&old, &new);
        assert!(diff.routing_changed);
    }

    #[test]
    fn test_validate_routing_unknown_agent() {
        let mut config = GatewayConfig::default();
        config.agents.list = vec![AgentEntry {
            id: "agent1".into(),
            default: true,
            workspace: None,
            model: None,
            skills: vec![],
            browser: None,
            tools: vec![],
            max_iterations: None,
            acp: None,
        }];
        config.routing.bindings = vec![RoutingBinding {
            agent_id: "nonexistent".into(),
            match_rule: RoutingMatch {
                channel: None,
                account_id: None,
                peer: None,
            },
        }];
        let err = validate_config(&config).unwrap_err();
        assert!(err.message.contains("unknown agent"));
    }
}
