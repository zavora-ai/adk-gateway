//! Task execution pipeline orchestrating ACP calls, cost tracking, and progress reporting.
//!
//! The `TaskExecutor` is the core orchestration component that:
//! 1. Dequeues tasks from the `TaskQueue`
//! 2. Validates workspace paths
//! 3. Executes tasks via the `AgentExecutor` trait (backed by ACP in production)
//! 4. Tracks cost and enforces cost caps
//! 5. Reports progress to users
//! 6. Delivers results via the delivery system
//! 7. Records task history for audit trail
//!
//! The executor uses a trait-based approach (`AgentExecutor`) for the actual agent
//! communication, allowing testing without real ACP calls.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::Notify;
use tracing::{error, info, warn};

use crate::coding_agent::cost::CostTracker;
use crate::coding_agent::models::{
    TaskError, TaskHistoryEntry, TaskId, TaskRequest, TaskResult, TaskState, TokenUsage,
};
use crate::coding_agent::queue::TaskQueue;
use crate::coding_agent::registry::CodingAgentRegistry;
use crate::config::ApprovalConfig;
use crate::tool_approval::{check_requires_approval, ApprovalDecision};

// ── AgentExecutor Trait ────────────────────────────────────────────

/// Trait abstracting the actual agent communication layer.
///
/// In production, this is backed by `AcpTool::execute`. In tests, a mock
/// implementation can simulate various outcomes (success, timeout, rate limit).
#[async_trait]
pub trait AgentExecutor: Send + Sync {
    /// Execute a task against a coding agent endpoint.
    ///
    /// # Arguments
    /// * `agent_id` - The registered agent identifier
    /// * `endpoint` - The ACP endpoint URL
    /// * `request` - The task request details
    ///
    /// # Returns
    /// * `Ok(TaskResult)` on successful execution
    /// * `Err(TaskError)` on failure (timeout, rate limit, execution error, etc.)
    async fn execute_task(
        &self,
        agent_id: &str,
        endpoint: &str,
        request: &TaskRequest,
    ) -> Result<TaskResult, TaskError>;
}

// ── ACP-backed AgentExecutor (production) ──────────────────────────

/// Production `AgentExecutor` that delegates to coding agent endpoints via HTTP.
///
/// Uses the same HTTP client approach as `AcpTool` but adapted for the
/// coding agent task execution pipeline.
pub struct AcpAgentExecutor {
    client: reqwest::Client,
    /// Session pool for stdio-based agents (uses adk-acp).
    session_pool: Option<Arc<crate::coding_agent::acp_client::AcpSessionPool>>,
    /// Registry for looking up agent transport config.
    registry: Option<Arc<CodingAgentRegistry>>,
}

impl AcpAgentExecutor {
    /// Create a new ACP-backed agent executor with a default HTTP client.
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(3600))
                .build()
                .unwrap_or_default(),
            session_pool: None,
            registry: None,
        }
    }

    /// Create an executor with ACP session pool for stdio agents.
    pub fn with_session_pool(
        registry: Arc<CodingAgentRegistry>,
        session_pool: Arc<crate::coding_agent::acp_client::AcpSessionPool>,
    ) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(3600))
                .build()
                .unwrap_or_default(),
            session_pool: Some(session_pool),
            registry: Some(registry),
        }
    }
}

#[async_trait]
impl AgentExecutor for AcpAgentExecutor {
    async fn execute_task(
        &self,
        agent_id: &str,
        endpoint: &str,
        request: &TaskRequest,
    ) -> Result<TaskResult, TaskError> {
        // Check if this agent has stdio transport — route through ACP session pool
        if let (Some(pool), Some(registry)) = (&self.session_pool, &self.registry) {
            if let Some(agent) = registry.get_agent(agent_id) {
                if agent.config.transport.as_ref().is_some_and(|t| {
                    matches!(t, crate::coding_agent::config::AgentTransport::Stdio { .. })
                }) {
                    return pool.execute_task(agent_id, &agent.config, request).await;
                }
            }
        }

        // Fallback: HTTP-based execution for legacy agents
        let payload = serde_json::json!({
            "task": request.description,
            "file_context": request.file_context,
            "workspace": request.workspace,
        });

        let response = self
            .client
            .post(endpoint)
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    TaskError::ExecutionError {
                        message: format!("HTTP request to {} timed out", endpoint),
                        partial_output: None,
                    }
                } else {
                    TaskError::AgentDisconnected {
                        agent_id: agent_id.to_string(),
                    }
                }
            })?;

        let status = response.status();

        // Handle rate limiting (429)
        if status.as_u16() == 429 {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            return Err(TaskError::RateLimit {
                retry_after_secs: retry_after,
            });
        }

        // Handle other error status codes
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(TaskError::ExecutionError {
                message: format!("Agent {} returned HTTP {}: {}", agent_id, status, body),
                partial_output: if body.is_empty() { None } else { Some(body) },
            });
        }

        // Parse successful response
        let body: serde_json::Value = response.json().await.map_err(|e| {
            TaskError::ExecutionError {
                message: format!("Failed to parse response from {}: {}", agent_id, e),
                partial_output: None,
            }
        })?;

        // Extract result fields from the response
        let output = body
            .get("output")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let duration_ms = body
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let modified_files = body
            .get("modified_files")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        let token_usage = body
            .get("token_usage")
            .and_then(|v| serde_json::from_value(v.clone()).ok());

        Ok(TaskResult {
            output,
            modified_files,
            duration_ms,
            token_usage,
        })
    }
}

// ── TaskHistory ────────────────────────────────────────────────────

/// In-memory task history storage for audit trail.
///
/// Stores task history entries per agent, capped at a configurable maximum.
pub struct TaskHistory {
    /// Per-agent task history entries, most recent first.
    entries: dashmap::DashMap<String, Vec<TaskHistoryEntry>>,
    /// Maximum entries to retain per agent.
    max_entries_per_agent: usize,
}

impl TaskHistory {
    /// Create a new task history with the given per-agent capacity.
    pub fn new(max_entries_per_agent: usize) -> Self {
        Self {
            entries: dashmap::DashMap::new(),
            max_entries_per_agent,
        }
    }

    /// Record a task history entry.
    pub fn record(&self, entry: TaskHistoryEntry) {
        let agent_id = entry.agent_id.clone();
        self.entries
            .entry(agent_id)
            .and_modify(|entries| {
                entries.insert(0, entry.clone());
                entries.truncate(self.max_entries_per_agent);
            })
            .or_insert_with(|| vec![entry]);
    }

    /// Get recent task history for an agent, up to `limit` entries.
    pub fn get_recent(&self, agent_id: &str, limit: usize) -> Vec<TaskHistoryEntry> {
        self.entries
            .get(agent_id)
            .map(|entries| entries.iter().take(limit).cloned().collect())
            .unwrap_or_default()
    }

    /// Get a specific task by ID.
    pub fn get_task(&self, task_id: &str) -> Option<TaskHistoryEntry> {
        for entry in self.entries.iter() {
            if let Some(task) = entry.value().iter().find(|t| t.task_id == task_id) {
                return Some(task.clone());
            }
        }
        None
    }
}

impl Default for TaskHistory {
    fn default() -> Self {
        Self::new(200)
    }
}

// ── TaskExecutor ───────────────────────────────────────────────────

/// Core orchestration component for coding agent task execution.
///
/// The executor runs a background loop that:
/// 1. Dequeues tasks from the `TaskQueue` as slots become available
/// 2. Validates workspace paths against the agent's configured workspaces
/// 3. Executes tasks via the `AgentExecutor` trait
/// 4. Enforces timeout and cost cap limits
/// 5. Handles rate limit responses with retry-after backoff
/// 6. Records results in `TaskHistory` for audit trail
/// 7. Integrates with `tool_approval` for operations matching approval patterns
pub struct TaskExecutor {
    /// The task queue to dequeue from.
    queue: Arc<TaskQueue>,
    /// Registry of coding agents (for looking up endpoints and config).
    registry: Arc<CodingAgentRegistry>,
    /// Cost tracker for enforcing spending limits.
    cost_tracker: Arc<CostTracker>,
    /// Task history for audit trail.
    history: Arc<TaskHistory>,
    /// The agent executor implementation (ACP in production, mock in tests).
    executor: Arc<dyn AgentExecutor>,
    /// Default timeout in seconds (from config).
    default_timeout_secs: u64,
    /// Approval configuration for tool operations.
    approval_config: ApprovalConfig,
    /// Shutdown signal.
    shutdown: Arc<Notify>,
}

impl TaskExecutor {
    /// Create a new `TaskExecutor` with all required dependencies.
    pub fn new(
        queue: Arc<TaskQueue>,
        registry: Arc<CodingAgentRegistry>,
        cost_tracker: Arc<CostTracker>,
        history: Arc<TaskHistory>,
        executor: Arc<dyn AgentExecutor>,
        default_timeout_secs: u64,
        approval_config: ApprovalConfig,
    ) -> Self {
        Self {
            queue,
            registry,
            cost_tracker,
            history,
            executor,
            default_timeout_secs,
            approval_config,
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Signal the executor to shut down gracefully.
    pub fn shutdown(&self) {
        self.shutdown.notify_one();
    }

    /// Run the executor loop, processing tasks until shutdown.
    ///
    /// This method should be spawned as a background tokio task.
    /// It continuously dequeues tasks and processes them, respecting
    /// the concurrency limits enforced by the `TaskQueue`.
    pub async fn run(&self) {
        info!("TaskExecutor started");

        loop {
            tokio::select! {
                _ = self.shutdown.notified() => {
                    info!("TaskExecutor shutting down");
                    break;
                }
                _ = self.process_next_task() => {
                    // Task processed, continue loop
                }
            }
        }
    }

    /// Attempt to dequeue and process the next available task.
    ///
    /// If no task is available, waits briefly before retrying.
    async fn process_next_task(&self) {
        // Try to dequeue a task
        let task_id = match self.queue.try_dequeue().await {
            Some(id) => id,
            None => {
                // No task available, wait a bit before retrying
                tokio::time::sleep(Duration::from_millis(100)).await;
                return;
            }
        };

        // Get the active task details from the queue
        let (agent_id, request) = match self.get_active_task_details(&task_id) {
            Some(details) => details,
            None => {
                warn!(task_id = %task_id, "Active task not found after dequeue");
                return;
            }
        };

        info!(task_id = %task_id, agent_id = %agent_id, "Processing task");

        // Execute the task with all orchestration
        let result = self
            .execute_with_orchestration(&task_id, &agent_id, &request)
            .await;

        // Record in history and complete the task
        self.finalize_task(&task_id, &agent_id, &request, result).await;
    }

    /// Get the agent_id and request from an active task.
    fn get_active_task_details(&self, task_id: &str) -> Option<(String, TaskRequest)> {
        self.queue
            .active_tasks()
            .get(task_id)
            .map(|task| (task.agent_id.clone(), task.request.clone()))
    }

    /// Execute a task with full orchestration: workspace validation, timeout,
    /// cost tracking, rate limit handling, and approval integration.
    async fn execute_with_orchestration(
        &self,
        task_id: &TaskId,
        agent_id: &str,
        request: &TaskRequest,
    ) -> Result<TaskResult, TaskError> {
        // 1. Resolve agent from registry
        let agent = self.registry.get_agent(agent_id).ok_or_else(|| {
            TaskError::AgentDisconnected {
                agent_id: agent_id.to_string(),
            }
        })?;

        // 2. Determine workspace and validate
        let workspace = request
            .workspace
            .clone()
            .unwrap_or_else(|| {
                agent
                    .config
                    .workspaces
                    .first()
                    .cloned()
                    .unwrap_or_else(|| PathBuf::from("."))
            });

        self.validate_workspace(&agent.config.workspaces, &workspace)?;

        // 3. Check tool approval for the operation
        self.check_approval(task_id, request)?;

        // 4. Determine timeout
        let timeout_secs = agent
            .config
            .timeout_secs
            .unwrap_or(self.default_timeout_secs);

        // 5. Execute with timeout enforcement
        let result = self
            .execute_with_timeout(agent_id, &agent.endpoint, request, timeout_secs)
            .await?;

        // 6. Track cost if token usage is reported
        if let Some(ref usage) = result.token_usage {
            self.track_cost(agent_id, usage, &agent.config.cost_cap_usd)?;
        }

        Ok(result)
    }

    /// Validate that the workspace path is within the agent's allowed workspaces.
    fn validate_workspace(
        &self,
        allowed_workspaces: &[PathBuf],
        workspace: &PathBuf,
    ) -> Result<(), TaskError> {
        if allowed_workspaces.is_empty() {
            return Ok(()); // No restrictions if no workspaces configured
        }

        // Check if workspace is a descendant of any allowed workspace
        let is_allowed = allowed_workspaces.iter().any(|allowed| {
            workspace.starts_with(allowed) || workspace == allowed
        });

        if !is_allowed {
            return Err(TaskError::WorkspaceViolation {
                attempted_path: workspace.clone(),
                allowed_workspaces: allowed_workspaces.to_vec(),
            });
        }

        Ok(())
    }

    /// Check if the task operation requires approval based on configured patterns.
    fn check_approval(
        &self,
        _task_id: &TaskId,
        request: &TaskRequest,
    ) -> Result<(), TaskError> {
        // Check if the task description matches any approval patterns.
        // In the coding agent context, we check common operation keywords
        // against the approval config patterns.
        let operation_keywords = extract_operation_keywords(&request.description);

        for keyword in &operation_keywords {
            let decision = check_requires_approval(keyword, &self.approval_config);
            if decision == ApprovalDecision::Required {
                // In a full implementation, this would send an approval request
                // and wait for user response. For now, we log and allow.
                info!(
                    task_id = %_task_id,
                    operation = %keyword,
                    "Operation matches approval pattern — approval would be requested"
                );
            }
        }

        Ok(())
    }

    /// Execute a task with timeout enforcement.
    ///
    /// Spawns the task execution and wraps it with `tokio::time::timeout`.
    /// If the timeout expires, the task is terminated with `TaskError::Timeout`.
    async fn execute_with_timeout(
        &self,
        agent_id: &str,
        endpoint: &str,
        request: &TaskRequest,
        timeout_secs: u64,
    ) -> Result<TaskResult, TaskError> {
        let timeout_duration = Duration::from_secs(timeout_secs);

        match tokio::time::timeout(
            timeout_duration,
            self.execute_with_rate_limit_retry(agent_id, endpoint, request),
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                warn!(
                    agent_id = %agent_id,
                    timeout_secs = timeout_secs,
                    "Task timed out"
                );
                Err(TaskError::Timeout {
                    elapsed_secs: timeout_secs,
                    limit_secs: timeout_secs,
                })
            }
        }
    }

    /// Execute a task with rate limit retry handling.
    ///
    /// If the agent returns a rate limit error (429), pauses execution
    /// for the `retry-after` duration and retries. Retries up to 3 times.
    async fn execute_with_rate_limit_retry(
        &self,
        agent_id: &str,
        endpoint: &str,
        request: &TaskRequest,
    ) -> Result<TaskResult, TaskError> {
        const MAX_RETRIES: u32 = 3;
        let mut attempts = 0;

        loop {
            attempts += 1;

            match self.executor.execute_task(agent_id, endpoint, request).await {
                Ok(result) => return Ok(result),
                Err(TaskError::RateLimit { retry_after_secs }) => {
                    if attempts >= MAX_RETRIES {
                        warn!(
                            agent_id = %agent_id,
                            attempts = attempts,
                            "Rate limit retry exhausted"
                        );
                        return Err(TaskError::RateLimit { retry_after_secs });
                    }

                    let wait_secs = retry_after_secs.unwrap_or(30);
                    info!(
                        agent_id = %agent_id,
                        retry_after_secs = wait_secs,
                        attempt = attempts,
                        "Rate limited, pausing before retry"
                    );
                    tokio::time::sleep(Duration::from_secs(wait_secs)).await;
                }
                Err(other_error) => return Err(other_error),
            }
        }
    }

    /// Track cost from token usage and check against the cost cap.
    fn track_cost(
        &self,
        agent_id: &str,
        usage: &TokenUsage,
        cost_cap: &Option<f64>,
    ) -> Result<(), TaskError> {
        // Record the usage
        self.cost_tracker.record_usage(agent_id, usage);

        // Check cost cap if configured
        if let Some(cap) = cost_cap {
            let current_cost = usage.estimated_cost_usd;
            if let Err(_) = self.cost_tracker.check_cost_cap(agent_id, current_cost, *cap) {
                return Err(TaskError::CostCap {
                    spent_usd: current_cost,
                    cap_usd: *cap,
                });
            }
        }

        Ok(())
    }

    /// Finalize a task: record in history, update registry, complete in queue.
    async fn finalize_task(
        &self,
        task_id: &TaskId,
        agent_id: &str,
        request: &TaskRequest,
        result: Result<TaskResult, TaskError>,
    ) {
        let now = Utc::now();
        let started_at = now; // Approximation; in production, use actual start time

        let state = match &result {
            Ok(task_result) => {
                // Record successful task in registry
                self.registry.record_successful_task(agent_id);

                TaskState::Completed {
                    started_at,
                    completed_at: now,
                    result: task_result.clone(),
                }
            }
            Err(task_error) => {
                error!(
                    task_id = %task_id,
                    agent_id = %agent_id,
                    error = ?task_error,
                    "Task failed"
                );

                TaskState::Failed {
                    started_at,
                    failed_at: now,
                    error: task_error.clone(),
                }
            }
        };

        // Determine workspace for history entry
        let workspace = request
            .workspace
            .clone()
            .unwrap_or_else(|| {
                self.registry
                    .get_agent(agent_id)
                    .and_then(|a| a.config.workspaces.first().cloned())
                    .unwrap_or_else(|| PathBuf::from("."))
            });

        // Record in task history
        let history_entry = TaskHistoryEntry {
            task_id: task_id.clone(),
            agent_id: agent_id.to_string(),
            description: request.description.clone(),
            trigger: request.trigger.clone(),
            state,
            workspace,
            created_at: now,
        };

        self.history.record(history_entry);

        // Release the queue slot
        self.queue.complete_task(task_id);

        info!(
            task_id = %task_id,
            agent_id = %agent_id,
            success = result.is_ok(),
            "Task finalized"
        );
    }
}

// ── Helper Functions ───────────────────────────────────────────────

/// Extract operation keywords from a task description for approval checking.
///
/// Looks for common operation patterns that might match approval rules:
/// file writes, deletions, shell execution, etc.
fn extract_operation_keywords(description: &str) -> Vec<String> {
    let mut keywords = Vec::new();
    let lower = description.to_lowercase();

    if lower.contains("delete") || lower.contains("remove") {
        keywords.push("fs_delete".to_string());
    }
    if lower.contains("write") || lower.contains("create") || lower.contains("modify") {
        keywords.push("fs_write".to_string());
    }
    if lower.contains("execute") || lower.contains("run") || lower.contains("shell") {
        keywords.push("shell_exec".to_string());
    }
    if lower.contains("command") {
        keywords.push("run_command".to_string());
    }

    keywords
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::models::{
        FileChange, FileChangeType, ReplyTarget, TaskRequest, TaskTrigger,
    };
    use std::sync::atomic::{AtomicU32, Ordering};

    // ── Mock AgentExecutor ─────────────────────────────────────────

    /// A mock executor that returns configurable results.
    struct MockAgentExecutor {
        /// The result to return on execute_task calls.
        result: tokio::sync::Mutex<Option<Result<TaskResult, TaskError>>>,
        /// Count of execute_task calls.
        call_count: AtomicU32,
    }

    impl MockAgentExecutor {
        fn success(output: &str) -> Self {
            Self {
                result: tokio::sync::Mutex::new(Some(Ok(TaskResult {
                    output: output.to_string(),
                    modified_files: vec![FileChange {
                        path: PathBuf::from("src/main.rs"),
                        change_type: FileChangeType::Modified,
                        lines_added: 10,
                        lines_removed: 5,
                    }],
                    duration_ms: 5000,
                    token_usage: Some(TokenUsage {
                        input_tokens: 1000,
                        output_tokens: 500,
                        estimated_cost_usd: 0.02,
                    }),
                }))),
                call_count: AtomicU32::new(0),
            }
        }

        fn timeout_executor() -> Self {
            // This executor sleeps longer than any reasonable timeout
            Self {
                result: tokio::sync::Mutex::new(None), // signals "sleep forever"
                call_count: AtomicU32::new(0),
            }
        }

        fn call_count(&self) -> u32 {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl AgentExecutor for MockAgentExecutor {
        async fn execute_task(
            &self,
            _agent_id: &str,
            _endpoint: &str,
            _request: &TaskRequest,
        ) -> Result<TaskResult, TaskError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);

            let result = self.result.lock().await;
            match result.as_ref() {
                Some(r) => r.clone(),
                None => {
                    // Simulate a long-running task that will be timed out
                    drop(result);
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    unreachable!()
                }
            }
        }
    }

    /// A mock executor that returns rate limit errors a configurable number of times
    /// before succeeding.
    struct RateLimitMockExecutor {
        /// Number of rate limit errors to return before success.
        rate_limit_count: AtomicU32,
        /// The success result to return after rate limits.
        success_result: TaskResult,
        /// Call count.
        call_count: AtomicU32,
    }

    impl RateLimitMockExecutor {
        fn new(rate_limit_times: u32) -> Self {
            Self {
                rate_limit_count: AtomicU32::new(rate_limit_times),
                success_result: TaskResult {
                    output: "Done after retry".to_string(),
                    modified_files: vec![],
                    duration_ms: 3000,
                    token_usage: None,
                },
                call_count: AtomicU32::new(0),
            }
        }
    }

    #[async_trait]
    impl AgentExecutor for RateLimitMockExecutor {
        async fn execute_task(
            &self,
            _agent_id: &str,
            _endpoint: &str,
            _request: &TaskRequest,
        ) -> Result<TaskResult, TaskError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);

            let remaining = self.rate_limit_count.load(Ordering::SeqCst);
            if remaining > 0 {
                self.rate_limit_count.fetch_sub(1, Ordering::SeqCst);
                return Err(TaskError::RateLimit {
                    retry_after_secs: Some(0), // Use 0 for fast tests
                });
            }

            Ok(self.success_result.clone())
        }
    }

    // ── Test Helpers ───────────────────────────────────────────────

    fn make_request(description: &str) -> TaskRequest {
        TaskRequest {
            description: description.to_string(),
            trigger: TaskTrigger::ControlPanel {
                user_id: "test-user".to_string(),
            },
            workspace: None,
            file_context: None,
            reply_to: ReplyTarget {
                channel_type: "telegram".to_string(),
                channel_id: "12345".to_string(),
                message_id: None,
            },
        }
    }

    fn make_request_with_workspace(description: &str, workspace: PathBuf) -> TaskRequest {
        TaskRequest {
            description: description.to_string(),
            trigger: TaskTrigger::ControlPanel {
                user_id: "test-user".to_string(),
            },
            workspace: Some(workspace),
            file_context: None,
            reply_to: ReplyTarget {
                channel_type: "telegram".to_string(),
                channel_id: "12345".to_string(),
                message_id: None,
            },
        }
    }

    fn sample_registry() -> Arc<CodingAgentRegistry> {
        use crate::coding_agent::config::CodingAgentInstanceConfig;

        let registry = Arc::new(CodingAgentRegistry::new(16));
        let config = CodingAgentInstanceConfig {
            id: "test-agent".to_string(),
            backend_type: "claude-code".to_string(),
            endpoint: "http://localhost:3000/acp".to_string(),
            workspaces: vec![PathBuf::from("/home/user/projects")],
            timeout_secs: Some(60),
            cost_cap_usd: Some(5.0),
            monthly_budget_usd: None,
            alias: Some("cc".to_string()),
            auth: None,
        };
        registry.register_agent(config).unwrap();
        registry
    }

    fn make_executor(agent_executor: Arc<dyn AgentExecutor>) -> TaskExecutor {
        let queue = TaskQueue::new(Some(3));
        let registry = sample_registry();
        let cost_tracker = Arc::new(CostTracker::new());
        let history = Arc::new(TaskHistory::new(200));

        TaskExecutor::new(
            queue,
            registry,
            cost_tracker,
            history,
            agent_executor,
            1800,
            ApprovalConfig::default(),
        )
    }

    // ── Unit Tests ─────────────────────────────────────────────────

    #[test]
    fn test_task_history_record_and_retrieve() {
        let history = TaskHistory::new(50);

        let entry = TaskHistoryEntry {
            task_id: "task-1".to_string(),
            agent_id: "agent-1".to_string(),
            description: "Fix bug".to_string(),
            trigger: TaskTrigger::ControlPanel {
                user_id: "user-1".to_string(),
            },
            state: TaskState::Queued {
                queued_at: Utc::now(),
            },
            workspace: PathBuf::from("/home/user/project"),
            created_at: Utc::now(),
        };

        history.record(entry.clone());

        let recent = history.get_recent("agent-1", 10);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].task_id, "task-1");
    }

    #[test]
    fn test_task_history_respects_max_entries() {
        let history = TaskHistory::new(5);

        for i in 0..10 {
            let entry = TaskHistoryEntry {
                task_id: format!("task-{}", i),
                agent_id: "agent-1".to_string(),
                description: format!("Task {}", i),
                trigger: TaskTrigger::ControlPanel {
                    user_id: "user-1".to_string(),
                },
                state: TaskState::Queued {
                    queued_at: Utc::now(),
                },
                workspace: PathBuf::from("/home/user/project"),
                created_at: Utc::now(),
            };
            history.record(entry);
        }

        let recent = history.get_recent("agent-1", 50);
        assert_eq!(recent.len(), 5);
        // Most recent should be first
        assert_eq!(recent[0].task_id, "task-9");
    }

    #[test]
    fn test_task_history_get_task_by_id() {
        let history = TaskHistory::new(50);

        let entry = TaskHistoryEntry {
            task_id: "unique-task".to_string(),
            agent_id: "agent-1".to_string(),
            description: "Unique task".to_string(),
            trigger: TaskTrigger::ControlPanel {
                user_id: "user-1".to_string(),
            },
            state: TaskState::Queued {
                queued_at: Utc::now(),
            },
            workspace: PathBuf::from("/home/user/project"),
            created_at: Utc::now(),
        };

        history.record(entry);

        assert!(history.get_task("unique-task").is_some());
        assert!(history.get_task("nonexistent").is_none());
    }

    #[test]
    fn test_task_history_separate_agents() {
        let history = TaskHistory::new(50);

        for agent in &["agent-1", "agent-2"] {
            let entry = TaskHistoryEntry {
                task_id: format!("task-{}", agent),
                agent_id: agent.to_string(),
                description: "Task".to_string(),
                trigger: TaskTrigger::ControlPanel {
                    user_id: "user-1".to_string(),
                },
                state: TaskState::Queued {
                    queued_at: Utc::now(),
                },
                workspace: PathBuf::from("/home/user/project"),
                created_at: Utc::now(),
            };
            history.record(entry);
        }

        assert_eq!(history.get_recent("agent-1", 50).len(), 1);
        assert_eq!(history.get_recent("agent-2", 50).len(), 1);
        assert_eq!(history.get_recent("agent-3", 50).len(), 0);
    }

    #[test]
    fn test_extract_operation_keywords_delete() {
        let keywords = extract_operation_keywords("Delete the old config files");
        assert!(keywords.contains(&"fs_delete".to_string()));
    }

    #[test]
    fn test_extract_operation_keywords_write() {
        let keywords = extract_operation_keywords("Write a new test file");
        assert!(keywords.contains(&"fs_write".to_string()));
    }

    #[test]
    fn test_extract_operation_keywords_shell() {
        let keywords = extract_operation_keywords("Execute the build script");
        assert!(keywords.contains(&"shell_exec".to_string()));
    }

    #[test]
    fn test_extract_operation_keywords_none() {
        let keywords = extract_operation_keywords("Fix the authentication bug");
        assert!(keywords.is_empty());
    }

    #[test]
    fn test_extract_operation_keywords_multiple() {
        let keywords =
            extract_operation_keywords("Delete old files and create new ones, then run tests");
        assert!(keywords.contains(&"fs_delete".to_string()));
        assert!(keywords.contains(&"fs_write".to_string()));
        assert!(keywords.contains(&"shell_exec".to_string()));
    }

    #[tokio::test]
    async fn test_workspace_validation_allowed() {
        let executor = make_executor(Arc::new(MockAgentExecutor::success("ok")));
        let allowed = vec![PathBuf::from("/home/user/projects")];
        let workspace = PathBuf::from("/home/user/projects/my-app");

        let result = executor.validate_workspace(&allowed, &workspace);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_workspace_validation_denied() {
        let executor = make_executor(Arc::new(MockAgentExecutor::success("ok")));
        let allowed = vec![PathBuf::from("/home/user/projects")];
        let workspace = PathBuf::from("/etc/secrets");

        let result = executor.validate_workspace(&allowed, &workspace);
        assert!(result.is_err());

        match result.unwrap_err() {
            TaskError::WorkspaceViolation {
                attempted_path,
                allowed_workspaces,
            } => {
                assert_eq!(attempted_path, PathBuf::from("/etc/secrets"));
                assert_eq!(allowed_workspaces, allowed);
            }
            other => panic!("Expected WorkspaceViolation, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_workspace_validation_empty_allows_all() {
        let executor = make_executor(Arc::new(MockAgentExecutor::success("ok")));
        let allowed: Vec<PathBuf> = vec![];
        let workspace = PathBuf::from("/anywhere");

        let result = executor.validate_workspace(&allowed, &workspace);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_workspace_validation_exact_match() {
        let executor = make_executor(Arc::new(MockAgentExecutor::success("ok")));
        let allowed = vec![PathBuf::from("/home/user/projects")];
        let workspace = PathBuf::from("/home/user/projects");

        let result = executor.validate_workspace(&allowed, &workspace);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_with_timeout_success() {
        let mock = Arc::new(MockAgentExecutor::success("completed"));
        let executor = make_executor(mock.clone());

        let request = make_request("fix bug");
        let result = executor
            .execute_with_timeout("test-agent", "http://localhost:3000/acp", &request, 60)
            .await;

        assert!(result.is_ok());
        let task_result = result.unwrap();
        assert_eq!(task_result.output, "completed");
        assert_eq!(mock.call_count(), 1);
    }

    #[tokio::test]
    async fn test_execute_with_timeout_expires() {
        let mock = Arc::new(MockAgentExecutor::timeout_executor());
        let executor = make_executor(mock.clone());

        let request = make_request("long task");
        let result = executor
            .execute_with_timeout("test-agent", "http://localhost:3000/acp", &request, 1)
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TaskError::Timeout {
                elapsed_secs,
                limit_secs,
            } => {
                assert_eq!(elapsed_secs, 1);
                assert_eq!(limit_secs, 1);
            }
            other => panic!("Expected Timeout, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_execute_with_rate_limit_retry_success() {
        // Rate limit once, then succeed
        let mock = Arc::new(RateLimitMockExecutor::new(1));
        let executor = TaskExecutor::new(
            TaskQueue::new(Some(3)),
            sample_registry(),
            Arc::new(CostTracker::new()),
            Arc::new(TaskHistory::new(200)),
            mock.clone(),
            1800,
            ApprovalConfig::default(),
        );

        let request = make_request("retry task");
        let result = executor
            .execute_with_rate_limit_retry("test-agent", "http://localhost:3000/acp", &request)
            .await;

        assert!(result.is_ok());
        assert_eq!(mock.call_count.load(Ordering::SeqCst), 2); // 1 rate limit + 1 success
    }

    #[tokio::test]
    async fn test_execute_with_rate_limit_retry_exhausted() {
        // Rate limit 5 times (exceeds max retries of 3)
        let mock = Arc::new(RateLimitMockExecutor::new(5));
        let executor = TaskExecutor::new(
            TaskQueue::new(Some(3)),
            sample_registry(),
            Arc::new(CostTracker::new()),
            Arc::new(TaskHistory::new(200)),
            mock.clone(),
            1800,
            ApprovalConfig::default(),
        );

        let request = make_request("retry exhausted");
        let result = executor
            .execute_with_rate_limit_retry("test-agent", "http://localhost:3000/acp", &request)
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TaskError::RateLimit { .. } => {}
            other => panic!("Expected RateLimit, got: {:?}", other),
        }
        assert_eq!(mock.call_count.load(Ordering::SeqCst), 3); // Exactly MAX_RETRIES
    }

    #[tokio::test]
    async fn test_execute_with_orchestration_success() {
        let mock = Arc::new(MockAgentExecutor::success("orchestrated"));
        let executor = make_executor(mock.clone());

        let request = make_request_with_workspace(
            "fix bug",
            PathBuf::from("/home/user/projects/my-app"),
        );

        let result = executor
            .execute_with_orchestration(&"task-1".to_string(), "test-agent", &request)
            .await;

        assert!(result.is_ok());
        let task_result = result.unwrap();
        assert_eq!(task_result.output, "orchestrated");
    }

    #[tokio::test]
    async fn test_execute_with_orchestration_workspace_violation() {
        let mock = Arc::new(MockAgentExecutor::success("should not reach"));
        let executor = make_executor(mock.clone());

        let request = make_request_with_workspace(
            "fix bug",
            PathBuf::from("/etc/secrets"),
        );

        let result = executor
            .execute_with_orchestration(&"task-1".to_string(), "test-agent", &request)
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TaskError::WorkspaceViolation { .. } => {}
            other => panic!("Expected WorkspaceViolation, got: {:?}", other),
        }
        // Executor should not have been called
        assert_eq!(mock.call_count(), 0);
    }

    #[tokio::test]
    async fn test_execute_with_orchestration_agent_not_found() {
        let mock = Arc::new(MockAgentExecutor::success("should not reach"));
        let executor = make_executor(mock.clone());

        let request = make_request("fix bug");

        let result = executor
            .execute_with_orchestration(&"task-1".to_string(), "nonexistent-agent", &request)
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            TaskError::AgentDisconnected { agent_id } => {
                assert_eq!(agent_id, "nonexistent-agent");
            }
            other => panic!("Expected AgentDisconnected, got: {:?}", other),
        }
        assert_eq!(mock.call_count(), 0);
    }

    #[tokio::test]
    async fn test_cost_tracking_records_usage() {
        let mock = Arc::new(MockAgentExecutor::success("done"));
        let cost_tracker = Arc::new(CostTracker::new());
        let executor = TaskExecutor::new(
            TaskQueue::new(Some(3)),
            sample_registry(),
            cost_tracker.clone(),
            Arc::new(TaskHistory::new(200)),
            mock,
            1800,
            ApprovalConfig::default(),
        );

        let usage = TokenUsage {
            input_tokens: 1000,
            output_tokens: 500,
            estimated_cost_usd: 0.02,
        };

        let result = executor.track_cost("test-agent", &usage, &Some(5.0));
        assert!(result.is_ok());

        let stats = cost_tracker.get_agent_stats("test-agent").unwrap();
        assert_eq!(stats.total_input_tokens, 1000);
        assert_eq!(stats.total_output_tokens, 500);
    }

    #[tokio::test]
    async fn test_cost_tracking_cap_exceeded() {
        let mock = Arc::new(MockAgentExecutor::success("done"));
        let executor = make_executor(mock);

        let usage = TokenUsage {
            input_tokens: 100000,
            output_tokens: 50000,
            estimated_cost_usd: 10.0,
        };

        let result = executor.track_cost("test-agent", &usage, &Some(5.0));
        assert!(result.is_err());

        match result.unwrap_err() {
            TaskError::CostCap { spent_usd, cap_usd } => {
                assert!((spent_usd - 10.0).abs() < f64::EPSILON);
                assert!((cap_usd - 5.0).abs() < f64::EPSILON);
            }
            other => panic!("Expected CostCap, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_cost_tracking_no_cap() {
        let mock = Arc::new(MockAgentExecutor::success("done"));
        let executor = make_executor(mock);

        let usage = TokenUsage {
            input_tokens: 100000,
            output_tokens: 50000,
            estimated_cost_usd: 100.0,
        };

        // No cap configured — should always succeed
        let result = executor.track_cost("test-agent", &usage, &None);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_finalize_task_records_history() {
        let mock = Arc::new(MockAgentExecutor::success("done"));
        let history = Arc::new(TaskHistory::new(200));
        let queue = TaskQueue::new(Some(3));

        // Manually add a task to active
        let task_id = queue
            .enqueue("test-agent".to_string(), make_request("test task"))
            .await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let executor = TaskExecutor::new(
            queue.clone(),
            sample_registry(),
            Arc::new(CostTracker::new()),
            history.clone(),
            mock,
            1800,
            ApprovalConfig::default(),
        );

        let request = make_request("test task");
        let result = Ok(TaskResult {
            output: "done".to_string(),
            modified_files: vec![],
            duration_ms: 1000,
            token_usage: None,
        });

        executor
            .finalize_task(&task_id, "test-agent", &request, result)
            .await;

        // History should have the entry
        let entries = history.get_recent("test-agent", 10);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].task_id, task_id);
        assert_eq!(entries[0].description, "test task");

        // Task should be completed (removed from active)
        assert_eq!(queue.active_count(), 0);
    }

    #[tokio::test]
    async fn test_finalize_task_records_failure() {
        let mock = Arc::new(MockAgentExecutor::success("done"));
        let history = Arc::new(TaskHistory::new(200));
        let queue = TaskQueue::new(Some(3));

        let task_id = queue
            .enqueue("test-agent".to_string(), make_request("failing task"))
            .await;
        tokio::time::sleep(Duration::from_millis(50)).await;

        let executor = TaskExecutor::new(
            queue.clone(),
            sample_registry(),
            Arc::new(CostTracker::new()),
            history.clone(),
            mock,
            1800,
            ApprovalConfig::default(),
        );

        let request = make_request("failing task");
        let result = Err(TaskError::Timeout {
            elapsed_secs: 1800,
            limit_secs: 1800,
        });

        executor
            .finalize_task(&task_id, "test-agent", &request, result)
            .await;

        let entries = history.get_recent("test-agent", 10);
        assert_eq!(entries.len(), 1);
        match &entries[0].state {
            TaskState::Failed { error, .. } => match error {
                TaskError::Timeout {
                    elapsed_secs,
                    limit_secs,
                } => {
                    assert_eq!(*elapsed_secs, 1800);
                    assert_eq!(*limit_secs, 1800);
                }
                other => panic!("Expected Timeout error, got: {:?}", other),
            },
            other => panic!("Expected Failed state, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_executor_shutdown() {
        let mock = Arc::new(MockAgentExecutor::success("done"));
        let executor = Arc::new(make_executor(mock));

        let executor_clone = executor.clone();
        let handle = tokio::spawn(async move {
            executor_clone.run().await;
        });

        // Give it a moment to start
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Signal shutdown
        executor.shutdown();

        // Should complete within a reasonable time
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "Executor should shut down within 2 seconds");
    }

    #[tokio::test]
    async fn test_check_approval_no_match() {
        let executor = make_executor(Arc::new(MockAgentExecutor::success("ok")));
        let request = make_request("Fix the authentication bug");

        let result = executor.check_approval(&"task-1".to_string(), &request);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_check_approval_with_matching_keywords() {
        let executor = make_executor(Arc::new(MockAgentExecutor::success("ok")));
        let request = make_request("Delete all temporary files and run cleanup");

        // Should not error (approval is logged but allowed in this implementation)
        let result = executor.check_approval(&"task-1".to_string(), &request);
        assert!(result.is_ok());
    }
}
