//! Task state, result, and request models for coding agent operations.
//!
//! Defines the core data types used throughout the coding agent subsystem:
//! task requests, execution states, results, errors, and history entries.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Unique task identifier.
pub type TaskId = String;

/// A request to execute a coding task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRequest {
    /// The task description/prompt.
    pub description: String,
    /// The trigger source.
    pub trigger: TaskTrigger,
    /// Optional workspace override.
    pub workspace: Option<PathBuf>,
    /// Optional file context paths.
    pub file_context: Option<Vec<PathBuf>>,
    /// The user/channel to deliver results to.
    pub reply_to: ReplyTarget,
}

/// How the task was triggered.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TaskTrigger {
    /// User command from a messaging channel.
    UserCommand { user_id: String, channel: String },
    /// Scheduled cron job.
    CronJob { job_id: String },
    /// Inter-agent delegation.
    AgentDelegation { source_agent_id: String },
    /// Control panel UI action.
    ControlPanel { user_id: String },
}

/// Current state of a task in the execution pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum TaskState {
    /// Waiting in queue for an execution slot.
    Queued { queued_at: DateTime<Utc> },
    /// Currently executing.
    Running {
        started_at: DateTime<Utc>,
        progress_percent: Option<u8>,
    },
    /// Completed successfully.
    Completed {
        started_at: DateTime<Utc>,
        completed_at: DateTime<Utc>,
        result: TaskResult,
    },
    /// Failed during execution.
    Failed {
        started_at: DateTime<Utc>,
        failed_at: DateTime<Utc>,
        error: TaskError,
    },
    /// Cancelled by user or system.
    Cancelled {
        started_at: Option<DateTime<Utc>>,
        cancelled_at: DateTime<Utc>,
        reason: CancellationReason,
    },
}

/// Successful task result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    /// The output produced by the coding agent.
    pub output: String,
    /// Files modified during task execution.
    pub modified_files: Vec<FileChange>,
    /// Total task duration in milliseconds.
    pub duration_ms: u64,
    /// Token usage reported by the coding agent (if available).
    pub token_usage: Option<TokenUsage>,
}

/// A file change produced by a coding agent task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    /// Path to the changed file.
    pub path: PathBuf,
    /// Type of change (added, modified, deleted).
    pub change_type: FileChangeType,
    /// Number of lines added.
    pub lines_added: u32,
    /// Number of lines removed.
    pub lines_removed: u32,
}

/// The type of file change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileChangeType {
    Added,
    Modified,
    Deleted,
}

/// Token usage reported by the coding agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    /// Number of input tokens consumed.
    pub input_tokens: u64,
    /// Number of output tokens generated.
    pub output_tokens: u64,
    /// Estimated cost in USD.
    pub estimated_cost_usd: f64,
}

/// Task error categories.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "category", rename_all = "camelCase")]
pub enum TaskError {
    /// Task exceeded its configured timeout.
    Timeout { elapsed_secs: u64, limit_secs: u64 },
    /// Task exceeded its cost cap.
    CostCap { spent_usd: f64, cap_usd: f64 },
    /// Provider rate limit encountered.
    RateLimit { retry_after_secs: Option<u64> },
    /// Execution error with optional partial output.
    ExecutionError {
        message: String,
        partial_output: Option<String>,
    },
    /// Agent became disconnected during execution.
    AgentDisconnected { agent_id: String },
    /// Agent attempted to access a path outside allowed workspaces.
    WorkspaceViolation {
        attempted_path: PathBuf,
        allowed_workspaces: Vec<PathBuf>,
    },
}

/// Reasons a task can be cancelled.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CancellationReason {
    UserRequested,
    Timeout,
    CostCapExceeded,
    AgentDisconnected,
    SystemShutdown,
}

/// Task history entry stored for audit trail.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskHistoryEntry {
    /// Unique task identifier.
    pub task_id: TaskId,
    /// The agent that executed this task.
    pub agent_id: String,
    /// Task description/prompt.
    pub description: String,
    /// How the task was triggered.
    pub trigger: TaskTrigger,
    /// Current state of the task.
    pub state: TaskState,
    /// Workspace directory used for execution.
    pub workspace: PathBuf,
    /// When the task was created.
    pub created_at: DateTime<Utc>,
}

/// Delivery routing target for task results and progress updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyTarget {
    /// The type of channel (e.g., "telegram", "slack").
    pub channel_type: String,
    /// The channel identifier (e.g., chat ID, channel ID).
    pub channel_id: String,
    /// Optional message ID for threading replies.
    pub message_id: Option<String>,
}
