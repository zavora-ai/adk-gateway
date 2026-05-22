//! Task delegation and command parsing for coding agent operations.
//!
//! Provides the `TaskDelegator` which routes coding tasks from various triggers
//! (Telegram commands, cron jobs, control panel, inter-agent delegation) to the
//! appropriate coding agent. Includes natural language command parsing for
//! delegation patterns like "delegate to {agent}: {task}".

use std::sync::Arc;

use tracing::{info, warn};

use super::cost::CostTracker;
use super::error::CodingAgentError;
use super::models::{TaskId, TaskRequest};
use super::queue::TaskQueue;
use super::registry::CodingAgentRegistry;
use super::status::AgentConnectionStatus;
use super::workspace::WorkspaceValidator;

/// A parsed delegation command extracted from user input.
#[derive(Debug, Clone, PartialEq)]
pub struct DelegationCommand {
    /// The agent name or alias specified in the command.
    pub agent_name: String,
    /// The task description to delegate.
    pub task_description: String,
}

/// Routes coding tasks from various triggers to the correct agent.
///
/// The `TaskDelegator` is the central orchestration point for task delegation.
/// It resolves agents by ID or alias, validates preconditions (status, workspace),
/// and enqueues tasks for execution.
pub struct TaskDelegator {
    /// Registry of all configured coding agents.
    registry: Arc<CodingAgentRegistry>,
    /// Task queue for managing concurrent execution.
    task_queue: Arc<TaskQueue>,
    /// Cost tracker for spending limit enforcement.
    cost_tracker: Arc<CostTracker>,
}

impl TaskDelegator {
    /// Creates a new `TaskDelegator` with the given dependencies.
    pub fn new(
        registry: Arc<CodingAgentRegistry>,
        task_queue: Arc<TaskQueue>,
        cost_tracker: Arc<CostTracker>,
    ) -> Self {
        Self {
            registry,
            task_queue,
            cost_tracker,
        }
    }

    /// Delegate a task to a specific coding agent.
    ///
    /// Resolves the agent by ID or alias, checks that it is connected,
    /// validates workspace restrictions, and enqueues the task for execution.
    ///
    /// # Arguments
    /// * `agent_id` - The agent ID or alias to delegate to.
    /// * `task` - The task request containing description, trigger, and delivery info.
    ///
    /// # Returns
    /// A `TaskId` for tracking the queued task.
    ///
    /// # Errors
    /// - `AgentNotFound` if the agent ID/alias cannot be resolved.
    /// - `AgentDisconnected` if the resolved agent is not connected.
    /// - `WorkspaceViolation` if the task workspace is outside allowed directories.
    pub async fn delegate(
        &self,
        agent_id: &str,
        task: TaskRequest,
    ) -> Result<TaskId, CodingAgentError> {
        // Resolve agent by ID first, then try alias
        let resolved_id = self.resolve_agent_id(agent_id)?;

        // Get the agent from registry
        let agent = self
            .registry
            .get_agent(&resolved_id)
            .ok_or_else(|| CodingAgentError::AgentNotFound(agent_id.to_string()))?;

        // Check agent connection status — reject if disconnected
        match &agent.status {
            AgentConnectionStatus::Connected => {}
            AgentConnectionStatus::Disconnected { .. } => {
                return Err(CodingAgentError::AgentDisconnected(resolved_id.clone()));
            }
            AgentConnectionStatus::Error { message, .. } => {
                return Err(CodingAgentError::AgentDisconnected(format!(
                    "{}: {}",
                    resolved_id, message
                )));
            }
            AgentConnectionStatus::Unknown => {
                // Allow delegation to agents with unknown status (not yet health-checked)
            }
        }

        // Validate workspace path restriction
        if let Some(ref workspace_override) = task.workspace {
            let validator = WorkspaceValidator::from_canonical(agent.config.workspaces.clone());
            validator.validate_path(workspace_override)?;
        }

        // Validate file context paths if provided
        if let Some(ref file_context) = task.file_context {
            let validator = WorkspaceValidator::from_canonical(agent.config.workspaces.clone());
            validator.validate_paths(file_context)?;
        }

        // Check if cost cap would be exceeded (pre-check based on existing spend)
        if let Some(cap) = agent.config.cost_cap_usd {
            if let Some(stats) = self.cost_tracker.get_agent_stats(&resolved_id) {
                self.cost_tracker
                    .check_cost_cap(&resolved_id, stats.estimated_total_cost_usd, cap)?;
            }
        }

        // Enqueue the task
        let task_id = self
            .task_queue
            .enqueue(resolved_id.clone(), task)
            .await;

        info!(
            task_id = %task_id,
            agent_id = %resolved_id,
            "Task delegated successfully"
        );

        Ok(task_id)
    }

    /// Parse a delegation command from user input.
    ///
    /// Supports the pattern: `"delegate to {agent_name}: {task_description}"`
    /// Matching is case-insensitive for the "delegate to" prefix.
    /// Agent names and aliases are preserved as-is (case-insensitive matching
    /// happens at resolution time).
    ///
    /// # Examples
    /// ```
    /// use adk_gateway::coding_agent::delegator::TaskDelegator;
    ///
    /// let cmd = TaskDelegator::parse_delegation_command("delegate to cc: fix the auth bug");
    /// assert_eq!(cmd, Some(adk_gateway::coding_agent::delegator::DelegationCommand {
    ///     agent_name: "cc".to_string(),
    ///     task_description: "fix the auth bug".to_string(),
    /// }));
    /// ```
    ///
    /// Returns `None` if the input doesn't match the delegation pattern or
    /// if the task description is empty.
    pub fn parse_delegation_command(input: &str) -> Option<DelegationCommand> {
        let trimmed = input.trim();

        // Case-insensitive prefix match for "delegate to "
        let lower = trimmed.to_lowercase();
        if !lower.starts_with("delegate to ") {
            return None;
        }

        // Extract everything after "delegate to "
        let after_prefix = &trimmed[12..]; // "delegate to " is 12 chars

        // Find the colon separator between agent name and task description
        let colon_pos = after_prefix.find(':')?;

        let agent_name = after_prefix[..colon_pos].trim().to_string();
        let task_description = after_prefix[colon_pos + 1..].trim().to_string();

        // Reject empty agent name or empty task description
        if agent_name.is_empty() || task_description.is_empty() {
            return None;
        }

        Some(DelegationCommand {
            agent_name,
            task_description,
        })
    }

    /// Cancel a running or queued task by its ID.
    ///
    /// This is used to handle the `/stop` command. It removes the task from
    /// the queue (if pending) or signals cancellation (if active).
    ///
    /// # Errors
    /// Returns `CodingAgentError::DelegationFailed` if the task was not found
    /// in either the pending or active queues.
    pub async fn cancel_task(&self, task_id: &str) -> Result<(), CodingAgentError> {
        let cancelled = self.task_queue.cancel_task(task_id).await;

        if cancelled {
            info!(task_id = %task_id, "Task cancelled successfully");
            Ok(())
        } else {
            warn!(task_id = %task_id, "Task not found for cancellation");
            Err(CodingAgentError::DelegationFailed(format!(
                "Task '{}' not found in queue",
                task_id
            )))
        }
    }

    /// Returns a reference to the underlying registry.
    pub fn registry(&self) -> &Arc<CodingAgentRegistry> {
        &self.registry
    }

    /// Returns a reference to the underlying task queue.
    pub fn task_queue(&self) -> &Arc<TaskQueue> {
        &self.task_queue
    }

    /// Returns a reference to the underlying cost tracker.
    pub fn cost_tracker(&self) -> &Arc<CostTracker> {
        &self.cost_tracker
    }

    /// Resolve an agent identifier to a registered agent ID.
    ///
    /// Tries direct ID lookup first, then falls back to alias resolution
    /// (case-insensitive).
    fn resolve_agent_id(&self, agent_id: &str) -> Result<String, CodingAgentError> {
        // Try direct ID lookup
        if self.registry.get_agent(agent_id).is_some() {
            return Ok(agent_id.to_string());
        }

        // Try alias resolution (case-insensitive)
        if let Some(resolved) = self.registry.resolve_by_alias(agent_id) {
            return Ok(resolved);
        }

        // Try case-insensitive ID match
        let lower_id = agent_id.to_lowercase();
        let agents = self.registry.list_agents();
        for agent in &agents {
            if agent.id.to_lowercase() == lower_id {
                return Ok(agent.id.clone());
            }
        }

        Err(CodingAgentError::AgentNotFound(agent_id.to_string()))
    }
}

impl std::fmt::Debug for TaskDelegator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskDelegator")
            .field("registry", &self.registry)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::config::CodingAgentInstanceConfig;
    use crate::coding_agent::models::{ReplyTarget, TaskTrigger};
    use std::path::PathBuf;

    // --- parse_delegation_command tests ---

    #[test]
    fn test_parse_basic_delegation_command() {
        let cmd = TaskDelegator::parse_delegation_command("delegate to cc: fix the auth bug");
        assert_eq!(
            cmd,
            Some(DelegationCommand {
                agent_name: "cc".to_string(),
                task_description: "fix the auth bug".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_delegation_command_case_insensitive_prefix() {
        let cmd = TaskDelegator::parse_delegation_command("Delegate To kiro: update deps");
        assert_eq!(
            cmd,
            Some(DelegationCommand {
                agent_name: "kiro".to_string(),
                task_description: "update deps".to_string(),
            })
        );

        let cmd2 = TaskDelegator::parse_delegation_command("DELEGATE TO claude-code: refactor");
        assert_eq!(
            cmd2,
            Some(DelegationCommand {
                agent_name: "claude-code".to_string(),
                task_description: "refactor".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_delegation_command_with_extra_whitespace() {
        let cmd =
            TaskDelegator::parse_delegation_command("  delegate to  cc :  fix the auth bug  ");
        assert_eq!(
            cmd,
            Some(DelegationCommand {
                agent_name: "cc".to_string(),
                task_description: "fix the auth bug".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_delegation_command_with_special_characters_in_task() {
        let cmd = TaskDelegator::parse_delegation_command(
            "delegate to cc: fix bug #123 (auth) & deploy!",
        );
        assert_eq!(
            cmd,
            Some(DelegationCommand {
                agent_name: "cc".to_string(),
                task_description: "fix bug #123 (auth) & deploy!".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_delegation_command_with_colon_in_task() {
        // Only the first colon separates agent from task
        let cmd = TaskDelegator::parse_delegation_command(
            "delegate to cc: fix error: connection refused",
        );
        assert_eq!(
            cmd,
            Some(DelegationCommand {
                agent_name: "cc".to_string(),
                task_description: "fix error: connection refused".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_delegation_command_empty_task_returns_none() {
        let cmd = TaskDelegator::parse_delegation_command("delegate to cc:");
        assert_eq!(cmd, None);

        let cmd2 = TaskDelegator::parse_delegation_command("delegate to cc:   ");
        assert_eq!(cmd2, None);
    }

    #[test]
    fn test_parse_delegation_command_empty_agent_returns_none() {
        let cmd = TaskDelegator::parse_delegation_command("delegate to : fix bug");
        assert_eq!(cmd, None);

        let cmd2 = TaskDelegator::parse_delegation_command("delegate to   : fix bug");
        assert_eq!(cmd2, None);
    }

    #[test]
    fn test_parse_delegation_command_no_colon_returns_none() {
        let cmd = TaskDelegator::parse_delegation_command("delegate to cc fix the bug");
        assert_eq!(cmd, None);
    }

    #[test]
    fn test_parse_delegation_command_wrong_prefix_returns_none() {
        assert_eq!(
            TaskDelegator::parse_delegation_command("send to cc: fix bug"),
            None
        );
        assert_eq!(
            TaskDelegator::parse_delegation_command("assign to cc: fix bug"),
            None
        );
        assert_eq!(
            TaskDelegator::parse_delegation_command("cc: fix bug"),
            None
        );
        assert_eq!(TaskDelegator::parse_delegation_command(""), None);
    }

    #[test]
    fn test_parse_delegation_command_agent_name_with_hyphens() {
        let cmd =
            TaskDelegator::parse_delegation_command("delegate to claude-code: write unit tests");
        assert_eq!(
            cmd,
            Some(DelegationCommand {
                agent_name: "claude-code".to_string(),
                task_description: "write unit tests".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_delegation_command_agent_name_with_spaces() {
        let cmd =
            TaskDelegator::parse_delegation_command("delegate to pi agent: analyze codebase");
        assert_eq!(
            cmd,
            Some(DelegationCommand {
                agent_name: "pi agent".to_string(),
                task_description: "analyze codebase".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_delegation_command_unicode_in_task() {
        let cmd = TaskDelegator::parse_delegation_command("delegate to cc: fix the 日本語 bug");
        assert_eq!(
            cmd,
            Some(DelegationCommand {
                agent_name: "cc".to_string(),
                task_description: "fix the 日本語 bug".to_string(),
            })
        );
    }

    #[test]
    fn test_parse_delegation_command_multiline_task() {
        // Only first line matters — newlines are part of the task description
        let cmd =
            TaskDelegator::parse_delegation_command("delegate to cc: fix bug\nand deploy");
        assert_eq!(
            cmd,
            Some(DelegationCommand {
                agent_name: "cc".to_string(),
                task_description: "fix bug\nand deploy".to_string(),
            })
        );
    }

    // --- delegate and cancel_task tests (async) ---

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

    fn make_task_request(description: &str) -> TaskRequest {
        TaskRequest {
            description: description.to_string(),
            trigger: TaskTrigger::UserCommand {
                user_id: "user-1".to_string(),
                channel: "telegram".to_string(),
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

    #[tokio::test]
    async fn test_delegate_resolves_by_id() {
        let registry = Arc::new(CodingAgentRegistry::new(16));
        registry
            .register_agent(sample_agent_config("claude-code-1", Some("cc")))
            .unwrap();
        // Set status to Connected so delegation succeeds
        registry
            .update_status("claude-code-1", AgentConnectionStatus::Connected)
            .unwrap();

        let queue = TaskQueue::new(Some(3));
        let cost_tracker = Arc::new(CostTracker::new());

        let delegator = TaskDelegator::new(registry, queue, cost_tracker);
        let task = make_task_request("fix auth bug");

        let result = delegator.delegate("claude-code-1", task).await;
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_delegate_resolves_by_alias() {
        let registry = Arc::new(CodingAgentRegistry::new(16));
        registry
            .register_agent(sample_agent_config("claude-code-1", Some("cc")))
            .unwrap();
        registry
            .update_status("claude-code-1", AgentConnectionStatus::Connected)
            .unwrap();

        let queue = TaskQueue::new(Some(3));
        let cost_tracker = Arc::new(CostTracker::new());

        let delegator = TaskDelegator::new(registry, queue, cost_tracker);
        let task = make_task_request("fix auth bug");

        let result = delegator.delegate("cc", task).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_delegate_rejects_disconnected_agent() {
        let registry = Arc::new(CodingAgentRegistry::new(16));
        registry
            .register_agent(sample_agent_config("claude-code-1", Some("cc")))
            .unwrap();
        registry
            .update_status(
                "claude-code-1",
                AgentConnectionStatus::Disconnected {
                    since: chrono::Utc::now(),
                },
            )
            .unwrap();

        let queue = TaskQueue::new(Some(3));
        let cost_tracker = Arc::new(CostTracker::new());

        let delegator = TaskDelegator::new(registry, queue, cost_tracker);
        let task = make_task_request("fix auth bug");

        let result = delegator.delegate("claude-code-1", task).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CodingAgentError::AgentDisconnected(_)
        ));
    }

    #[tokio::test]
    async fn test_delegate_rejects_unknown_agent() {
        let registry = Arc::new(CodingAgentRegistry::new(16));
        let queue = TaskQueue::new(Some(3));
        let cost_tracker = Arc::new(CostTracker::new());

        let delegator = TaskDelegator::new(registry, queue, cost_tracker);
        let task = make_task_request("fix auth bug");

        let result = delegator.delegate("nonexistent", task).await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CodingAgentError::AgentNotFound(_)
        ));
    }

    #[tokio::test]
    async fn test_delegate_allows_unknown_status() {
        let registry = Arc::new(CodingAgentRegistry::new(16));
        registry
            .register_agent(sample_agent_config("claude-code-1", Some("cc")))
            .unwrap();
        // Status is Unknown by default — should still allow delegation

        let queue = TaskQueue::new(Some(3));
        let cost_tracker = Arc::new(CostTracker::new());

        let delegator = TaskDelegator::new(registry, queue, cost_tracker);
        let task = make_task_request("fix auth bug");

        let result = delegator.delegate("claude-code-1", task).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_cancel_task_success() {
        let registry = Arc::new(CodingAgentRegistry::new(16));
        registry
            .register_agent(sample_agent_config("claude-code-1", Some("cc")))
            .unwrap();
        registry
            .update_status("claude-code-1", AgentConnectionStatus::Connected)
            .unwrap();

        let queue = TaskQueue::new(Some(3));
        let cost_tracker = Arc::new(CostTracker::new());

        let delegator = TaskDelegator::new(registry, queue, cost_tracker);
        let task = make_task_request("fix auth bug");

        let task_id = delegator.delegate("claude-code-1", task).await.unwrap();

        // Give background processor time to move task to active
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        let cancel_result = delegator.cancel_task(&task_id).await;
        assert!(cancel_result.is_ok());
    }

    #[tokio::test]
    async fn test_cancel_task_not_found() {
        let registry = Arc::new(CodingAgentRegistry::new(16));
        let queue = TaskQueue::new(Some(3));
        let cost_tracker = Arc::new(CostTracker::new());

        let delegator = TaskDelegator::new(registry, queue, cost_tracker);

        let result = delegator.cancel_task("nonexistent-task-id").await;
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CodingAgentError::DelegationFailed(_)
        ));
    }

    #[tokio::test]
    async fn test_delegate_case_insensitive_alias() {
        let registry = Arc::new(CodingAgentRegistry::new(16));
        registry
            .register_agent(sample_agent_config("claude-code-1", Some("CC")))
            .unwrap();
        registry
            .update_status("claude-code-1", AgentConnectionStatus::Connected)
            .unwrap();

        let queue = TaskQueue::new(Some(3));
        let cost_tracker = Arc::new(CostTracker::new());

        let delegator = TaskDelegator::new(registry, queue, cost_tracker);

        // Should resolve "cc" to "claude-code-1" (case-insensitive alias)
        let task = make_task_request("fix bug");
        let result = delegator.delegate("cc", task).await;
        assert!(result.is_ok());
    }
}
