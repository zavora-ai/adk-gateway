//! Central registry for configured coding agents and their runtime state.
//!
//! Provides thread-safe management of registered coding agents, backend definitions,
//! and real-time status broadcasting via WebSocket events. Supports hot-reload of
//! backend definitions from configuration without restart.

use std::time::Instant;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::broadcast;
use tracing::{info, warn};

use super::config::{BackendDefinition, CodingAgentInstanceConfig, CodingAgentsConfig};
use super::error::CodingAgentError;
use super::status::{AgentConnectionStatus, AgentStatusEvent};

/// A registered coding agent instance with its runtime state.
#[derive(Debug, Clone)]
pub struct RegisteredAgent {
    /// Unique identifier for this agent instance.
    pub id: String,
    /// The backend type (references a BackendDefinition).
    pub backend_type: String,
    /// Full configuration for this agent instance.
    pub config: CodingAgentInstanceConfig,
    /// Current connection status.
    pub status: AgentConnectionStatus,
    /// When the last health check was performed.
    pub last_health_check: Option<Instant>,
    /// When the last task completed successfully.
    pub last_successful_task: Option<DateTime<Utc>>,
    /// The ACP endpoint URL for this agent (used to create AcpTool on demand).
    pub endpoint: String,
}

/// Central registry for all configured coding agents.
/// Thread-safe via DashMap, supports hot-reload of backend definitions.
pub struct CodingAgentRegistry {
    /// Registered agents indexed by their unique ID.
    agents: DashMap<String, RegisteredAgent>,
    /// Backend definitions loaded from config, indexed by agent_type.
    backends: DashMap<String, BackendDefinition>,
    /// WebSocket broadcast channel for status update events.
    status_tx: broadcast::Sender<AgentStatusEvent>,
}

impl CodingAgentRegistry {
    /// Create a new empty registry with the given broadcast channel capacity.
    ///
    /// The `channel_capacity` determines how many status events can be buffered
    /// before slow receivers start missing events.
    pub fn new(channel_capacity: usize) -> Self {
        let (status_tx, _) = broadcast::channel(channel_capacity);
        Self {
            agents: DashMap::new(),
            backends: DashMap::new(),
            status_tx,
        }
    }

    /// Create a registry pre-populated from a `CodingAgentsConfig`.
    ///
    /// Loads backend definitions and registers all configured agent instances.
    pub fn from_config(config: &CodingAgentsConfig) -> Self {
        let registry = Self::new(64);
        registry.load_backends_from_config(&config.backends);

        for agent_config in &config.agents {
            if let Err(e) = registry.register_agent(agent_config.clone()) {
                warn!(
                    agent_id = %agent_config.id,
                    error = %e,
                    "Failed to register agent from config"
                );
            }
        }

        registry
    }

    /// Register a new coding agent instance.
    ///
    /// Returns an error if an agent with the same ID is already registered.
    pub fn register_agent(
        &self,
        config: CodingAgentInstanceConfig,
    ) -> Result<(), CodingAgentError> {
        let id = config.id.clone();

        if self.agents.contains_key(&id) {
            return Err(CodingAgentError::ConfigValidation(format!(
                "Agent with id '{}' is already registered",
                id
            )));
        }

        let agent = RegisteredAgent {
            id: id.clone(),
            backend_type: config.backend_type.clone(),
            endpoint: config.endpoint.clone(),
            config,
            status: AgentConnectionStatus::Unknown,
            last_health_check: None,
            last_successful_task: None,
        };

        self.agents.insert(id.clone(), agent);
        info!(agent_id = %id, "Registered coding agent");
        Ok(())
    }

    /// Unregister a coding agent by its ID.
    ///
    /// Returns the removed agent, or an error if not found.
    pub fn unregister_agent(&self, agent_id: &str) -> Result<RegisteredAgent, CodingAgentError> {
        match self.agents.remove(agent_id) {
            Some((_, agent)) => {
                info!(agent_id = %agent_id, "Unregistered coding agent");
                Ok(agent)
            }
            None => Err(CodingAgentError::AgentNotFound(agent_id.to_string())),
        }
    }

    /// Get a clone of a registered agent by its ID.
    pub fn get_agent(&self, agent_id: &str) -> Option<RegisteredAgent> {
        self.agents.get(agent_id).map(|entry| entry.value().clone())
    }

    /// List all registered agents.
    pub fn list_agents(&self) -> Vec<RegisteredAgent> {
        self.agents
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Load (or reload) backend definitions from configuration.
    ///
    /// This replaces all existing backend definitions with the new set.
    /// Existing registered agents are not affected — only the available
    /// backend type catalog is updated.
    pub fn load_backends_from_config(&self, backends: &[BackendDefinition]) {
        // Clear existing backends
        self.backends.clear();

        let mut loaded = 0;
        for backend in backends {
            let missing = super::config::validate_backend_definition(backend);
            if missing.is_empty() {
                self.backends
                    .insert(backend.agent_type.clone(), backend.clone());
                loaded += 1;
            } else {
                warn!(
                    agent_type = %backend.agent_type,
                    missing_fields = ?missing,
                    "Skipping invalid backend definition"
                );
            }
        }

        info!(count = loaded, "Loaded backend definitions from config");
    }

    /// Get a backend definition by its agent type identifier.
    pub fn get_backend(&self, agent_type: &str) -> Option<BackendDefinition> {
        self.backends.get(agent_type).map(|entry| entry.value().clone())
    }

    /// List all loaded backend definitions.
    pub fn list_backends(&self) -> Vec<BackendDefinition> {
        self.backends
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Resolve an agent by its shorthand alias.
    ///
    /// Searches all registered agents for one whose `alias` field matches
    /// the given alias (case-insensitive). Returns the agent ID if found.
    pub fn resolve_by_alias(&self, alias: &str) -> Option<String> {
        let alias_lower = alias.to_lowercase();
        self.agents
            .iter()
            .find(|entry| {
                entry
                    .value()
                    .config
                    .alias
                    .as_ref()
                    .map(|a| a.to_lowercase() == alias_lower)
                    .unwrap_or(false)
            })
            .map(|entry| entry.value().id.clone())
    }

    /// Update the connection status of an agent and emit a status event.
    ///
    /// If the new status differs from the current status, an `AgentStatusEvent`
    /// is broadcast to all subscribers (WebSocket clients).
    ///
    /// Returns an error if the agent is not found.
    pub fn update_status(
        &self,
        agent_id: &str,
        new_status: AgentConnectionStatus,
    ) -> Result<(), CodingAgentError> {
        let mut agent = self
            .agents
            .get_mut(agent_id)
            .ok_or_else(|| CodingAgentError::AgentNotFound(agent_id.to_string()))?;

        let previous_status = agent.status.clone();

        // Only emit event if status actually changed
        if previous_status != new_status {
            let status_label = match &new_status {
                AgentConnectionStatus::Connected => "connected".to_string(),
                AgentConnectionStatus::Disconnected { .. } => "disconnected".to_string(),
                AgentConnectionStatus::Error { message, .. } => format!("error: {}", message),
                AgentConnectionStatus::Unknown => "unknown".to_string(),
            };

            agent.status = new_status.clone();

            let event = AgentStatusEvent {
                agent_id: agent_id.to_string(),
                previous_status,
                new_status,
                timestamp: Utc::now(),
            };

            // Broadcast to subscribers; ignore error if no receivers are listening
            let _ = self.status_tx.send(event);

            info!(
                agent_id = %agent_id,
                status = %status_label,
                "Agent status updated"
            );
        }

        Ok(())
    }

    /// Subscribe to agent status events.
    ///
    /// Returns a broadcast receiver that will receive `AgentStatusEvent` messages
    /// whenever any agent's status changes.
    pub fn subscribe_status(&self) -> broadcast::Receiver<AgentStatusEvent> {
        self.status_tx.subscribe()
    }

    /// Get a reference to the status broadcast sender.
    ///
    /// Useful for components that need to forward the sender without
    /// subscribing themselves.
    pub fn status_sender(&self) -> &broadcast::Sender<AgentStatusEvent> {
        &self.status_tx
    }

    /// Record a successful task completion timestamp for an agent.
    pub fn record_successful_task(&self, agent_id: &str) {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            agent.last_successful_task = Some(Utc::now());
        }
    }

    /// Record that a health check was performed for an agent.
    pub fn record_health_check(&self, agent_id: &str) {
        if let Some(mut agent) = self.agents.get_mut(agent_id) {
            agent.last_health_check = Some(Instant::now());
        }
    }

    /// Get the number of registered agents.
    pub fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Get the number of loaded backend definitions.
    pub fn backend_count(&self) -> usize {
        self.backends.len()
    }

    /// Reload the registry from a new configuration.
    ///
    /// This is the hot-reload entry point called by the config watcher.
    /// It reloads backend definitions and registers any new agents found
    /// in the config that aren't already registered.
    pub fn reload_from_config(&self, config: &CodingAgentsConfig) {
        // Reload backend definitions
        self.load_backends_from_config(&config.backends);

        // Register any new agents (don't touch existing ones)
        for agent_config in &config.agents {
            if !self.agents.contains_key(&agent_config.id) {
                if let Err(e) = self.register_agent(agent_config.clone()) {
                    warn!(
                        agent_id = %agent_config.id,
                        error = %e,
                        "Failed to register agent during config reload"
                    );
                }
            }
        }

        info!("Config reload complete");
    }
}

/// Allow wrapping in Arc for shared ownership across async tasks.
impl std::fmt::Debug for CodingAgentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodingAgentRegistry")
            .field("agent_count", &self.agents.len())
            .field("backend_count", &self.backends.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use super::super::config::{
        AgentCapabilities, AuthMethod, BackendDefinition, CodingAgentInstanceConfig,
    };

    fn sample_agent_config(id: &str, alias: Option<&str>) -> CodingAgentInstanceConfig {
        CodingAgentInstanceConfig {
            id: id.to_string(),
            backend_type: "claude-code".to_string(),
            endpoint: format!("http://localhost:3000/{}", id),
            transport: None,
            workspaces: vec![PathBuf::from("/home/user/projects")],
            timeout_secs: Some(900),
            cost_cap_usd: Some(5.0),
            monthly_budget_usd: None,
            alias: alias.map(|a| a.to_string()),
            auth: None,
        }
    }

    fn sample_backend_definition(agent_type: &str) -> BackendDefinition {
        BackendDefinition {
            agent_type: agent_type.to_string(),
            display_name: format!("{} Display", agent_type),
            cli_command: agent_type.to_string(),
            install_check_command: format!("{} --version", agent_type),
            auth_method: AuthMethod::None,
            capabilities: AgentCapabilities::default(),
            install_instructions: "Install instructions here".to_string(),
            install_instructions_windows: None,
            install_instructions_linux: None,
        }
    }

    #[test]
    fn test_new_registry_is_empty() {
        let registry = CodingAgentRegistry::new(16);
        assert_eq!(registry.agent_count(), 0);
        assert_eq!(registry.backend_count(), 0);
    }

    #[test]
    fn test_register_agent_success() {
        let registry = CodingAgentRegistry::new(16);
        let config = sample_agent_config("agent-1", Some("cc"));

        let result = registry.register_agent(config);
        assert!(result.is_ok());
        assert_eq!(registry.agent_count(), 1);
    }

    #[test]
    fn test_register_agent_duplicate_id_fails() {
        let registry = CodingAgentRegistry::new(16);
        let config = sample_agent_config("agent-1", None);

        registry.register_agent(config.clone()).unwrap();
        let result = registry.register_agent(config);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CodingAgentError::ConfigValidation(_)));
    }

    #[test]
    fn test_unregister_agent_success() {
        let registry = CodingAgentRegistry::new(16);
        let config = sample_agent_config("agent-1", None);
        registry.register_agent(config).unwrap();

        let result = registry.unregister_agent("agent-1");
        assert!(result.is_ok());
        assert_eq!(registry.agent_count(), 0);
    }

    #[test]
    fn test_unregister_agent_not_found() {
        let registry = CodingAgentRegistry::new(16);
        let result = registry.unregister_agent("nonexistent");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CodingAgentError::AgentNotFound(_)));
    }

    #[test]
    fn test_get_agent() {
        let registry = CodingAgentRegistry::new(16);
        let config = sample_agent_config("agent-1", Some("cc"));
        registry.register_agent(config).unwrap();

        let agent = registry.get_agent("agent-1");
        assert!(agent.is_some());
        let agent = agent.unwrap();
        assert_eq!(agent.id, "agent-1");
        assert_eq!(agent.backend_type, "claude-code");
        assert!(matches!(agent.status, AgentConnectionStatus::Unknown));
    }

    #[test]
    fn test_get_agent_not_found() {
        let registry = CodingAgentRegistry::new(16);
        assert!(registry.get_agent("nonexistent").is_none());
    }

    #[test]
    fn test_list_agents() {
        let registry = CodingAgentRegistry::new(16);
        registry
            .register_agent(sample_agent_config("agent-1", None))
            .unwrap();
        registry
            .register_agent(sample_agent_config("agent-2", None))
            .unwrap();

        let agents = registry.list_agents();
        assert_eq!(agents.len(), 2);
    }

    #[test]
    fn test_load_backends_from_config() {
        let registry = CodingAgentRegistry::new(16);
        let backends = vec![
            sample_backend_definition("claude-code"),
            sample_backend_definition("kiro-cli"),
        ];

        registry.load_backends_from_config(&backends);
        assert_eq!(registry.backend_count(), 2);
        assert!(registry.get_backend("claude-code").is_some());
        assert!(registry.get_backend("kiro-cli").is_some());
    }

    #[test]
    fn test_load_backends_skips_invalid() {
        let registry = CodingAgentRegistry::new(16);
        let backends = vec![
            sample_backend_definition("valid-agent"),
            BackendDefinition {
                agent_type: "".to_string(), // invalid: empty agent_type
                display_name: "Invalid".to_string(),
                cli_command: "cmd".to_string(),
                install_check_command: "cmd --version".to_string(),
                auth_method: AuthMethod::None,
                capabilities: AgentCapabilities::default(),
                install_instructions: "".to_string(),
            install_instructions_windows: None,
            install_instructions_linux: None,
            },
        ];

        registry.load_backends_from_config(&backends);
        assert_eq!(registry.backend_count(), 1);
        assert!(registry.get_backend("valid-agent").is_some());
    }

    #[test]
    fn test_load_backends_replaces_existing() {
        let registry = CodingAgentRegistry::new(16);

        // Load initial set
        registry.load_backends_from_config(&vec![sample_backend_definition("old-agent")]);
        assert_eq!(registry.backend_count(), 1);

        // Reload with new set
        registry.load_backends_from_config(&vec![
            sample_backend_definition("new-agent-1"),
            sample_backend_definition("new-agent-2"),
        ]);
        assert_eq!(registry.backend_count(), 2);
        assert!(registry.get_backend("old-agent").is_none());
        assert!(registry.get_backend("new-agent-1").is_some());
    }

    #[test]
    fn test_resolve_by_alias() {
        let registry = CodingAgentRegistry::new(16);
        registry
            .register_agent(sample_agent_config("claude-code-1", Some("cc")))
            .unwrap();
        registry
            .register_agent(sample_agent_config("kiro-cli-1", Some("kiro")))
            .unwrap();

        assert_eq!(
            registry.resolve_by_alias("cc"),
            Some("claude-code-1".to_string())
        );
        assert_eq!(
            registry.resolve_by_alias("kiro"),
            Some("kiro-cli-1".to_string())
        );
        assert_eq!(registry.resolve_by_alias("nonexistent"), None);
    }

    #[test]
    fn test_resolve_by_alias_case_insensitive() {
        let registry = CodingAgentRegistry::new(16);
        registry
            .register_agent(sample_agent_config("agent-1", Some("CC")))
            .unwrap();

        assert_eq!(
            registry.resolve_by_alias("cc"),
            Some("agent-1".to_string())
        );
        assert_eq!(
            registry.resolve_by_alias("CC"),
            Some("agent-1".to_string())
        );
        assert_eq!(
            registry.resolve_by_alias("Cc"),
            Some("agent-1".to_string())
        );
    }

    #[test]
    fn test_update_status_emits_event() {
        let registry = CodingAgentRegistry::new(16);
        registry
            .register_agent(sample_agent_config("agent-1", None))
            .unwrap();

        let mut rx = registry.subscribe_status();

        // Update status from Unknown to Connected
        registry
            .update_status("agent-1", AgentConnectionStatus::Connected)
            .unwrap();

        let event = rx.try_recv().unwrap();
        assert_eq!(event.agent_id, "agent-1");
        assert!(matches!(event.previous_status, AgentConnectionStatus::Unknown));
        assert!(matches!(event.new_status, AgentConnectionStatus::Connected));
    }

    #[test]
    fn test_update_status_no_event_on_same_status() {
        let registry = CodingAgentRegistry::new(16);
        registry
            .register_agent(sample_agent_config("agent-1", None))
            .unwrap();

        // First set to Connected
        registry
            .update_status("agent-1", AgentConnectionStatus::Connected)
            .unwrap();

        let mut rx = registry.subscribe_status();

        // Update to same status — should not emit
        registry
            .update_status("agent-1", AgentConnectionStatus::Connected)
            .unwrap();

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn test_update_status_agent_not_found() {
        let registry = CodingAgentRegistry::new(16);
        let result = registry.update_status("nonexistent", AgentConnectionStatus::Connected);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), CodingAgentError::AgentNotFound(_)));
    }

    #[test]
    fn test_record_successful_task() {
        let registry = CodingAgentRegistry::new(16);
        registry
            .register_agent(sample_agent_config("agent-1", None))
            .unwrap();

        assert!(registry.get_agent("agent-1").unwrap().last_successful_task.is_none());

        registry.record_successful_task("agent-1");

        assert!(registry.get_agent("agent-1").unwrap().last_successful_task.is_some());
    }

    #[test]
    fn test_record_health_check() {
        let registry = CodingAgentRegistry::new(16);
        registry
            .register_agent(sample_agent_config("agent-1", None))
            .unwrap();

        assert!(registry.get_agent("agent-1").unwrap().last_health_check.is_none());

        registry.record_health_check("agent-1");

        assert!(registry.get_agent("agent-1").unwrap().last_health_check.is_some());
    }

    #[test]
    fn test_from_config() {
        let config = CodingAgentsConfig {
            enabled: true,
            max_concurrent_tasks: 3,
            default_timeout_secs: 1800,
            progress_interval_secs: 30,
            agents: vec![
                sample_agent_config("agent-1", Some("cc")),
                sample_agent_config("agent-2", Some("kiro")),
            ],
            backends: vec![sample_backend_definition("claude-code")],
        };

        let registry = CodingAgentRegistry::from_config(&config);
        assert_eq!(registry.agent_count(), 2);
        assert_eq!(registry.backend_count(), 1);
    }

    #[test]
    fn test_reload_from_config_adds_new_agents() {
        let registry = CodingAgentRegistry::new(16);
        registry
            .register_agent(sample_agent_config("existing-agent", None))
            .unwrap();

        let config = CodingAgentsConfig {
            enabled: true,
            max_concurrent_tasks: 3,
            default_timeout_secs: 1800,
            progress_interval_secs: 30,
            agents: vec![
                sample_agent_config("existing-agent", None),
                sample_agent_config("new-agent", Some("new")),
            ],
            backends: vec![sample_backend_definition("claude-code")],
        };

        registry.reload_from_config(&config);

        // Existing agent preserved, new agent added
        assert_eq!(registry.agent_count(), 2);
        assert!(registry.get_agent("existing-agent").is_some());
        assert!(registry.get_agent("new-agent").is_some());
    }

    #[test]
    fn test_reload_from_config_does_not_duplicate_existing() {
        let registry = CodingAgentRegistry::new(16);
        registry
            .register_agent(sample_agent_config("agent-1", None))
            .unwrap();

        // Set a status to verify it's preserved
        registry
            .update_status("agent-1", AgentConnectionStatus::Connected)
            .unwrap();

        let config = CodingAgentsConfig {
            enabled: true,
            max_concurrent_tasks: 3,
            default_timeout_secs: 1800,
            progress_interval_secs: 30,
            agents: vec![sample_agent_config("agent-1", None)],
            backends: vec![],
        };

        registry.reload_from_config(&config);

        // Agent count unchanged, status preserved
        assert_eq!(registry.agent_count(), 1);
        let agent = registry.get_agent("agent-1").unwrap();
        assert!(matches!(agent.status, AgentConnectionStatus::Connected));
    }

    #[test]
    fn test_list_backends() {
        let registry = CodingAgentRegistry::new(16);
        registry.load_backends_from_config(&vec![
            sample_backend_definition("claude-code"),
            sample_backend_definition("kiro-cli"),
        ]);

        let backends = registry.list_backends();
        assert_eq!(backends.len(), 2);
    }

    #[test]
    fn test_debug_impl() {
        let registry = CodingAgentRegistry::new(16);
        let debug_str = format!("{:?}", registry);
        assert!(debug_str.contains("CodingAgentRegistry"));
        assert!(debug_str.contains("agent_count"));
        assert!(debug_str.contains("backend_count"));
    }

    #[test]
    fn test_subscribe_multiple_receivers() {
        let registry = CodingAgentRegistry::new(16);
        registry
            .register_agent(sample_agent_config("agent-1", None))
            .unwrap();

        let mut rx1 = registry.subscribe_status();
        let mut rx2 = registry.subscribe_status();

        registry
            .update_status("agent-1", AgentConnectionStatus::Connected)
            .unwrap();

        // Both receivers should get the event
        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[test]
    fn test_status_sender_accessible() {
        let registry = CodingAgentRegistry::new(16);
        let _sender = registry.status_sender();
        // Just verify it's accessible without panic
    }
}
