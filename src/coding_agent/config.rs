//! Configuration models for the coding agent subsystem.
//!
//! Defines all configuration structs used to register and manage external coding agents.
//! These models are deserialized from the gateway configuration file and support
//! hot-reload without restart.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Top-level coding agent configuration added to GatewayConfig.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodingAgentsConfig {
    /// Whether the coding agent subsystem is enabled.
    pub enabled: bool,
    /// Maximum concurrent coding agent tasks (default: 3).
    #[serde(rename = "maxConcurrentTasks")]
    pub max_concurrent_tasks: u32,
    /// Default task timeout in seconds (default: 1800).
    #[serde(rename = "defaultTimeoutSecs")]
    pub default_timeout_secs: u64,
    /// Progress reporting interval in seconds (default: 30).
    #[serde(rename = "progressIntervalSecs")]
    pub progress_interval_secs: u64,
    /// Registered agent instances.
    pub agents: Vec<CodingAgentInstanceConfig>,
    /// Backend definitions (extensible agent types).
    pub backends: Vec<BackendDefinition>,
}

impl Default for CodingAgentsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_concurrent_tasks: 3,
            default_timeout_secs: 1800,
            progress_interval_secs: 30,
            agents: Vec::new(),
            backends: Vec::new(),
        }
    }
}

/// Configuration for a single registered coding agent instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodingAgentInstanceConfig {
    /// Unique identifier for this agent instance.
    pub id: String,
    /// The backend type (references a BackendDefinition).
    #[serde(rename = "backendType")]
    pub backend_type: String,
    /// ACP endpoint URL for this agent.
    pub endpoint: String,
    /// Workspace directories this agent can operate in.
    pub workspaces: Vec<PathBuf>,
    /// Per-agent timeout override in seconds.
    #[serde(rename = "timeoutSecs")]
    pub timeout_secs: Option<u64>,
    /// Per-task cost cap in USD.
    #[serde(rename = "costCapUsd")]
    pub cost_cap_usd: Option<f64>,
    /// Monthly budget alert threshold in USD.
    #[serde(rename = "monthlyBudgetUsd")]
    pub monthly_budget_usd: Option<f64>,
    /// Shorthand alias for delegation commands (e.g., "cc").
    pub alias: Option<String>,
    /// Authentication configuration.
    pub auth: Option<AgentAuthConfig>,
}

/// Extensible backend definition — loaded from config, no code changes needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackendDefinition {
    /// Agent type identifier (e.g., "claude-code", "kiro-cli").
    #[serde(rename = "agentType")]
    pub agent_type: String,
    /// Human-readable display name.
    #[serde(rename = "displayName")]
    pub display_name: String,
    /// CLI command to invoke (e.g., "claude", "kiro").
    #[serde(rename = "cliCommand")]
    pub cli_command: String,
    /// Command to check installation (e.g., "claude --version").
    #[serde(rename = "installCheckCommand")]
    pub install_check_command: String,
    /// Authentication method.
    #[serde(rename = "authMethod")]
    pub auth_method: AuthMethod,
    /// Supported capabilities.
    pub capabilities: AgentCapabilities,
    /// Installation instructions text.
    #[serde(rename = "installInstructions")]
    pub install_instructions: String,
    /// Windows-specific install command (optional, falls back to installInstructions).
    #[serde(default, rename = "installInstructionsWindows")]
    pub install_instructions_windows: Option<String>,
    /// Linux-specific install command (optional, falls back to installInstructions).
    #[serde(default, rename = "installInstructionsLinux")]
    pub install_instructions_linux: Option<String>,
}

/// Authentication methods supported by coding agent backends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AuthMethod {
    /// API key authentication via environment variable.
    ApiKey {
        /// The environment variable name holding the API key.
        env_var: String,
    },
    /// OAuth flow authentication.
    OAuth {
        /// The authorization URL.
        auth_url: String,
        /// The token exchange URL.
        token_url: String,
    },
    /// CLI login command authentication.
    CliLogin {
        /// The CLI command to run for login.
        command: String,
    },
    /// No authentication required.
    None,
}

/// Capabilities declared by a coding agent backend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentCapabilities {
    /// Supports providing file context with tasks.
    #[serde(default, rename = "fileContext")]
    pub file_context: bool,
    /// Supports streaming output during execution.
    #[serde(default, rename = "streamingOutput")]
    pub streaming_output: bool,
    /// Reports cost/token usage in results.
    #[serde(default, rename = "costReporting")]
    pub cost_reporting: bool,
    /// Supports graceful cancellation.
    #[serde(default)]
    pub cancellation: bool,
}

/// Authentication configuration for a coding agent instance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentAuthConfig {
    /// Stored credential value (API key, token, etc.).
    pub credentials: Option<String>,
    /// OAuth or session token.
    pub token: Option<String>,
    /// Token expiration timestamp.
    #[serde(rename = "expiresAt")]
    pub expires_at: Option<DateTime<Utc>>,
}

/// Validates a `BackendDefinition` for required fields.
///
/// Returns a list of missing required field names. An empty vector indicates
/// the definition is valid.
///
/// Required fields: `agent_type`, `cli_command`, `install_check_command`.
pub fn validate_backend_definition(definition: &BackendDefinition) -> Vec<String> {
    let mut missing = Vec::new();

    if definition.agent_type.trim().is_empty() {
        missing.push("agent_type".to_string());
    }
    if definition.cli_command.trim().is_empty() {
        missing.push("cli_command".to_string());
    }
    if definition.install_check_command.trim().is_empty() {
        missing.push("install_check_command".to_string());
    }

    missing
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CodingAgentsConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.max_concurrent_tasks, 3);
        assert_eq!(config.default_timeout_secs, 1800);
        assert_eq!(config.progress_interval_secs, 30);
        assert!(config.agents.is_empty());
        assert!(config.backends.is_empty());
    }

    #[test]
    fn test_validate_backend_definition_valid() {
        let def = BackendDefinition {
            agent_type: "claude-code".to_string(),
            display_name: "Claude Code".to_string(),
            cli_command: "claude".to_string(),
            install_check_command: "claude --version".to_string(),
            auth_method: AuthMethod::ApiKey {
                env_var: "ANTHROPIC_API_KEY".to_string(),
            },
            capabilities: AgentCapabilities::default(),
            install_instructions: "npm install -g @anthropic/claude-code".to_string(),
            install_instructions_windows: None,
            install_instructions_linux: None,
        };

        let missing = validate_backend_definition(&def);
        assert!(missing.is_empty());
    }

    #[test]
    fn test_validate_backend_definition_missing_agent_type() {
        let def = BackendDefinition {
            agent_type: "".to_string(),
            display_name: "Claude Code".to_string(),
            cli_command: "claude".to_string(),
            install_check_command: "claude --version".to_string(),
            auth_method: AuthMethod::None,
            capabilities: AgentCapabilities::default(),
            install_instructions: "".to_string(),
            install_instructions_windows: None,
            install_instructions_linux: None,
        };

        let missing = validate_backend_definition(&def);
        assert_eq!(missing, vec!["agent_type"]);
    }

    #[test]
    fn test_validate_backend_definition_missing_multiple() {
        let def = BackendDefinition {
            agent_type: "  ".to_string(),
            display_name: "Test".to_string(),
            cli_command: "".to_string(),
            install_check_command: "   ".to_string(),
            auth_method: AuthMethod::None,
            capabilities: AgentCapabilities::default(),
            install_instructions: "".to_string(),
            install_instructions_windows: None,
            install_instructions_linux: None,
        };

        let missing = validate_backend_definition(&def);
        assert_eq!(missing.len(), 3);
        assert!(missing.contains(&"agent_type".to_string()));
        assert!(missing.contains(&"cli_command".to_string()));
        assert!(missing.contains(&"install_check_command".to_string()));
    }

    #[test]
    fn test_auth_method_serialization() {
        let api_key = AuthMethod::ApiKey {
            env_var: "MY_KEY".to_string(),
        };
        let json = serde_json::to_string(&api_key).unwrap();
        assert!(json.contains("\"type\":\"apiKey\""));
        assert!(json.contains("\"env_var\":\"MY_KEY\""));

        let none = AuthMethod::None;
        let json = serde_json::to_string(&none).unwrap();
        assert!(json.contains("\"type\":\"none\""));
    }

    #[test]
    fn test_auth_method_deserialization() {
        let json = r#"{"type":"apiKey","env_var":"ANTHROPIC_API_KEY"}"#;
        let method: AuthMethod = serde_json::from_str(json).unwrap();
        match method {
            AuthMethod::ApiKey { env_var } => assert_eq!(env_var, "ANTHROPIC_API_KEY"),
            _ => panic!("Expected ApiKey variant"),
        }

        let json = r#"{"type":"oAuth","auth_url":"https://auth.example.com","token_url":"https://token.example.com"}"#;
        let method: AuthMethod = serde_json::from_str(json).unwrap();
        match method {
            AuthMethod::OAuth {
                auth_url,
                token_url,
            } => {
                assert_eq!(auth_url, "https://auth.example.com");
                assert_eq!(token_url, "https://token.example.com");
            }
            _ => panic!("Expected OAuth variant"),
        }

        let json = r#"{"type":"cliLogin","command":"claude login"}"#;
        let method: AuthMethod = serde_json::from_str(json).unwrap();
        match method {
            AuthMethod::CliLogin { command } => assert_eq!(command, "claude login"),
            _ => panic!("Expected CliLogin variant"),
        }

        let json = r#"{"type":"none"}"#;
        let method: AuthMethod = serde_json::from_str(json).unwrap();
        assert!(matches!(method, AuthMethod::None));
    }

    #[test]
    fn test_agent_capabilities_defaults() {
        let caps = AgentCapabilities::default();
        assert!(!caps.file_context);
        assert!(!caps.streaming_output);
        assert!(!caps.cost_reporting);
        assert!(!caps.cancellation);
    }

    #[test]
    fn test_coding_agents_config_serde_roundtrip() {
        let config = CodingAgentsConfig {
            enabled: true,
            max_concurrent_tasks: 5,
            default_timeout_secs: 3600,
            progress_interval_secs: 60,
            agents: vec![CodingAgentInstanceConfig {
                id: "test-agent".to_string(),
                backend_type: "claude-code".to_string(),
                endpoint: "http://localhost:8080".to_string(),
                workspaces: vec![PathBuf::from("/home/user/projects")],
                timeout_secs: Some(900),
                cost_cap_usd: Some(5.0),
                monthly_budget_usd: Some(100.0),
                alias: Some("cc".to_string()),
                auth: Some(AgentAuthConfig {
                    credentials: Some("sk-test-key".to_string()),
                    token: None,
                    expires_at: None,
                }),
            }],
            backends: vec![],
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: CodingAgentsConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.enabled, config.enabled);
        assert_eq!(deserialized.max_concurrent_tasks, config.max_concurrent_tasks);
        assert_eq!(deserialized.default_timeout_secs, config.default_timeout_secs);
        assert_eq!(deserialized.progress_interval_secs, config.progress_interval_secs);
        assert_eq!(deserialized.agents.len(), 1);
        assert_eq!(deserialized.agents[0].id, "test-agent");
        assert_eq!(deserialized.agents[0].alias, Some("cc".to_string()));
    }

    #[test]
    fn test_config_deserialize_with_defaults() {
        let json = r#"{}"#;
        let config: CodingAgentsConfig = serde_json::from_str(json).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.max_concurrent_tasks, 3);
        assert_eq!(config.default_timeout_secs, 1800);
        assert_eq!(config.progress_interval_secs, 30);
    }
}
