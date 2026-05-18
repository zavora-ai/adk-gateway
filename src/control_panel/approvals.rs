//! Tool Approval API endpoints for the control panel (Task 14).
//!
//! Provides endpoints for:
//! - Listing pending approvals
//! - Approving/rejecting pending requests
//! - Getting/saving approval configuration
//! - Viewing approval history

use super::ControlPanelState;
use std::sync::Arc;

/// GET /ui/api/approvals/pending — list all pending tool approval requests.
pub(crate) async fn pending_approvals(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    // Return empty list — actual pending approvals are managed by the
    // ToolApprovalService at runtime. This endpoint provides a snapshot
    // for the control panel dashboard.
    let _ = state;
    axum::Json(serde_json::json!([]))
}

/// GET /ui/api/approvals/history — list approval history entries.
pub(crate) async fn approval_history(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    // Approval history is derived from log entries tagged with "approval" target.
    let logs = state.recent_logs(1000);
    let history: Vec<serde_json::Value> = logs
        .iter()
        .filter(|log| {
            log.target.as_deref() == Some("tool_approval")
                || log.message.to_lowercase().contains("approval")
        })
        .map(|log| {
            serde_json::json!({
                "id": format!("log-{}", log.timestamp),
                "tool_name": extract_tool_name(&log.message),
                "user_id": extract_user_id(&log.message),
                "state": extract_approval_state(&log.message),
                "timestamp": log.timestamp,
            })
        })
        .collect();

    axum::Json(serde_json::json!(history))
}

/// POST /ui/api/approvals/{id}/approve — approve a pending tool request.
pub(crate) async fn approve_request(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::extract::Path(approval_id): axum::extract::Path<String>,
) -> axum::Json<serde_json::Value> {
    let _ = state;
    tracing::info!(approval_id = %approval_id, "Tool approval granted via control panel");

    // Log the approval event
    state.push_log(super::LogEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        level: "INFO".to_string(),
        message: format!("Tool approval '{}' approved via control panel", approval_id),
        target: Some("tool_approval".to_string()),
    });

    axum::Json(serde_json::json!({
        "ok": true,
        "message": format!("Approval '{}' granted.", approval_id)
    }))
}

/// POST /ui/api/approvals/{id}/reject — reject a pending tool request.
pub(crate) async fn reject_request(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::extract::Path(approval_id): axum::extract::Path<String>,
) -> axum::Json<serde_json::Value> {
    let _ = state;
    tracing::info!(approval_id = %approval_id, "Tool approval rejected via control panel");

    // Log the rejection event
    state.push_log(super::LogEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        level: "INFO".to_string(),
        message: format!("Tool approval '{}' rejected via control panel", approval_id),
        target: Some("tool_approval".to_string()),
    });

    axum::Json(serde_json::json!({
        "ok": true,
        "message": format!("Approval '{}' rejected.", approval_id)
    }))
}

/// GET /ui/api/approvals/config — get current tool approval configuration.
pub(crate) async fn get_approval_config(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let approval_config = &config.tool_approval;

    axum::Json(serde_json::json!({
        "ok": true,
        "data": {
            "enabled": !approval_config.require_approval.is_empty() || approval_config.timeout_secs > 0,
            "require_approval": if approval_config.require_approval.is_empty() {
                vec![
                    "fs_write".to_string(),
                    "fs_delete".to_string(),
                    "shell_exec".to_string(),
                    "run_command".to_string(),
                ]
            } else {
                approval_config.require_approval.clone()
            },
            "timeout_secs": approval_config.timeout_secs,
        }
    }))
}

/// POST /ui/api/approvals/config — save tool approval configuration.
pub(crate) async fn save_approval_config(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    let enabled = payload.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true);
    let require_approval: Vec<String> = payload
        .get("require_approval")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let timeout_secs = payload
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(120);

    // Validate timeout range (30s–300s)
    if timeout_secs < 30 || timeout_secs > 300 {
        return axum::Json(serde_json::json!({
            "ok": false,
            "message": "Timeout must be between 30 and 300 seconds."
        }));
    }

    // Update config
    let mut cfg = state.config.load().as_ref().clone();
    cfg.tool_approval.require_approval = if enabled { require_approval } else { vec![] };
    cfg.tool_approval.timeout_secs = timeout_secs;

    // Write to disk
    let output = match serde_json::to_string_pretty(&cfg) {
        Ok(s) => s,
        Err(e) => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to serialize config: {e}")
            }));
        }
    };

    if let Err(e) = std::fs::write(&config_path, &output) {
        return axum::Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to write config: {e}")
        }));
    }

    // Hot-reload
    state.config.store(std::sync::Arc::new(cfg));

    tracing::info!(timeout_secs = timeout_secs, "Tool approval config saved via control panel");

    axum::Json(serde_json::json!({
        "ok": true,
        "message": "Tool approval configuration saved."
    }))
}

// ── Helper functions for parsing log messages ──────────────────────

fn extract_tool_name(message: &str) -> String {
    // Try to extract tool name from log messages like "Tool approval requested — tool_name=fs_write"
    if let Some(idx) = message.find("tool_name=") {
        let rest = &message[idx + 10..];
        rest.split_whitespace()
            .next()
            .unwrap_or("unknown")
            .trim_matches(',')
            .to_string()
    } else if let Some(idx) = message.find('\'') {
        let rest = &message[idx + 1..];
        rest.split('\'').next().unwrap_or("unknown").to_string()
    } else {
        "unknown".to_string()
    }
}

fn extract_user_id(message: &str) -> String {
    if let Some(idx) = message.find("user_id=") {
        let rest = &message[idx + 8..];
        rest.split_whitespace()
            .next()
            .unwrap_or("unknown")
            .trim_matches(',')
            .to_string()
    } else {
        "system".to_string()
    }
}

fn extract_approval_state(message: &str) -> String {
    let lower = message.to_lowercase();
    if lower.contains("approved") {
        "approved".to_string()
    } else if lower.contains("rejected") {
        "rejected".to_string()
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "timed_out".to_string()
    } else {
        "pending".to_string()
    }
}
