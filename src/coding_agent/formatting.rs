//! Result message formatting for multi-channel delivery.
//!
//! Provides formatting functions that adapt task results and errors
//! to channel-specific formats (Markdown for Telegram, Block Kit JSON for Slack).

use std::fmt;

use super::models::{FileChange, FileChangeType, TaskError, TaskResult, TokenUsage};

/// Maximum message length before truncation.
const MAX_MESSAGE_LENGTH: usize = 4000;

/// Threshold for switching to condensed file list format.
const CONDENSED_FILE_THRESHOLD: usize = 5;

/// The delivery channel type, determining output format.
#[derive(Debug, Clone, PartialEq)]
pub enum ChannelType {
    Telegram,
    Slack,
}

impl fmt::Display for ChannelType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChannelType::Telegram => write!(f, "Telegram"),
            ChannelType::Slack => write!(f, "Slack"),
        }
    }
}

/// Format a successful task result message for the given channel.
///
/// - For <= 5 modified files: includes full diff summary (path, change type, lines added/removed).
/// - For > 5 modified files: condensed format (file names + change types) with a "View Full Details" link.
/// - Truncates output at 4000 characters with a link to the full output in the Control Panel.
pub fn format_result_message(result: &TaskResult, channel: ChannelType, task_id: &str) -> String {
    match channel {
        ChannelType::Telegram => format_result_telegram(result, task_id),
        ChannelType::Slack => format_result_slack(result, task_id),
    }
}

/// Format a task error message for the given channel.
pub fn format_error_message(error: &TaskError, channel: ChannelType) -> String {
    match channel {
        ChannelType::Telegram => format_error_telegram(error),
        ChannelType::Slack => format_error_slack(error),
    }
}

// ─── Telegram (Markdown) Formatting ─────────────────────────────────────────

fn format_result_telegram(result: &TaskResult, task_id: &str) -> String {
    let mut msg = String::new();

    msg.push_str("*✅ Task Completed*\n\n");

    // Duration
    let duration = format_duration(result.duration_ms);
    msg.push_str(&format!("*Duration:* {}\n", duration));

    // Token usage
    if let Some(ref usage) = result.token_usage {
        msg.push_str(&format_token_usage_telegram(usage));
    }

    msg.push('\n');

    // File changes
    if result.modified_files.is_empty() {
        msg.push_str("_No files modified._\n");
    } else if result.modified_files.len() <= CONDENSED_FILE_THRESHOLD {
        msg.push_str(&format_files_full_telegram(&result.modified_files));
    } else {
        msg.push_str(&format_files_condensed_telegram(&result.modified_files, task_id));
    }

    // Output summary
    if !result.output.is_empty() {
        msg.push_str("\n*Output:*\n");
        msg.push_str("```\n");
        msg.push_str(&result.output);
        msg.push_str("\n```\n");
    }

    truncate_message(msg, task_id)
}

fn format_token_usage_telegram(usage: &TokenUsage) -> String {
    format!(
        "*Tokens:* {} in / {} out (${:.4})\n",
        usage.input_tokens, usage.output_tokens, usage.estimated_cost_usd
    )
}

fn format_files_full_telegram(files: &[FileChange]) -> String {
    let mut msg = String::new();
    msg.push_str("*Modified Files:*\n");
    for file in files {
        let change_icon = change_type_icon(&file.change_type);
        let path = file.path.display();
        msg.push_str(&format!(
            "  {} `{}` (+{}/-{})\n",
            change_icon, path, file.lines_added, file.lines_removed
        ));
    }
    msg
}

fn format_files_condensed_telegram(files: &[FileChange], task_id: &str) -> String {
    let mut msg = String::new();
    msg.push_str(&format!("*Modified Files ({} files):*\n", files.len()));
    for file in files {
        let change_icon = change_type_icon(&file.change_type);
        let path = file.path.display();
        msg.push_str(&format!("  {} `{}`\n", change_icon, path));
    }
    msg.push_str(&format!(
        "\n[View Full Details]({})\n",
        control_panel_url(task_id)
    ));
    msg
}

fn format_error_telegram(error: &TaskError) -> String {
    let mut msg = String::new();
    msg.push_str("*❌ Task Failed*\n\n");

    match error {
        TaskError::Timeout {
            elapsed_secs,
            limit_secs,
        } => {
            msg.push_str(&format!(
                "*Error:* Timeout after {}s (limit: {}s)\n",
                elapsed_secs, limit_secs
            ));
        }
        TaskError::CostCap { spent_usd, cap_usd } => {
            msg.push_str(&format!(
                "*Error:* Cost cap exceeded (${:.4} spent, ${:.4} cap)\n",
                spent_usd, cap_usd
            ));
        }
        TaskError::RateLimit { retry_after_secs } => {
            let retry_info = match retry_after_secs {
                Some(secs) => format!(" (retry after {}s)", secs),
                None => String::new(),
            };
            msg.push_str(&format!("*Error:* Rate limited{}\n", retry_info));
        }
        TaskError::ExecutionError {
            message,
            partial_output,
        } => {
            msg.push_str(&format!("*Error:* {}\n", message));
            if let Some(output) = partial_output {
                if !output.is_empty() {
                    msg.push_str("\n*Partial Output:*\n");
                    msg.push_str("```\n");
                    msg.push_str(output);
                    msg.push_str("\n```\n");
                }
            }
        }
        TaskError::AgentDisconnected { agent_id } => {
            msg.push_str(&format!(
                "*Error:* Agent `{}` disconnected during execution\n",
                agent_id
            ));
        }
        TaskError::WorkspaceViolation {
            attempted_path,
            allowed_workspaces,
        } => {
            msg.push_str(&format!(
                "*Error:* Workspace violation — attempted to access `{}`\n",
                attempted_path.display()
            ));
            msg.push_str("*Allowed workspaces:*\n");
            for ws in allowed_workspaces {
                msg.push_str(&format!("  • `{}`\n", ws.display()));
            }
        }
    }

    msg
}

// ─── Slack (Block Kit JSON) Formatting ──────────────────────────────────────

fn format_result_slack(result: &TaskResult, task_id: &str) -> String {
    let duration = format_duration(result.duration_ms);

    let mut blocks: Vec<serde_json::Value> = Vec::new();

    // Header
    blocks.push(serde_json::json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": "*✅ Task Completed*"
        }
    }));

    // Duration and token usage
    let mut stats = format!("*Duration:* {}", duration);
    if let Some(ref usage) = result.token_usage {
        stats.push_str(&format!(
            "\n*Tokens:* {} in / {} out (${:.4})",
            usage.input_tokens, usage.output_tokens, usage.estimated_cost_usd
        ));
    }
    blocks.push(serde_json::json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": stats
        }
    }));

    // File changes
    if result.modified_files.is_empty() {
        blocks.push(serde_json::json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": "_No files modified._"
            }
        }));
    } else if result.modified_files.len() <= CONDENSED_FILE_THRESHOLD {
        blocks.push(format_files_full_slack(&result.modified_files));
    } else {
        blocks.push(format_files_condensed_slack(&result.modified_files, task_id));
    }

    // Output summary
    if !result.output.is_empty() {
        let output_text = format!("*Output:*\n```{}```", &result.output);
        blocks.push(serde_json::json!({
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": output_text
            }
        }));
    }

    let payload = serde_json::json!({ "blocks": blocks });
    let formatted = serde_json::to_string(&payload).unwrap_or_default();

    truncate_message(formatted, task_id)
}

fn format_files_full_slack(files: &[FileChange]) -> serde_json::Value {
    let mut text = String::from("*Modified Files:*\n");
    for file in files {
        let change_icon = change_type_icon(&file.change_type);
        let path = file.path.display();
        text.push_str(&format!(
            "  {} `{}` (+{}/-{})\n",
            change_icon, path, file.lines_added, file.lines_removed
        ));
    }
    serde_json::json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": text.trim_end()
        }
    })
}

fn format_files_condensed_slack(files: &[FileChange], task_id: &str) -> serde_json::Value {
    let mut text = format!("*Modified Files ({} files):*\n", files.len());
    for file in files {
        let change_icon = change_type_icon(&file.change_type);
        let path = file.path.display();
        text.push_str(&format!("  {} `{}`\n", change_icon, path));
    }
    text.push_str(&format!("\n<{}|View Full Details>", control_panel_url(task_id)));
    serde_json::json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": text.trim_end()
        }
    })
}

fn format_error_slack(error: &TaskError) -> String {
    let mut blocks: Vec<serde_json::Value> = Vec::new();

    blocks.push(serde_json::json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": "*❌ Task Failed*"
        }
    }));

    let error_text = match error {
        TaskError::Timeout {
            elapsed_secs,
            limit_secs,
        } => {
            format!(
                "*Error:* Timeout after {}s (limit: {}s)",
                elapsed_secs, limit_secs
            )
        }
        TaskError::CostCap { spent_usd, cap_usd } => {
            format!(
                "*Error:* Cost cap exceeded (${:.4} spent, ${:.4} cap)",
                spent_usd, cap_usd
            )
        }
        TaskError::RateLimit { retry_after_secs } => {
            let retry_info = match retry_after_secs {
                Some(secs) => format!(" (retry after {}s)", secs),
                None => String::new(),
            };
            format!("*Error:* Rate limited{}", retry_info)
        }
        TaskError::ExecutionError {
            message,
            partial_output,
        } => {
            let mut text = format!("*Error:* {}", message);
            if let Some(output) = partial_output {
                if !output.is_empty() {
                    text.push_str(&format!("\n*Partial Output:*\n```{}```", output));
                }
            }
            text
        }
        TaskError::AgentDisconnected { agent_id } => {
            format!("*Error:* Agent `{}` disconnected during execution", agent_id)
        }
        TaskError::WorkspaceViolation {
            attempted_path,
            allowed_workspaces,
        } => {
            let mut text = format!(
                "*Error:* Workspace violation — attempted to access `{}`\n*Allowed workspaces:*",
                attempted_path.display()
            );
            for ws in allowed_workspaces {
                text.push_str(&format!("\n  • `{}`", ws.display()));
            }
            text
        }
    };

    blocks.push(serde_json::json!({
        "type": "section",
        "text": {
            "type": "mrkdwn",
            "text": error_text
        }
    }));

    let payload = serde_json::json!({ "blocks": blocks });
    serde_json::to_string(&payload).unwrap_or_default()
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn change_type_icon(change_type: &FileChangeType) -> &'static str {
    match change_type {
        FileChangeType::Added => "➕",
        FileChangeType::Modified => "📝",
        FileChangeType::Deleted => "🗑️",
    }
}

fn format_duration(duration_ms: u64) -> String {
    let secs = duration_ms / 1000;
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        let mins = secs / 60;
        let remaining_secs = secs % 60;
        if remaining_secs == 0 {
            format!("{}m", mins)
        } else {
            format!("{}m {}s", mins, remaining_secs)
        }
    } else {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        if mins == 0 {
            format!("{}h", hours)
        } else {
            format!("{}h {}m", hours, mins)
        }
    }
}

fn control_panel_url(task_id: &str) -> String {
    format!("https://gateway.local/ui/tasks/{}", task_id)
}

fn truncate_message(msg: String, task_id: &str) -> String {
    if msg.len() <= MAX_MESSAGE_LENGTH {
        return msg;
    }

    let link = format!("\n\n... [View full output]({})", control_panel_url(task_id));
    let available = MAX_MESSAGE_LENGTH - link.len();
    let truncated = &msg[..available];
    format!("{}{}", truncated, link)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_file_change(path: &str, change_type: FileChangeType, added: u32, removed: u32) -> FileChange {
        FileChange {
            path: PathBuf::from(path),
            change_type,
            lines_added: added,
            lines_removed: removed,
        }
    }

    fn make_result_with_files(count: usize) -> TaskResult {
        let files: Vec<FileChange> = (0..count)
            .map(|i| make_file_change(
                &format!("src/file_{}.rs", i),
                FileChangeType::Modified,
                10,
                5,
            ))
            .collect();

        TaskResult {
            output: "Task completed successfully.".to_string(),
            modified_files: files,
            duration_ms: 45_000,
            token_usage: Some(TokenUsage {
                input_tokens: 1500,
                output_tokens: 800,
                estimated_cost_usd: 0.0035,
            }),
        }
    }

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(5000), "5s");
        assert_eq!(format_duration(59000), "59s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(60000), "1m");
        assert_eq!(format_duration(90000), "1m 30s");
        assert_eq!(format_duration(120000), "2m");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(3600000), "1h");
        assert_eq!(format_duration(5400000), "1h 30m");
    }

    #[test]
    fn test_telegram_full_diff_for_few_files() {
        let result = make_result_with_files(3);
        let msg = format_result_message(&result, ChannelType::Telegram, "task-123");

        assert!(msg.contains("*✅ Task Completed*"));
        assert!(msg.contains("*Duration:* 45s"));
        assert!(msg.contains("*Tokens:* 1500 in / 800 out"));
        assert!(msg.contains("`src/file_0.rs`"));
        assert!(msg.contains("+10/-5"));
        // Should NOT contain "View Full Details" link for <= 5 files
        assert!(!msg.contains("View Full Details"));
    }

    #[test]
    fn test_telegram_condensed_for_many_files() {
        let result = make_result_with_files(8);
        let msg = format_result_message(&result, ChannelType::Telegram, "task-456");

        assert!(msg.contains("*Modified Files (8 files):*"));
        assert!(msg.contains("`src/file_0.rs`"));
        // Condensed format should NOT include line counts
        assert!(!msg.contains("+10/-5"));
        // Should contain "View Full Details" link
        assert!(msg.contains("View Full Details"));
        assert!(msg.contains("https://gateway.local/ui/tasks/task-456"));
    }

    #[test]
    fn test_telegram_no_files_modified() {
        let result = TaskResult {
            output: "No changes needed.".to_string(),
            modified_files: vec![],
            duration_ms: 2000,
            token_usage: None,
        };
        let msg = format_result_message(&result, ChannelType::Telegram, "task-789");

        assert!(msg.contains("_No files modified._"));
    }

    #[test]
    fn test_telegram_truncation() {
        let long_output = "x".repeat(5000);
        let result = TaskResult {
            output: long_output,
            modified_files: vec![],
            duration_ms: 1000,
            token_usage: None,
        };
        let msg = format_result_message(&result, ChannelType::Telegram, "task-trunc");

        assert!(msg.len() <= MAX_MESSAGE_LENGTH);
        assert!(msg.contains("View full output"));
        assert!(msg.contains("https://gateway.local/ui/tasks/task-trunc"));
    }

    #[test]
    fn test_telegram_error_timeout() {
        let error = TaskError::Timeout {
            elapsed_secs: 1800,
            limit_secs: 1800,
        };
        let msg = format_error_message(&error, ChannelType::Telegram);

        assert!(msg.contains("*❌ Task Failed*"));
        assert!(msg.contains("Timeout after 1800s (limit: 1800s)"));
    }

    #[test]
    fn test_telegram_error_cost_cap() {
        let error = TaskError::CostCap {
            spent_usd: 5.50,
            cap_usd: 5.00,
        };
        let msg = format_error_message(&error, ChannelType::Telegram);

        assert!(msg.contains("Cost cap exceeded"));
        assert!(msg.contains("$5.5000 spent"));
        assert!(msg.contains("$5.0000 cap"));
    }

    #[test]
    fn test_telegram_error_rate_limit() {
        let error = TaskError::RateLimit {
            retry_after_secs: Some(60),
        };
        let msg = format_error_message(&error, ChannelType::Telegram);

        assert!(msg.contains("Rate limited"));
        assert!(msg.contains("retry after 60s"));
    }

    #[test]
    fn test_telegram_error_execution() {
        let error = TaskError::ExecutionError {
            message: "Compilation failed".to_string(),
            partial_output: Some("error[E0308]: mismatched types".to_string()),
        };
        let msg = format_error_message(&error, ChannelType::Telegram);

        assert!(msg.contains("Compilation failed"));
        assert!(msg.contains("Partial Output"));
        assert!(msg.contains("error[E0308]"));
    }

    #[test]
    fn test_telegram_error_agent_disconnected() {
        let error = TaskError::AgentDisconnected {
            agent_id: "claude-code-1".to_string(),
        };
        let msg = format_error_message(&error, ChannelType::Telegram);

        assert!(msg.contains("claude-code-1"));
        assert!(msg.contains("disconnected"));
    }

    #[test]
    fn test_telegram_error_workspace_violation() {
        let error = TaskError::WorkspaceViolation {
            attempted_path: PathBuf::from("/etc/passwd"),
            allowed_workspaces: vec![
                PathBuf::from("/home/user/project"),
                PathBuf::from("/tmp/workspace"),
            ],
        };
        let msg = format_error_message(&error, ChannelType::Telegram);

        assert!(msg.contains("Workspace violation"));
        assert!(msg.contains("/etc/passwd"));
        assert!(msg.contains("/home/user/project"));
        assert!(msg.contains("/tmp/workspace"));
    }

    #[test]
    fn test_slack_full_diff_for_few_files() {
        let result = make_result_with_files(3);
        let msg = format_result_message(&result, ChannelType::Slack, "task-123");

        // Should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        let blocks = parsed["blocks"].as_array().unwrap();

        // First block is header
        assert!(blocks[0]["text"]["text"].as_str().unwrap().contains("✅ Task Completed"));

        // Should contain file details with line counts
        let full_msg = msg.clone();
        assert!(full_msg.contains("+10/-5"));
        assert!(!full_msg.contains("View Full Details"));
    }

    #[test]
    fn test_slack_condensed_for_many_files() {
        let result = make_result_with_files(8);
        let msg = format_result_message(&result, ChannelType::Slack, "task-456");

        // Should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        let blocks = parsed["blocks"].as_array().unwrap();
        assert!(!blocks.is_empty());

        // Should contain "View Full Details" link in Slack format
        assert!(msg.contains("View Full Details"));
        assert!(msg.contains("https://gateway.local/ui/tasks/task-456"));
        // Should NOT contain line counts in condensed mode
        assert!(!msg.contains("+10/-5"));
    }

    #[test]
    fn test_slack_error_format() {
        let error = TaskError::Timeout {
            elapsed_secs: 300,
            limit_secs: 600,
        };
        let msg = format_error_message(&error, ChannelType::Slack);

        // Should be valid JSON
        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        let blocks = parsed["blocks"].as_array().unwrap();

        assert!(blocks[0]["text"]["text"].as_str().unwrap().contains("❌ Task Failed"));
        assert!(msg.contains("Timeout after 300s"));
    }

    #[test]
    fn test_slack_block_kit_structure() {
        let result = make_result_with_files(2);
        let msg = format_result_message(&result, ChannelType::Slack, "task-struct");

        let parsed: serde_json::Value = serde_json::from_str(&msg).unwrap();
        let blocks = parsed["blocks"].as_array().unwrap();

        // All blocks should have type "section"
        for block in blocks {
            assert_eq!(block["type"].as_str().unwrap(), "section");
            assert_eq!(block["text"]["type"].as_str().unwrap(), "mrkdwn");
        }
    }

    #[test]
    fn test_change_type_icons() {
        assert_eq!(change_type_icon(&FileChangeType::Added), "➕");
        assert_eq!(change_type_icon(&FileChangeType::Modified), "📝");
        assert_eq!(change_type_icon(&FileChangeType::Deleted), "🗑️");
    }

    #[test]
    fn test_control_panel_url() {
        assert_eq!(
            control_panel_url("abc-123"),
            "https://gateway.local/ui/tasks/abc-123"
        );
    }

    #[test]
    fn test_truncate_short_message() {
        let msg = "Short message".to_string();
        let result = truncate_message(msg.clone(), "task-1");
        assert_eq!(result, msg);
    }

    #[test]
    fn test_truncate_long_message() {
        let msg = "x".repeat(5000);
        let result = truncate_message(msg, "task-1");
        assert!(result.len() <= MAX_MESSAGE_LENGTH);
        assert!(result.contains("View full output"));
    }

    #[test]
    fn test_exactly_5_files_uses_full_format() {
        let result = make_result_with_files(5);
        let msg = format_result_message(&result, ChannelType::Telegram, "task-5");

        // 5 files should use full format (not condensed)
        assert!(msg.contains("+10/-5"));
        assert!(!msg.contains("View Full Details"));
    }

    #[test]
    fn test_6_files_uses_condensed_format() {
        let result = make_result_with_files(6);
        let msg = format_result_message(&result, ChannelType::Telegram, "task-6");

        // 6 files should use condensed format
        assert!(!msg.contains("+10/-5"));
        assert!(msg.contains("View Full Details"));
    }

    #[test]
    fn test_channel_type_display() {
        assert_eq!(format!("{}", ChannelType::Telegram), "Telegram");
        assert_eq!(format!("{}", ChannelType::Slack), "Slack");
    }
}
