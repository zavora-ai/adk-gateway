//! Streaming ACP client for real-time coding agent updates.
//!
//! This module provides a streaming interface to ACP agents, exposing:
//! - Real-time text chunks as they're generated
//! - Tool call notifications (start/complete)
//! - Agent thought/reasoning visibility
//! - Permission request notifications
//! - Status tracking (Starting, Running, WaitingPermission, etc.)
//! - Usage metrics (calls, chars, duration)
//!
//! Unlike the basic `AcpSession` which blocks until completion, this streams
//! updates via channels that can be forwarded to users in real-time.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use adk_acp::{
    AcpAgentConfig, OutputChunk, PermissionPolicy,
    StatusTracker, UsageTracker, AcpUsage, stream_prompt,
    status::AgentStatus,
};
use tokio::sync::mpsc;
use tracing::{info, warn};

use super::config::{AgentTransport, CodingAgentInstanceConfig};
use super::models::{TaskError, TaskRequest, TaskResult};

/// A streaming update from the coding agent.
///
/// These updates are sent in real-time as the agent works, enabling
/// transparent visibility into what the agent is doing.
#[derive(Debug, Clone)]
pub enum CodingAgentUpdate {
    /// Agent status changed (Starting, Running, WaitingPermission, etc.)
    Status(AgentStatusUpdate),
    
    /// Text chunk from the agent's response (stream as it's generated)
    Text(String),
    
    /// Agent is thinking/reasoning (internal monologue)
    Thought(String),
    
    /// Agent started a tool call (e.g., "Reading file src/main.rs")
    ToolCallStarted {
        /// Human-readable description of what the tool is doing
        title: String,
    },
    
    /// Agent completed a tool call
    ToolCallCompleted {
        /// Human-readable description
        title: String,
    },
    
    /// Agent requested permission for an operation
    PermissionRequested {
        /// What the agent wants to do
        title: String,
        /// Whether it was approved by the policy
        approved: bool,
    },
    
    /// Agent finished (success or error)
    Done {
        /// Final text output (complete response)
        output: String,
        /// Duration of the task
        duration: Duration,
        /// Whether it succeeded
        success: bool,
        /// Error message if failed
        error: Option<String>,
    },
}

/// Agent status update with human-readable description.
#[derive(Debug, Clone)]
pub struct AgentStatusUpdate {
    /// The status enum value
    pub status: AgentStatusKind,
    /// Human-readable description
    pub description: String,
}

/// Simplified status enum for external consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatusKind {
    Starting,
    Running,
    WaitingPermission,
    Idle,
    Error,
    Stopped,
}

impl From<AgentStatus> for AgentStatusKind {
    fn from(status: AgentStatus) -> Self {
        match status {
            AgentStatus::Starting => Self::Starting,
            AgentStatus::Running => Self::Running,
            AgentStatus::WaitingPermission => Self::WaitingPermission,
            AgentStatus::Idle => Self::Idle,
            AgentStatus::Error => Self::Error,
            AgentStatus::Stopping | AgentStatus::Stopped => Self::Stopped,
        }
    }
}

/// Receiver for streaming coding agent updates.
pub type CodingAgentUpdateStream = mpsc::Receiver<CodingAgentUpdate>;

/// Streaming ACP session manager.
///
/// Provides streaming task execution with real-time updates forwarded
/// via channels. Integrates with the gateway's delivery system for
/// transparent agent visibility.
pub struct StreamingAcpClient {
    /// Usage tracker for metrics across all invocations
    usage_tracker: UsageTracker,
    /// Permission policy for tool approvals
    policy: Arc<PermissionPolicy>,
}

impl StreamingAcpClient {
    /// Create a new streaming ACP client with auto-approve policy.
    pub fn new() -> Self {
        Self {
            usage_tracker: UsageTracker::new(),
            policy: Arc::new(PermissionPolicy::AutoApprove),
        }
    }
    
    /// Create a streaming ACP client with a custom permission policy.
    pub fn with_policy(policy: PermissionPolicy) -> Self {
        Self {
            usage_tracker: UsageTracker::new(),
            policy: Arc::new(policy),
        }
    }
    
    /// Execute a task with streaming updates.
    ///
    /// Returns a channel receiver that yields `CodingAgentUpdate`s in real-time
    /// as the agent works. The final `Done` update contains the complete result.
    ///
    /// # Arguments
    /// * `agent_id` - The agent identifier (for logging)
    /// * `config` - The agent's configuration (must have stdio transport)
    /// * `request` - The task request
    ///
    /// # Returns
    /// A receiver for streaming updates. Consume all updates until `Done`.
    pub async fn execute_streaming(
        &self,
        agent_id: &str,
        config: &CodingAgentInstanceConfig,
        request: &TaskRequest,
    ) -> Result<CodingAgentUpdateStream, TaskError> {
        let transport = config.transport.as_ref().ok_or_else(|| {
            TaskError::ExecutionError {
                message: format!("Agent '{}' has no transport configured", agent_id),
                partial_output: None,
            }
        })?;

        let AgentTransport::Stdio { command, args, env } = transport else {
            return Err(TaskError::ExecutionError {
                message: format!("Agent '{}' has non-stdio transport — streaming requires stdio", agent_id),
                partial_output: None,
            });
        };

        // Build the command string
        let full_command = if args.is_empty() {
            command.clone()
        } else {
            format!("{} {}", command, args.join(" "))
        };

        let working_dir = request.workspace.clone()
            .or_else(|| config.workspaces.first().cloned())
            .unwrap_or_else(|| PathBuf::from("."));

        // Build ACP config
        let mut acp_config = AcpAgentConfig::new(&full_command)
            .working_dir(&working_dir);

        // Inject env vars
        for (key, val) in env {
            acp_config = acp_config.env(key, val);
        }

        // Build the prompt
        let prompt = build_prompt(request);

        // Create the update channel
        let (update_tx, update_rx) = mpsc::channel::<CodingAgentUpdate>(64);

        // Status tracker for real-time status updates
        let status_tracker = StatusTracker::new();
        
        // Clone what we need for the spawned task
        let policy = self.policy.clone();
        let usage_tracker = self.usage_tracker.clone();
        let agent_id_owned = agent_id.to_string();
        let prompt_len = prompt.len();

        // Spawn the streaming task
        tokio::spawn(async move {
            let start = Instant::now();
            let mut full_output = String::new();
            let mut success = true;
            let mut error_msg: Option<String> = None;

            // Send initial status
            let _ = update_tx.send(CodingAgentUpdate::Status(AgentStatusUpdate {
                status: AgentStatusKind::Starting,
                description: "Starting coding agent...".to_string(),
            })).await;

            // Start the streaming prompt
            match stream_prompt(&acp_config, &prompt, policy, status_tracker.clone()).await {
                Ok(mut stream) => {
                    // Send running status
                    let _ = update_tx.send(CodingAgentUpdate::Status(AgentStatusUpdate {
                        status: AgentStatusKind::Running,
                        description: "Agent is working...".to_string(),
                    })).await;

                    // Process stream chunks
                    while let Some(chunk) = stream.recv().await {
                        match chunk {
                            OutputChunk::Text(text) => {
                                full_output.push_str(&text);
                                let _ = update_tx.send(CodingAgentUpdate::Text(text)).await;
                            }
                            OutputChunk::Thought(thought) => {
                                let _ = update_tx.send(CodingAgentUpdate::Thought(thought)).await;
                            }
                            OutputChunk::ToolCall { title } => {
                                let _ = update_tx.send(CodingAgentUpdate::ToolCallStarted {
                                    title: title.clone(),
                                }).await;
                            }
                            OutputChunk::ToolCallComplete { title } => {
                                let _ = update_tx.send(CodingAgentUpdate::ToolCallCompleted {
                                    title,
                                }).await;
                            }
                            OutputChunk::PermissionRequested { title, approved } => {
                                // Send status update for permission wait
                                if !approved {
                                    let _ = update_tx.send(CodingAgentUpdate::Status(AgentStatusUpdate {
                                        status: AgentStatusKind::WaitingPermission,
                                        description: format!("Permission denied: {}", title),
                                    })).await;
                                }
                                let _ = update_tx.send(CodingAgentUpdate::PermissionRequested {
                                    title,
                                    approved,
                                }).await;
                            }
                            OutputChunk::Done => {
                                break;
                            }
                            OutputChunk::Error(err) => {
                                success = false;
                                error_msg = Some(err.clone());
                                warn!(agent_id = %agent_id_owned, error = %err, "ACP stream error");
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    success = false;
                    error_msg = Some(e.to_string());
                    warn!(agent_id = %agent_id_owned, error = %e, "Failed to start ACP stream");
                }
            }

            let duration = start.elapsed();

            // Record usage
            usage_tracker.record(&AcpUsage {
                tool_name: agent_id_owned.clone(),
                prompt_chars: prompt_len,
                response_chars: full_output.len(),
                duration,
                success,
                permission_requests: 0, // TODO: track from stream
                permissions_denied: 0,
            });

            // Send final done update
            let _ = update_tx.send(CodingAgentUpdate::Done {
                output: full_output,
                duration,
                success,
                error: error_msg,
            }).await;

            info!(
                agent_id = %agent_id_owned,
                duration_ms = duration.as_millis() as u64,
                success = success,
                "Streaming ACP task completed"
            );
        });

        Ok(update_rx)
    }

    /// Execute a task and collect the full result (non-streaming convenience method).
    ///
    /// This is equivalent to consuming all updates from `execute_streaming` and
    /// returning the final result. Use this when you don't need real-time updates.
    pub async fn execute_task(
        &self,
        agent_id: &str,
        config: &CodingAgentInstanceConfig,
        request: &TaskRequest,
    ) -> Result<TaskResult, TaskError> {
        let mut stream = self.execute_streaming(agent_id, config, request).await?;
        
        let mut output = String::new();
        let mut duration = Duration::ZERO;
        let mut error: Option<String> = None;

        while let Some(update) = stream.recv().await {
            match update {
                CodingAgentUpdate::Text(text) => {
                    output.push_str(&text);
                }
                CodingAgentUpdate::Done { output: final_output, duration: d, success, error: e } => {
                    output = final_output;
                    duration = d;
                    if !success {
                        error = e;
                    }
                    break;
                }
                _ => {
                    // Ignore other updates in non-streaming mode
                }
            }
        }

        if let Some(err) = error {
            return Err(TaskError::ExecutionError {
                message: err,
                partial_output: if output.is_empty() { None } else { Some(output) },
            });
        }

        Ok(TaskResult {
            output,
            modified_files: vec![], // ACP doesn't report file changes in stream
            duration_ms: duration.as_millis() as u64,
            token_usage: None,
        })
    }

    /// Get aggregated usage statistics.
    pub fn usage_stats(&self) -> adk_acp::AcpUsageStats {
        self.usage_tracker.stats()
    }

    /// Reset usage statistics.
    pub fn reset_stats(&self) {
        self.usage_tracker.reset();
    }
}

impl Default for StreamingAcpClient {
    fn default() -> Self {
        Self::new()
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

/// Format a coding agent update for human display.
///
/// Returns a short, human-readable string suitable for showing in chat.
pub fn format_update_for_display(update: &CodingAgentUpdate) -> Option<String> {
    match update {
        CodingAgentUpdate::Status(status) => {
            let emoji = match status.status {
                AgentStatusKind::Starting => "🚀",
                AgentStatusKind::Running => "⚙️",
                AgentStatusKind::WaitingPermission => "🔐",
                AgentStatusKind::Idle => "💤",
                AgentStatusKind::Error => "❌",
                AgentStatusKind::Stopped => "🛑",
            };
            Some(format!("{} {}", emoji, status.description))
        }
        CodingAgentUpdate::ToolCallStarted { title } => {
            Some(format!("🔧 {}", title))
        }
        CodingAgentUpdate::ToolCallCompleted { title } => {
            Some(format!("✅ {}", title))
        }
        CodingAgentUpdate::PermissionRequested { title, approved } => {
            if *approved {
                Some(format!("🔓 Approved: {}", title))
            } else {
                Some(format!("🔒 Denied: {}", title))
            }
        }
        CodingAgentUpdate::Done { duration, success, error, .. } => {
            if *success {
                Some(format!("✅ Completed in {:.1}s", duration.as_secs_f64()))
            } else {
                Some(format!("❌ Failed after {:.1}s: {}", 
                    duration.as_secs_f64(),
                    error.as_deref().unwrap_or("unknown error")
                ))
            }
        }
        // Text and Thought are streamed separately, not formatted here
        CodingAgentUpdate::Text(_) | CodingAgentUpdate::Thought(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_update_status() {
        let update = CodingAgentUpdate::Status(AgentStatusUpdate {
            status: AgentStatusKind::Running,
            description: "Agent is working...".to_string(),
        });
        let formatted = format_update_for_display(&update);
        assert!(formatted.is_some());
        let text = formatted.unwrap();
        assert!(text.contains("⚙️"));
    }

    #[test]
    fn test_format_update_tool_call() {
        let update = CodingAgentUpdate::ToolCallStarted {
            title: "Reading file src/main.rs".to_string(),
        };
        let formatted = format_update_for_display(&update);
        assert!(formatted.is_some());
        let text = formatted.unwrap();
        assert!(text.contains("🔧"));
        assert!(text.contains("Reading file"));
    }

    #[test]
    fn test_format_update_done_success() {
        let update = CodingAgentUpdate::Done {
            output: "Done".to_string(),
            duration: Duration::from_secs(5),
            success: true,
            error: None,
        };
        let formatted = format_update_for_display(&update);
        assert!(formatted.is_some());
        let text = formatted.unwrap();
        assert!(text.contains("✅"));
        assert!(text.contains("5.0s"));
    }

    #[test]
    fn test_format_update_done_failure() {
        let update = CodingAgentUpdate::Done {
            output: "".to_string(),
            duration: Duration::from_secs(3),
            success: false,
            error: Some("Connection lost".to_string()),
        };
        let formatted = format_update_for_display(&update);
        assert!(formatted.is_some());
        let text = formatted.unwrap();
        assert!(text.contains("❌"));
        assert!(text.contains("Connection lost"));
    }

    #[test]
    fn test_format_update_text_returns_none() {
        let update = CodingAgentUpdate::Text("Hello".to_string());
        assert!(format_update_for_display(&update).is_none());
    }

    #[test]
    fn test_build_prompt_basic() {
        let request = TaskRequest {
            description: "Fix the bug".to_string(),
            trigger: super::super::models::TaskTrigger::ControlPanel {
                user_id: "test".to_string(),
            },
            workspace: None,
            file_context: None,
            reply_to: super::super::models::ReplyTarget {
                channel_type: "telegram".to_string(),
                channel_id: "123".to_string(),
                message_id: None,
            },
        };
        let prompt = build_prompt(&request);
        assert_eq!(prompt, "Fix the bug");
    }

    #[test]
    fn test_build_prompt_with_workspace() {
        let request = TaskRequest {
            description: "Fix the bug".to_string(),
            trigger: super::super::models::TaskTrigger::ControlPanel {
                user_id: "test".to_string(),
            },
            workspace: Some(PathBuf::from("/home/user/project")),
            file_context: None,
            reply_to: super::super::models::ReplyTarget {
                channel_type: "telegram".to_string(),
                channel_id: "123".to_string(),
                message_id: None,
            },
        };
        let prompt = build_prompt(&request);
        assert!(prompt.contains("Working directory: /home/user/project"));
        assert!(prompt.contains("Fix the bug"));
    }
}
