//! Interactive tool approval flow for dangerous tool calls.
//!
//! When the Runner invokes a tool marked as `requires_approval`, execution pauses
//! and a message with inline keyboard buttons (✅ Approve / ❌ Reject) is sent to
//! the user. The system waits up to 120 seconds for a response.
//!
//! Default dangerous categories: `fs_write`, `fs_delete`, `shell_exec`, `run_command`.
//! Custom approval rules from config override defaults entirely.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::oneshot;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::ApprovalConfig;

// ── Constants ──────────────────────────────────────────────────────

/// Default tool categories that require approval.
pub const DEFAULT_DANGEROUS_TOOLS: &[&str] = &[
    "fs_write",
    "fs_delete",
    "shell_exec",
    "run_command",
];

/// Default timeout in seconds before auto-reject.
pub const DEFAULT_TIMEOUT_SECS: u64 = 120;

// ── Enums ──────────────────────────────────────────────────────────

/// Classification of whether a tool requires approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// The tool requires user approval before execution.
    Required,
    /// The tool does not require approval; proceed immediately.
    NotRequired,
}

/// State machine for a pending approval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalState {
    /// Waiting for user response; will expire at the given instant.
    Pending { expires_at_ms: u64 },
    /// User approved the tool call.
    Approved,
    /// User rejected the tool call.
    Rejected,
    /// Approval timed out (auto-rejected).
    TimedOut,
}

// ── Pending Approval ───────────────────────────────────────────────

/// A pending approval request waiting for user response.
pub struct PendingApproval {
    /// Unique identifier for this approval request.
    pub id: String,
    /// Name of the tool being invoked.
    pub tool_name: String,
    /// Arguments passed to the tool.
    pub tool_args: serde_json::Value,
    /// User who needs to approve.
    pub user_id: String,
    /// When the approval was requested.
    pub requested_at: Instant,
    /// When the approval expires.
    pub expires_at: Instant,
    /// Channel to send the approval result back to the caller.
    pub callback_tx: Option<oneshot::Sender<ApprovalState>>,
}

// ── Trait ──────────────────────────────────────────────────────────

/// Service for managing tool approval flows.
///
/// Implementations handle the full lifecycle: checking if approval is needed,
/// requesting it from the user, and processing callbacks.
#[async_trait::async_trait]
pub trait ToolApprovalService: Send + Sync {
    /// Determine if a tool call requires approval based on the tool name and config.
    ///
    /// When custom rules are configured (non-empty `require_approval` in config),
    /// they take complete precedence over defaults.
    fn requires_approval(&self, tool_name: &str, config: &ApprovalConfig) -> ApprovalDecision;

    /// Request approval from the user, returning a oneshot receiver for the result.
    ///
    /// This sends a message with inline keyboard buttons to the user and returns
    /// a receiver that will resolve when the user responds or the timeout expires.
    async fn request_approval(
        &self,
        tool_name: &str,
        tool_args: &serde_json::Value,
        user_id: &str,
    ) -> Result<oneshot::Receiver<ApprovalState>, anyhow::Error>;

    /// Handle a callback from an inline button press.
    ///
    /// Resolves the pending approval with Approved or Rejected based on the
    /// user's button press.
    async fn handle_callback(
        &self,
        callback_id: &str,
        approved: bool,
    ) -> Result<(), anyhow::Error>;
}

// ── Implementation ─────────────────────────────────────────────────

/// Default implementation of the tool approval service.
///
/// Stores pending approvals in a concurrent map and resolves them
/// when callbacks arrive or timeouts expire.
pub struct DefaultToolApprovalService {
    /// Pending approvals indexed by their unique ID.
    pending: Arc<DashMap<String, PendingApproval>>,
    /// Timeout duration for approval requests.
    timeout: Duration,
}

impl DefaultToolApprovalService {
    /// Create a new approval service with the given timeout.
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            pending: Arc::new(DashMap::new()),
            timeout: Duration::from_secs(timeout_secs),
        }
    }

    /// Create a new approval service with the default timeout (120 seconds).
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_TIMEOUT_SECS)
    }

    /// Get the number of pending approvals.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Check if a specific approval is still pending.
    pub fn is_pending(&self, approval_id: &str) -> bool {
        self.pending.contains_key(approval_id)
    }

    /// Remove expired approvals and notify their receivers of timeout.
    pub fn expire_stale(&self) {
        let now = Instant::now();
        let mut expired_ids = Vec::new();

        for entry in self.pending.iter() {
            if now >= entry.value().expires_at {
                expired_ids.push(entry.key().clone());
            }
        }

        for id in expired_ids {
            if let Some((_, mut approval)) = self.pending.remove(&id) {
                if let Some(tx) = approval.callback_tx.take() {
                    let _ = tx.send(ApprovalState::TimedOut);
                }
                warn!(
                    approval_id = %id,
                    tool_name = %approval.tool_name,
                    user_id = %approval.user_id,
                    "Tool approval timed out (auto-rejected)"
                );
            }
        }
    }
}

#[async_trait::async_trait]
impl ToolApprovalService for DefaultToolApprovalService {
    fn requires_approval(&self, tool_name: &str, config: &ApprovalConfig) -> ApprovalDecision {
        check_requires_approval(tool_name, config)
    }

    async fn request_approval(
        &self,
        tool_name: &str,
        tool_args: &serde_json::Value,
        user_id: &str,
    ) -> Result<oneshot::Receiver<ApprovalState>, anyhow::Error> {
        let (tx, rx) = oneshot::channel();
        let now = Instant::now();
        let expires_at = now + self.timeout;
        let id = Uuid::new_v4().to_string();

        let approval = PendingApproval {
            id: id.clone(),
            tool_name: tool_name.to_string(),
            tool_args: tool_args.clone(),
            user_id: user_id.to_string(),
            requested_at: now,
            expires_at,
            callback_tx: Some(tx),
        };

        info!(
            approval_id = %id,
            tool_name = %tool_name,
            user_id = %user_id,
            timeout_secs = self.timeout.as_secs(),
            "Tool approval requested — ⏳ Waiting for approval..."
        );

        self.pending.insert(id.clone(), approval);

        // Spawn a timeout task that will auto-reject if no response arrives
        let pending_clone = Arc::clone(&self.pending);
        let timeout = self.timeout;
        let id_clone = id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            if let Some((_, mut approval)) = pending_clone.remove(&id_clone) {
                if let Some(tx) = approval.callback_tx.take() {
                    let _ = tx.send(ApprovalState::TimedOut);
                }
                warn!(
                    approval_id = %id_clone,
                    tool_name = %approval.tool_name,
                    "Tool approval timed out after {}s — auto-rejected",
                    timeout.as_secs()
                );
            }
        });

        Ok(rx)
    }

    async fn handle_callback(
        &self,
        callback_id: &str,
        approved: bool,
    ) -> Result<(), anyhow::Error> {
        let entry = self.pending.remove(callback_id);

        match entry {
            Some((_, mut approval)) => {
                let state = if approved {
                    ApprovalState::Approved
                } else {
                    ApprovalState::Rejected
                };

                info!(
                    approval_id = %callback_id,
                    tool_name = %approval.tool_name,
                    user_id = %approval.user_id,
                    approved = approved,
                    "Tool approval callback received"
                );

                if let Some(tx) = approval.callback_tx.take() {
                    tx.send(state).map_err(|_| {
                        anyhow::anyhow!("Failed to send approval state — receiver dropped")
                    })?;
                }

                Ok(())
            }
            None => {
                warn!(
                    callback_id = %callback_id,
                    "Received callback for unknown or expired approval"
                );
                Err(anyhow::anyhow!(
                    "Approval '{}' not found — it may have already expired or been processed",
                    callback_id
                ))
            }
        }
    }
}

// ── Pure Decision Logic ────────────────────────────────────────────

/// Pure function to determine if a tool requires approval.
///
/// When custom rules are configured (non-empty `require_approval` in config),
/// they take complete precedence over defaults. Otherwise, the default
/// dangerous tool categories are used.
pub fn check_requires_approval(tool_name: &str, config: &ApprovalConfig) -> ApprovalDecision {
    let active_rules: &[String] = &config.require_approval;

    // If custom rules are configured (non-empty), they take complete precedence
    if !active_rules.is_empty() {
        if active_rules.iter().any(|pattern| matches_pattern(tool_name, pattern)) {
            return ApprovalDecision::Required;
        }
        return ApprovalDecision::NotRequired;
    }

    // Fall back to defaults when no custom rules are configured
    if DEFAULT_DANGEROUS_TOOLS.iter().any(|&default| matches_pattern(tool_name, default)) {
        return ApprovalDecision::Required;
    }

    ApprovalDecision::NotRequired
}

/// Match a tool name against a pattern.
///
/// Supports:
/// - Exact match: `"fs_write"` matches `"fs_write"`
/// - Prefix wildcard: `"fs_*"` matches `"fs_write"`, `"fs_delete"`
/// - Suffix wildcard: `"*_exec"` matches `"shell_exec"`
/// - Contains wildcard: `"*delete*"` matches `"fs_delete"`, `"kg_delete_node"`
/// - Plain substring when no wildcards: exact match only
pub fn matches_pattern(tool_name: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let starts_with_star = pattern.starts_with('*');
    let ends_with_star = pattern.ends_with('*');

    match (starts_with_star, ends_with_star) {
        (true, true) => {
            // *substring* — contains match
            let inner = &pattern[1..pattern.len() - 1];
            if inner.is_empty() {
                return true; // "**" matches everything
            }
            tool_name.contains(inner)
        }
        (true, false) => {
            // *suffix — ends with
            let suffix = &pattern[1..];
            tool_name.ends_with(suffix)
        }
        (false, true) => {
            // prefix* — starts with
            let prefix = &pattern[..pattern.len() - 1];
            tool_name.starts_with(prefix)
        }
        (false, false) => {
            // Exact match
            tool_name == pattern
        }
    }
}

/// Build the inline keyboard markup data for a Telegram approval message.
///
/// Returns a structure suitable for serialization into Telegram's
/// `InlineKeyboardMarkup` format.
pub fn build_approval_keyboard(approval_id: &str) -> serde_json::Value {
    serde_json::json!({
        "inline_keyboard": [[
            {
                "text": "✅ Approve",
                "callback_data": format!("approve:{}", approval_id)
            },
            {
                "text": "❌ Reject",
                "callback_data": format!("reject:{}", approval_id)
            }
        ]]
    })
}

/// Build the approval request message text.
pub fn build_approval_message(tool_name: &str, tool_args: &serde_json::Value) -> String {
    let args_summary = match tool_args {
        serde_json::Value::Object(map) => {
            let keys: Vec<&String> = map.keys().take(5).collect();
            if keys.is_empty() {
                "no arguments".to_string()
            } else {
                format!("args: {}", keys.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", "))
            }
        }
        _ => "no arguments".to_string(),
    };

    format!(
        "🔒 *Tool Approval Required*\n\n\
         Tool: `{}`\n\
         {}\n\n\
         ⏳ Waiting for approval... (120s timeout)",
        tool_name, args_summary
    )
}

/// Parse a callback data string from a Telegram inline button press.
///
/// Expected format: `"approve:<approval_id>"` or `"reject:<approval_id>"`.
/// Returns `(approval_id, approved)` if valid.
pub fn parse_callback_data(data: &str) -> Option<(&str, bool)> {
    if let Some(id) = data.strip_prefix("approve:") {
        Some((id, true))
    } else if let Some(id) = data.strip_prefix("reject:") {
        Some((id, false))
    } else {
        None
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> ApprovalConfig {
        ApprovalConfig::default()
    }

    fn custom_config(rules: Vec<&str>) -> ApprovalConfig {
        ApprovalConfig {
            require_approval: rules.into_iter().map(String::from).collect(),
            timeout_secs: 120,
        }
    }

    // ── requires_approval tests ────────────────────────────────────

    #[test]
    fn test_default_rules_require_approval_for_dangerous_tools() {
        let config = default_config();
        let service = DefaultToolApprovalService::with_defaults();

        assert_eq!(
            service.requires_approval("fs_write", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("fs_delete", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("shell_exec", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("run_command", &config),
            ApprovalDecision::Required
        );
    }

    #[test]
    fn test_default_rules_allow_safe_tools() {
        let config = default_config();
        let service = DefaultToolApprovalService::with_defaults();

        assert_eq!(
            service.requires_approval("read_file", &config),
            ApprovalDecision::NotRequired
        );
        assert_eq!(
            service.requires_approval("list_directory", &config),
            ApprovalDecision::NotRequired
        );
        assert_eq!(
            service.requires_approval("search", &config),
            ApprovalDecision::NotRequired
        );
    }

    #[test]
    fn test_custom_rules_override_defaults_completely() {
        let config = custom_config(vec!["custom_tool", "another_tool"]);
        let service = DefaultToolApprovalService::with_defaults();

        // Custom rules should match
        assert_eq!(
            service.requires_approval("custom_tool", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("another_tool", &config),
            ApprovalDecision::Required
        );

        // Default dangerous tools should NOT require approval when custom rules are set
        assert_eq!(
            service.requires_approval("fs_write", &config),
            ApprovalDecision::NotRequired
        );
        assert_eq!(
            service.requires_approval("shell_exec", &config),
            ApprovalDecision::NotRequired
        );
    }

    #[test]
    fn test_wildcard_prefix_matching() {
        let config = custom_config(vec!["fs_*"]);
        let service = DefaultToolApprovalService::with_defaults();

        assert_eq!(
            service.requires_approval("fs_write", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("fs_delete", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("fs_read", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("shell_exec", &config),
            ApprovalDecision::NotRequired
        );
    }

    #[test]
    fn test_wildcard_suffix_matching() {
        let config = custom_config(vec!["*_exec"]);
        let service = DefaultToolApprovalService::with_defaults();

        assert_eq!(
            service.requires_approval("shell_exec", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("remote_exec", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("run_command", &config),
            ApprovalDecision::NotRequired
        );
    }

    #[test]
    fn test_wildcard_contains_matching() {
        let config = custom_config(vec!["*delete*"]);
        let service = DefaultToolApprovalService::with_defaults();

        assert_eq!(
            service.requires_approval("fs_delete", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("kg_delete_node", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("delete_all", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("fs_write", &config),
            ApprovalDecision::NotRequired
        );
    }

    #[test]
    fn test_star_matches_everything() {
        let config = custom_config(vec!["*"]);
        let service = DefaultToolApprovalService::with_defaults();

        assert_eq!(
            service.requires_approval("anything", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("fs_write", &config),
            ApprovalDecision::Required
        );
    }

    #[test]
    fn test_empty_custom_rules_falls_back_to_defaults() {
        let config = ApprovalConfig {
            require_approval: vec![],
            timeout_secs: 120,
        };
        let service = DefaultToolApprovalService::with_defaults();

        // Should use defaults
        assert_eq!(
            service.requires_approval("fs_write", &config),
            ApprovalDecision::Required
        );
        assert_eq!(
            service.requires_approval("read_file", &config),
            ApprovalDecision::NotRequired
        );
    }

    // ── Pattern matching tests ─────────────────────────────────────

    #[test]
    fn test_matches_pattern_exact() {
        assert!(matches_pattern("fs_write", "fs_write"));
        assert!(!matches_pattern("fs_write", "fs_read"));
    }

    #[test]
    fn test_matches_pattern_prefix_wildcard() {
        assert!(matches_pattern("fs_write", "fs_*"));
        assert!(matches_pattern("fs_delete", "fs_*"));
        assert!(!matches_pattern("shell_exec", "fs_*"));
    }

    #[test]
    fn test_matches_pattern_suffix_wildcard() {
        assert!(matches_pattern("shell_exec", "*_exec"));
        assert!(matches_pattern("remote_exec", "*_exec"));
        assert!(!matches_pattern("run_command", "*_exec"));
    }

    #[test]
    fn test_matches_pattern_contains_wildcard() {
        assert!(matches_pattern("fs_delete", "*delete*"));
        assert!(matches_pattern("kg_delete_node", "*delete*"));
        assert!(!matches_pattern("fs_write", "*delete*"));
    }

    // ── Callback data parsing ──────────────────────────────────────

    #[test]
    fn test_parse_callback_data_approve() {
        let result = parse_callback_data("approve:abc-123");
        assert_eq!(result, Some(("abc-123", true)));
    }

    #[test]
    fn test_parse_callback_data_reject() {
        let result = parse_callback_data("reject:abc-123");
        assert_eq!(result, Some(("abc-123", false)));
    }

    #[test]
    fn test_parse_callback_data_invalid() {
        assert_eq!(parse_callback_data("invalid:abc"), None);
        assert_eq!(parse_callback_data(""), None);
        assert_eq!(parse_callback_data("approve"), None);
    }

    // ── Approval message building ──────────────────────────────────

    #[test]
    fn test_build_approval_message() {
        let args = serde_json::json!({"path": "/tmp/test", "content": "hello"});
        let msg = build_approval_message("fs_write", &args);
        assert!(msg.contains("fs_write"));
        assert!(msg.contains("⏳ Waiting for approval..."));
        assert!(msg.contains("path"));
    }

    #[test]
    fn test_build_approval_keyboard() {
        let kb = build_approval_keyboard("test-id-123");
        let keyboard = kb["inline_keyboard"].as_array().unwrap();
        let row = keyboard[0].as_array().unwrap();
        assert_eq!(row[0]["text"], "✅ Approve");
        assert_eq!(row[0]["callback_data"], "approve:test-id-123");
        assert_eq!(row[1]["text"], "❌ Reject");
        assert_eq!(row[1]["callback_data"], "reject:test-id-123");
    }

    // ── Async tests ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_request_and_approve() {
        let service = DefaultToolApprovalService::new(10); // 10s timeout for test
        let args = serde_json::json!({"path": "/tmp/test"});

        let rx = service
            .request_approval("fs_write", &args, "user-1")
            .await
            .unwrap();

        assert_eq!(service.pending_count(), 1);

        // Get the approval ID
        let approval_id = service
            .pending
            .iter()
            .next()
            .map(|entry| entry.key().clone())
            .unwrap();

        // Handle the callback (approve)
        service.handle_callback(&approval_id, true).await.unwrap();

        // Receiver should get Approved
        let state = rx.await.unwrap();
        assert_eq!(state, ApprovalState::Approved);
        assert_eq!(service.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_request_and_reject() {
        let service = DefaultToolApprovalService::new(10);
        let args = serde_json::json!({"command": "rm -rf /"});

        let rx = service
            .request_approval("shell_exec", &args, "user-1")
            .await
            .unwrap();

        let approval_id = service
            .pending
            .iter()
            .next()
            .map(|entry| entry.key().clone())
            .unwrap();

        // Handle the callback (reject)
        service.handle_callback(&approval_id, false).await.unwrap();

        let state = rx.await.unwrap();
        assert_eq!(state, ApprovalState::Rejected);
        assert_eq!(service.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_timeout_auto_rejects() {
        let service = DefaultToolApprovalService::new(1); // 1 second timeout
        let args = serde_json::json!({});

        let rx = service
            .request_approval("fs_delete", &args, "user-1")
            .await
            .unwrap();

        // Wait for timeout
        tokio::time::sleep(Duration::from_millis(1500)).await;

        let state = rx.await.unwrap();
        assert_eq!(state, ApprovalState::TimedOut);
        assert_eq!(service.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_handle_callback_unknown_id() {
        let service = DefaultToolApprovalService::with_defaults();

        let result = service.handle_callback("nonexistent-id", true).await;
        assert!(result.is_err());
    }
}
