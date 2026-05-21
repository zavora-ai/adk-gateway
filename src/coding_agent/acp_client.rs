//! ACP client integration using the `adk-acp` crate.
//!
//! Wraps `adk_acp::AcpSession` for persistent connections to coding agent CLIs.
//! The gateway maintains one session per registered agent, reusing it across
//! multiple task delegations for context preservation and lower latency.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use adk_acp::{AcpAgentConfig, AcpSession, PermissionPolicy};
use dashmap::DashMap;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use super::config::{AgentTransport, CodingAgentInstanceConfig};
use super::models::{TaskError, TaskRequest, TaskResult};
use super::registry::CodingAgentRegistry;
use super::status::AgentConnectionStatus;

/// Manages persistent ACP sessions for registered coding agents.
///
/// Each agent with a stdio transport gets a long-lived `AcpSession` that
/// preserves context across multiple prompts. Sessions are lazily created
/// on first task delegation and restarted on failure.
pub struct AcpSessionPool {
    /// Active sessions keyed by agent ID.
    sessions: DashMap<String, Arc<Mutex<AcpSession>>>,
    /// Registry reference for status updates.
    registry: Arc<CodingAgentRegistry>,
    /// Optional HITL permission manager for routing approvals to Telegram.
    permission_manager: Option<Arc<super::hitl_permissions::HitlPermissionManager>>,
}

impl AcpSessionPool {
    /// Create a new session pool.
    pub fn new(registry: Arc<CodingAgentRegistry>) -> Self {
        Self {
            sessions: DashMap::new(),
            registry,
            permission_manager: None,
        }
    }

    /// Create a session pool with HITL permission handling.
    pub fn with_hitl(
        registry: Arc<CodingAgentRegistry>,
        permission_manager: Arc<super::hitl_permissions::HitlPermissionManager>,
    ) -> Self {
        Self {
            sessions: DashMap::new(),
            registry,
            permission_manager: Some(permission_manager),
        }
    }

    /// Get or create a session for the given agent.
    pub async fn get_or_create(
        &self,
        agent_id: &str,
        config: &CodingAgentInstanceConfig,
    ) -> Result<Arc<Mutex<AcpSession>>, TaskError> {
        // Return existing session if available
        if let Some(session) = self.sessions.get(agent_id) {
            let s = session.value().clone();
            let guard = s.lock().await;
            if guard.is_active() {
                drop(guard);
                return Ok(s);
            }
            // Session died — remove and recreate
            drop(guard);
            self.sessions.remove(agent_id);
        }

        // Create new session
        let session = self.create_session(agent_id, config).await?;
        let arc_session = Arc::new(Mutex::new(session));
        self.sessions.insert(agent_id.to_string(), arc_session.clone());
        Ok(arc_session)
    }

    /// Create a new ACP session for an agent.
    async fn create_session(
        &self,
        agent_id: &str,
        config: &CodingAgentInstanceConfig,
    ) -> Result<AcpSession, TaskError> {
        let transport = config.transport.as_ref().ok_or_else(|| {
            TaskError::ExecutionError {
                message: format!("Agent '{}' has no transport configured", agent_id),
                partial_output: None,
            }
        })?;

        let AgentTransport::Stdio { command, args, env } = transport else {
            return Err(TaskError::ExecutionError {
                message: format!("Agent '{}' has non-stdio transport — use HTTP executor", agent_id),
                partial_output: None,
            });
        };

        // Build the command string (command + args joined)
        let full_command = if args.is_empty() {
            command.clone()
        } else {
            format!("{} {}", command, args.join(" "))
        };

        // Build ACP config with env vars
        let working_dir = config.workspaces.first()
            .cloned()
            .unwrap_or_else(|| PathBuf::from("."));

        let mut acp_config = AcpAgentConfig::new(&full_command)
            .working_dir(&working_dir)
            .auto_approve(true);

        // Inject env vars (API keys, etc.)
        for (key, val) in env {
            acp_config = acp_config.env(key, val);
        }

        // Also inject auth credentials as env var if configured
        if let Some(auth) = &config.auth {
            if let Some(creds) = &auth.credentials {
                // Try to determine the env var name from the backend definition
                // For now, use a generic approach
                acp_config = acp_config.env("AGENT_API_KEY", creds);
            }
        }

        info!(agent_id = %agent_id, command = %full_command, "creating ACP session");

        let policy = if let Some(ref pm) = self.permission_manager {
            pm.build_policy()
        } else {
            PermissionPolicy::AutoApprove
        };
        let policy = Arc::new(policy);
        let session = AcpSession::start(acp_config, policy).await.map_err(|e| {
            error!(agent_id = %agent_id, error = %e, "failed to start ACP session");
            TaskError::AgentDisconnected {
                agent_id: agent_id.to_string(),
            }
        })?;

        // Update status to connected
        let _ = self.registry.update_status(agent_id, AgentConnectionStatus::Connected);
        info!(agent_id = %agent_id, "ACP session established");

        Ok(session)
    }

    /// Execute a task against an agent using its ACP session.
    pub async fn execute_task(
        &self,
        agent_id: &str,
        config: &CodingAgentInstanceConfig,
        request: &TaskRequest,
    ) -> Result<TaskResult, TaskError> {
        let session_arc = self.get_or_create(agent_id, config).await?;
        let mut session = session_arc.lock().await;

        // Build the prompt from the task request
        let prompt = build_prompt(request);

        let start = std::time::Instant::now();
        let result = session.prompt(&prompt).await;
        let duration = start.elapsed();

        match result {
            Ok(prompt_result) => {
                info!(
                    agent_id = %agent_id,
                    duration_ms = duration.as_millis() as u64,
                    response_len = prompt_result.text.len(),
                    "ACP task completed"
                );

                Ok(TaskResult {
                    output: prompt_result.text,
                    modified_files: vec![], // ACP doesn't report file changes in text response
                    duration_ms: duration.as_millis() as u64,
                    token_usage: None, // ACP doesn't report token usage directly
                })
            }
            Err(e) => {
                warn!(agent_id = %agent_id, error = %e, "ACP task failed");

                // Session might be dead — remove it so next call recreates
                drop(session);
                self.sessions.remove(agent_id);
                let _ = self.registry.update_status(
                    agent_id,
                    AgentConnectionStatus::Disconnected { since: chrono::Utc::now() },
                );

                Err(TaskError::ExecutionError {
                    message: format!("ACP error: {}", e),
                    partial_output: None,
                })
            }
        }
    }

    /// Cancel an in-progress task for an agent.
    pub async fn cancel_task(&self, agent_id: &str) -> Result<(), TaskError> {
        let Some(session_arc) = self.sessions.get(agent_id).map(|s| s.value().clone()) else {
            return Err(TaskError::ExecutionError {
                message: format!("No active session for agent '{}'", agent_id),
                partial_output: None,
            });
        };

        let mut session = session_arc.lock().await;
        session.cancel().await.map_err(|e| {
            TaskError::ExecutionError {
                message: format!("Cancel failed: {}", e),
                partial_output: None,
            }
        })?;

        info!(agent_id = %agent_id, "ACP task cancelled");
        Ok(())
    }

    /// Close all sessions (for shutdown).
    pub async fn close_all(&self) {
        for entry in self.sessions.iter() {
            let agent_id = entry.key().clone();
            let session = entry.value().clone();
            let mut s = session.lock().await;
            if let Err(e) = s.close().await {
                warn!(agent_id = %agent_id, error = %e, "error closing ACP session");
            }
        }
        self.sessions.clear();
    }

    /// Check if an agent has an active session.
    pub fn has_session(&self, agent_id: &str) -> bool {
        self.sessions.contains_key(agent_id)
    }

    /// Close a specific agent's session.
    pub async fn close_session(&self, agent_id: &str) {
        if let Some((_, session)) = self.sessions.remove(agent_id) {
            let mut s = session.lock().await;
            if let Err(e) = s.close().await {
                tracing::warn!(agent_id = %agent_id, error = %e, "error closing ACP session");
            }
        }
    }
}

/// Build a prompt string from a TaskRequest.
fn build_prompt(request: &TaskRequest) -> String {
    let mut prompt = request.description.clone();

    if let Some(workspace) = &request.workspace {
        prompt = format!("Working directory: {}\n\n{}", workspace.display(), prompt);
    }

    if let Some(files) = &request.file_context {
        if !files.is_empty() {
            let file_list: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
            prompt = format!("{}\n\nRelevant files:\n{}", prompt, file_list.join("\n"));
        }
    }

    prompt
}
