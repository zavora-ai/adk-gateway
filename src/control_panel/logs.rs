//! Logs JSON API handler.

use super::{ControlPanelState, LogEntry};
use std::sync::Arc;

pub(crate) async fn logs_json(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<Vec<LogEntry>> {
    let mut logs = state.recent_logs(200);

    // If in-memory buffer is sparse, supplement from the log file
    if logs.len() < 50 {
        let config = state.config.load();
        if let Some(ref log_dir) = config.telemetry.log_dir {
            let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
            let log_file = std::path::Path::new(log_dir)
                .join(format!("adk-gateway.log.{}", today));

            if let Ok(content) = std::fs::read_to_string(&log_file) {
                let file_logs: Vec<LogEntry> = content
                    .lines()
                    .rev()
                    .take(200)
                    .filter_map(|line| parse_log_line(line))
                    .collect();

                // Merge: file logs first (older), then in-memory (newer)
                let mut merged = file_logs;
                merged.extend(logs);
                // Deduplicate by timestamp+message
                merged.dedup_by(|a, b| a.timestamp == b.timestamp && a.message == b.message);
                merged.truncate(200);
                logs = merged;
            }
        }
    }

    axum::Json(logs)
}

/// Parse a tracing log line into a LogEntry.
/// Format: `2026-05-12T04:45:22.351142Z  INFO adk_gateway::gateway: message here`
fn parse_log_line(line: &str) -> Option<LogEntry> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }

    // Try to parse structured format: TIMESTAMP LEVEL TARGET: MESSAGE
    let parts: Vec<&str> = line.splitn(2, "  ").collect();
    if parts.len() < 2 {
        // Fallback: treat entire line as message
        return Some(LogEntry {
            timestamp: String::new(),
            level: "INFO".into(),
            message: line.to_string(),
            target: None,
        });
    }

    let timestamp = parts[0].trim().to_string();
    let rest = parts[1].trim();

    // Extract level (INFO, WARN, ERROR, DEBUG)
    let (level, remainder) = if let Some(r) = rest.strip_prefix("INFO ") {
        ("INFO", r)
    } else if let Some(r) = rest.strip_prefix("WARN ") {
        ("WARN", r)
    } else if let Some(r) = rest.strip_prefix("ERROR ") {
        ("ERROR", r)
    } else if let Some(r) = rest.strip_prefix("DEBUG ") {
        ("DEBUG", r)
    } else {
        ("INFO", rest)
    };

    // Extract target and message: "target: message"
    let (target, message) = if let Some(colon_pos) = remainder.find(": ") {
        let t = &remainder[..colon_pos];
        let m = &remainder[colon_pos + 2..];
        (Some(t.to_string()), m.to_string())
    } else {
        (None, remainder.to_string())
    };

    Some(LogEntry {
        timestamp,
        level: level.to_string(),
        message,
        target,
    })
}
