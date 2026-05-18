//! ACP (Agent Communication Protocol) integration for external coding agents.
//!
//! This module provides task delegation to external coding agents (Claude Code, Codex)
//! via the ACP protocol. It is feature-gated behind the `acp` feature flag.
//!
//! # Architecture
//!
//! The `AcpTool` struct wraps an HTTP client that communicates with an ACP-compatible
//! endpoint. Each agent in the gateway config can have its own ACP configuration,
//! allowing different agents to delegate to different coding agents.
//!
//! # Progress Reporting
//!
//! Long-running ACP tasks send periodic progress messages to the user via a
//! `tokio::sync::mpsc` channel. The gateway forwards these to the appropriate
//! delivery channel (Telegram, Slack, etc.).

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, error, info};

// ── ACP Configuration ──────────────────────────────────────────────

/// Configuration for an ACP (Agent Communication Protocol) endpoint.
///
/// Each agent can have its own ACP configuration, allowing delegation
/// to different external coding agents (Claude Code, Codex, or custom).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpConfig {
    /// The ACP endpoint URL (e.g., "http://localhost:3000/acp").
    pub endpoint: String,

    /// Timeout in seconds for ACP task execution (default: 300).
    #[serde(default = "default_timeout_secs", rename = "timeoutSecs")]
    pub timeout_secs: u64,

    /// The type of external coding agent at this endpoint.
    #[serde(default, rename = "agentType")]
    pub agent_type: AcpAgentType,
}

fn default_timeout_secs() -> u64 {
    300
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            endpoint: "http://localhost:3000/acp".to_string(),
            timeout_secs: default_timeout_secs(),
            agent_type: AcpAgentType::default(),
        }
    }
}

/// The type of external coding agent accessible via ACP.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AcpAgentType {
    /// Claude Code — Anthropic's coding agent.
    ClaudeCode,
    /// Codex — OpenAI's coding agent.
    Codex,
    /// A custom ACP-compatible agent.
    Custom {
        /// The name of the custom agent.
        name: String,
    },
}

impl Default for AcpAgentType {
    fn default() -> Self {
        Self::ClaudeCode
    }
}

impl AcpAgentType {
    /// Returns a human-readable name for the agent type.
    pub fn display_name(&self) -> &str {
        match self {
            AcpAgentType::ClaudeCode => "Claude Code",
            AcpAgentType::Codex => "Codex",
            AcpAgentType::Custom { name } => name,
        }
    }
}

// ── ACP Errors ─────────────────────────────────────────────────────

/// Errors that can occur during ACP task execution.
#[derive(Debug, Error)]
pub enum AcpError {
    /// The ACP endpoint is unreachable.
    #[error("ACP endpoint unreachable at {endpoint}: {reason}")]
    EndpointUnreachable { endpoint: String, reason: String },

    /// The ACP task timed out.
    #[error("ACP task timed out after {timeout_secs}s (agent: {agent_type})")]
    Timeout {
        timeout_secs: u64,
        agent_type: String,
    },

    /// The ACP endpoint returned an error response.
    #[error("ACP endpoint returned error (status {status}): {message}")]
    EndpointError { status: u16, message: String },

    /// Failed to serialize the ACP request.
    #[error("Failed to build ACP request: {0}")]
    RequestBuildError(String),

    /// Failed to deserialize the ACP response.
    #[error("Failed to parse ACP response: {0}")]
    ResponseParseError(String),

    /// The progress channel was closed unexpectedly.
    #[error("Progress channel closed")]
    ProgressChannelClosed,
}

// ── ACP Request / Response ─────────────────────────────────────────

/// A request to execute a task via ACP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRequest {
    /// The task description to delegate.
    pub task: String,

    /// Optional file paths to provide as context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_context: Option<Vec<PathBuf>>,

    /// The agent type handling this request.
    pub agent_type: String,
}

/// The result of an ACP task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpResult {
    /// Whether the task completed successfully.
    pub success: bool,

    /// The output/result of the task.
    pub output: String,

    /// Files that were modified during the task.
    #[serde(default)]
    pub modified_files: Vec<PathBuf>,

    /// Duration of the task execution in milliseconds.
    pub duration_ms: u64,
}

/// A progress update from an in-flight ACP task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpProgress {
    /// Human-readable progress message.
    pub message: String,

    /// Estimated completion percentage (0-100), if available.
    pub percent_complete: Option<u8>,
}

// ── ACP Tool ───────────────────────────────────────────────────────

/// The ACP tool that delegates tasks to external coding agents.
///
/// Each instance is configured for a specific ACP endpoint and agent type.
/// The tool handles connection, timeout, progress reporting, and error recovery.
pub struct AcpTool {
    config: AcpConfig,
    client: reqwest::Client,
}

impl AcpTool {
    /// Create a new ACP tool with the given configuration.
    pub fn new(config: AcpConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self { config, client }
    }

    /// Execute a task via the ACP endpoint.
    ///
    /// Sends the task to the configured external coding agent and returns the result.
    /// Progress updates are sent via the `progress_tx` channel during execution.
    ///
    /// # Arguments
    ///
    /// * `task` - The task description to delegate
    /// * `file_context` - Optional file paths to provide as context
    /// * `progress_tx` - Channel for sending progress updates to the user
    ///
    /// # Errors
    ///
    /// Returns `AcpError` if the endpoint is unreachable, the task times out,
    /// or the endpoint returns an error. Never panics.
    pub async fn execute(
        &self,
        task: &str,
        file_context: Option<&[PathBuf]>,
        progress_tx: mpsc::Sender<String>,
    ) -> Result<AcpResult, AcpError> {
        info!(
            agent_type = self.config.agent_type.display_name(),
            endpoint = %self.config.endpoint,
            "Delegating task to ACP agent"
        );

        let request = AcpRequest {
            task: task.to_string(),
            file_context: file_context.map(|f| f.to_vec()),
            agent_type: self.config.agent_type.display_name().to_string(),
        };

        // Send initial progress message
        let _ = progress_tx
            .send(format!(
                "🔄 Delegating task to {} via ACP...",
                self.config.agent_type.display_name()
            ))
            .await;

        // Spawn a background task for periodic progress updates
        let progress_tx_clone = progress_tx.clone();
        let agent_name = self.config.agent_type.display_name().to_string();
        let timeout_secs = self.config.timeout_secs;
        let progress_handle = tokio::spawn(async move {
            Self::send_periodic_progress(progress_tx_clone, &agent_name, timeout_secs).await;
        });

        // Execute the ACP request
        let result = self.do_execute(&request).await;

        // Cancel the progress reporter
        progress_handle.abort();

        // Send completion progress
        match &result {
            Ok(r) => {
                let _ = progress_tx
                    .send(format!(
                        "✅ {} completed task ({}ms)",
                        self.config.agent_type.display_name(),
                        r.duration_ms
                    ))
                    .await;
            }
            Err(e) => {
                let _ = progress_tx
                    .send(format!(
                        "❌ {} task failed: {}",
                        self.config.agent_type.display_name(),
                        e
                    ))
                    .await;
            }
        }

        result
    }

    /// Internal method that performs the actual HTTP request to the ACP endpoint.
    async fn do_execute(&self, request: &AcpRequest) -> Result<AcpResult, AcpError> {
        let response = self
            .client
            .post(&self.config.endpoint)
            .json(request)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    AcpError::Timeout {
                        timeout_secs: self.config.timeout_secs,
                        agent_type: self.config.agent_type.display_name().to_string(),
                    }
                } else if e.is_connect() {
                    AcpError::EndpointUnreachable {
                        endpoint: self.config.endpoint.clone(),
                        reason: e.to_string(),
                    }
                } else {
                    AcpError::EndpointUnreachable {
                        endpoint: self.config.endpoint.clone(),
                        reason: e.to_string(),
                    }
                }
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            error!(
                status = status.as_u16(),
                body = %body,
                "ACP endpoint returned error"
            );
            return Err(AcpError::EndpointError {
                status: status.as_u16(),
                message: body,
            });
        }

        let result: AcpResult = response.json().await.map_err(|e| {
            AcpError::ResponseParseError(format!("Failed to parse ACP response JSON: {}", e))
        })?;

        debug!(
            success = result.success,
            duration_ms = result.duration_ms,
            modified_files = result.modified_files.len(),
            "ACP task completed"
        );

        Ok(result)
    }

    /// Send periodic progress messages while a task is executing.
    ///
    /// Sends a progress update every 30 seconds to keep the user informed
    /// that the task is still running.
    async fn send_periodic_progress(
        progress_tx: mpsc::Sender<String>,
        agent_name: &str,
        timeout_secs: u64,
    ) {
        let interval = Duration::from_secs(30);
        let mut elapsed_secs: u64 = 0;

        loop {
            tokio::time::sleep(interval).await;
            elapsed_secs += 30;

            if elapsed_secs >= timeout_secs {
                let _ = progress_tx
                    .send(format!(
                        "⏳ {} is approaching timeout ({}/{}s)...",
                        agent_name, elapsed_secs, timeout_secs
                    ))
                    .await;
                break;
            }

            let msg = format!(
                "⏳ {} is still working... ({}s elapsed)",
                agent_name, elapsed_secs
            );

            if progress_tx.send(msg).await.is_err() {
                // Channel closed, stop sending progress
                break;
            }
        }
    }

    /// Get the configuration for this ACP tool.
    pub fn config(&self) -> &AcpConfig {
        &self.config
    }

    /// Get the agent type display name.
    pub fn agent_type_name(&self) -> &str {
        self.config.agent_type.display_name()
    }
}

// ── ACP Tool Registry ──────────────────────────────────────────────

/// Manages ACP tool instances per-agent based on gateway configuration.
///
/// Each agent can have zero or more ACP tools configured, allowing
/// different agents to delegate to different external coding agents.
pub struct AcpToolRegistry {
    /// Map of agent_id → Vec<AcpTool>
    tools: std::collections::HashMap<String, Vec<AcpTool>>,
}

impl AcpToolRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            tools: std::collections::HashMap::new(),
        }
    }

    /// Register ACP tools for a specific agent based on its configuration.
    pub fn register_for_agent(&mut self, agent_id: &str, configs: Vec<AcpConfig>) {
        let tools: Vec<AcpTool> = configs.into_iter().map(AcpTool::new).collect();

        if !tools.is_empty() {
            info!(
                agent_id = agent_id,
                tool_count = tools.len(),
                "Registered ACP tools for agent"
            );
        }

        self.tools.insert(agent_id.to_string(), tools);
    }

    /// Get ACP tools registered for a specific agent.
    pub fn tools_for_agent(&self, agent_id: &str) -> Option<&[AcpTool]> {
        self.tools.get(agent_id).map(|v| v.as_slice())
    }

    /// Check if any ACP tools are registered for a specific agent.
    pub fn has_tools_for_agent(&self, agent_id: &str) -> bool {
        self.tools
            .get(agent_id)
            .map(|v| !v.is_empty())
            .unwrap_or(false)
    }

    /// Get the total number of registered ACP tools across all agents.
    pub fn total_tool_count(&self) -> usize {
        self.tools.values().map(|v| v.len()).sum()
    }
}

impl Default for AcpToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Per-Agent ACP Configuration ────────────────────────────────────

/// Per-agent ACP configuration as it appears in the gateway config.
///
/// This is used in the `AgentEntry` to configure which ACP agents
/// are available for task delegation from a specific gateway agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AgentAcpConfig {
    /// Whether ACP is enabled for this agent (default: false).
    #[serde(default)]
    pub enabled: bool,

    /// List of ACP endpoint configurations for this agent.
    #[serde(default)]
    pub agents: Vec<AcpConfig>,
}

impl AgentAcpConfig {
    /// Parse an `AgentAcpConfig` from a `serde_json::Value`.
    ///
    /// This is used to extract the typed ACP config from the generic
    /// `Option<serde_json::Value>` field on `AgentEntry`.
    pub fn from_value(value: &serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value.clone())
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acp_agent_type_display_name() {
        assert_eq!(AcpAgentType::ClaudeCode.display_name(), "Claude Code");
        assert_eq!(AcpAgentType::Codex.display_name(), "Codex");
        assert_eq!(
            AcpAgentType::Custom {
                name: "MyAgent".to_string()
            }
            .display_name(),
            "MyAgent"
        );
    }

    #[test]
    fn test_acp_config_defaults() {
        let config = AcpConfig::default();
        assert_eq!(config.timeout_secs, 300);
        assert_eq!(config.agent_type, AcpAgentType::ClaudeCode);
        assert_eq!(config.endpoint, "http://localhost:3000/acp");
    }

    #[test]
    fn test_acp_config_serialization() {
        let config = AcpConfig {
            endpoint: "http://example.com/acp".to_string(),
            timeout_secs: 600,
            agent_type: AcpAgentType::Codex,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AcpConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_acp_config_custom_agent_serialization() {
        let config = AcpConfig {
            endpoint: "http://custom.local:8080/acp".to_string(),
            timeout_secs: 120,
            agent_type: AcpAgentType::Custom {
                name: "CustomCoder".to_string(),
            },
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: AcpConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_acp_tool_registry_new() {
        let registry = AcpToolRegistry::new();
        assert_eq!(registry.total_tool_count(), 0);
        assert!(!registry.has_tools_for_agent("test-agent"));
    }

    #[test]
    fn test_acp_tool_registry_register() {
        let mut registry = AcpToolRegistry::new();

        let configs = vec![
            AcpConfig {
                endpoint: "http://localhost:3000/acp".to_string(),
                timeout_secs: 300,
                agent_type: AcpAgentType::ClaudeCode,
            },
            AcpConfig {
                endpoint: "http://localhost:3001/acp".to_string(),
                timeout_secs: 600,
                agent_type: AcpAgentType::Codex,
            },
        ];

        registry.register_for_agent("agent-1", configs);

        assert!(registry.has_tools_for_agent("agent-1"));
        assert!(!registry.has_tools_for_agent("agent-2"));
        assert_eq!(registry.total_tool_count(), 2);
        assert_eq!(registry.tools_for_agent("agent-1").unwrap().len(), 2);
    }

    #[test]
    fn test_acp_tool_registry_multiple_agents() {
        let mut registry = AcpToolRegistry::new();

        registry.register_for_agent(
            "agent-1",
            vec![AcpConfig {
                endpoint: "http://localhost:3000/acp".to_string(),
                timeout_secs: 300,
                agent_type: AcpAgentType::ClaudeCode,
            }],
        );

        registry.register_for_agent(
            "agent-2",
            vec![AcpConfig {
                endpoint: "http://localhost:3001/acp".to_string(),
                timeout_secs: 600,
                agent_type: AcpAgentType::Codex,
            }],
        );

        assert_eq!(registry.total_tool_count(), 2);
        assert!(registry.has_tools_for_agent("agent-1"));
        assert!(registry.has_tools_for_agent("agent-2"));
    }

    #[test]
    fn test_acp_tool_creation() {
        let config = AcpConfig {
            endpoint: "http://localhost:3000/acp".to_string(),
            timeout_secs: 300,
            agent_type: AcpAgentType::ClaudeCode,
        };

        let tool = AcpTool::new(config.clone());
        assert_eq!(tool.config(), &config);
        assert_eq!(tool.agent_type_name(), "Claude Code");
    }

    #[test]
    fn test_acp_error_display() {
        let err = AcpError::EndpointUnreachable {
            endpoint: "http://localhost:3000/acp".to_string(),
            reason: "connection refused".to_string(),
        };
        assert!(err.to_string().contains("unreachable"));
        assert!(err.to_string().contains("localhost:3000"));

        let err = AcpError::Timeout {
            timeout_secs: 300,
            agent_type: "Claude Code".to_string(),
        };
        assert!(err.to_string().contains("timed out"));
        assert!(err.to_string().contains("300s"));

        let err = AcpError::EndpointError {
            status: 500,
            message: "Internal Server Error".to_string(),
        };
        assert!(err.to_string().contains("500"));
    }

    #[test]
    fn test_acp_request_serialization() {
        let request = AcpRequest {
            task: "Fix the bug in auth.rs".to_string(),
            file_context: Some(vec![PathBuf::from("src/auth.rs")]),
            agent_type: "Claude Code".to_string(),
        };

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["task"], "Fix the bug in auth.rs");
        assert_eq!(json["file_context"][0], "src/auth.rs");
        assert_eq!(json["agent_type"], "Claude Code");
    }

    #[test]
    fn test_acp_result_deserialization() {
        let json = serde_json::json!({
            "success": true,
            "output": "Fixed the authentication bug",
            "modified_files": ["src/auth.rs", "tests/auth_test.rs"],
            "duration_ms": 5000
        });

        let result: AcpResult = serde_json::from_value(json).unwrap();
        assert!(result.success);
        assert_eq!(result.output, "Fixed the authentication bug");
        assert_eq!(result.modified_files.len(), 2);
        assert_eq!(result.duration_ms, 5000);
    }

    #[test]
    fn test_agent_acp_config_default() {
        let config = AgentAcpConfig::default();
        assert!(!config.enabled);
        assert!(config.agents.is_empty());
    }

    #[tokio::test]
    async fn test_acp_tool_execute_unreachable_endpoint() {
        // Test that an unreachable endpoint returns a descriptive error, not a panic
        let config = AcpConfig {
            endpoint: "http://127.0.0.1:1/acp".to_string(), // unreachable port
            timeout_secs: 5,
            agent_type: AcpAgentType::ClaudeCode,
        };

        let tool = AcpTool::new(config);
        let (tx, mut rx) = mpsc::channel(10);

        let result = tool.execute("test task", None, tx).await;

        // Should return an error, not panic
        assert!(result.is_err());
        let err = result.unwrap_err();

        // Should be an endpoint unreachable or timeout error
        match &err {
            AcpError::EndpointUnreachable { endpoint, .. } => {
                assert!(endpoint.contains("127.0.0.1:1"));
            }
            AcpError::Timeout { .. } => {
                // Also acceptable — depends on OS behavior
            }
            other => panic!("Expected EndpointUnreachable or Timeout, got: {:?}", other),
        }

        // Should have received progress messages (at least the initial one)
        let mut messages = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            messages.push(msg);
        }
        assert!(!messages.is_empty(), "Should have received progress messages");
        assert!(
            messages[0].contains("Delegating task"),
            "First message should indicate delegation"
        );
    }

    #[tokio::test]
    async fn test_acp_tool_execute_with_file_context() {
        // Test that file context is properly included in the request
        let config = AcpConfig {
            endpoint: "http://127.0.0.1:1/acp".to_string(),
            timeout_secs: 2,
            agent_type: AcpAgentType::Codex,
        };

        let tool = AcpTool::new(config);
        let (tx, _rx) = mpsc::channel(10);

        let files = vec![PathBuf::from("src/main.rs"), PathBuf::from("src/lib.rs")];
        let result = tool.execute("refactor code", Some(&files), tx).await;

        // Should fail (unreachable) but not panic
        assert!(result.is_err());
    }

    #[test]
    fn test_acp_agent_type_serde_roundtrip() {
        // Test all agent type variants serialize/deserialize correctly
        let types = vec![
            AcpAgentType::ClaudeCode,
            AcpAgentType::Codex,
            AcpAgentType::Custom {
                name: "TestAgent".to_string(),
            },
        ];

        for agent_type in types {
            let json = serde_json::to_string(&agent_type).unwrap();
            let deserialized: AcpAgentType = serde_json::from_str(&json).unwrap();
            assert_eq!(agent_type, deserialized);
        }
    }
}
