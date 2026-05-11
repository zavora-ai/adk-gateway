//! Memory protocol load/save API handlers.

use super::ControlPanelState;
use std::sync::Arc;

#[derive(serde::Deserialize)]
pub(crate) struct MemoryPayload {
    content: String,
}

pub(crate) async fn memory_load(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let protocol_path = config
        .memory
        .as_ref()
        .map(|m| m.protocol_path.display().to_string())
        .unwrap_or_else(|| "memory.md".to_string());

    let resolved_path = if let Some(ref cfg_path) = state.config_path {
        let p = std::path::Path::new(&protocol_path);
        if p.is_relative() {
            cfg_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(p)
        } else {
            p.to_path_buf()
        }
    } else {
        std::path::PathBuf::from(&protocol_path)
    };

    let content = std::fs::read_to_string(&resolved_path).unwrap_or_default();
    axum::Json(serde_json::json!({
        "content": content,
        "path": resolved_path.display().to_string(),
        "exists": resolved_path.exists(),
    }))
}

pub(crate) async fn memory_save(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<MemoryPayload>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let protocol_path = config
        .memory
        .as_ref()
        .map(|m| m.protocol_path.display().to_string())
        .unwrap_or_else(|| "memory.md".to_string());

    let resolved_path = if let Some(ref cfg_path) = state.config_path {
        let p = std::path::Path::new(&protocol_path);
        if p.is_relative() {
            cfg_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(p)
        } else {
            p.to_path_buf()
        }
    } else {
        std::path::PathBuf::from(&protocol_path)
    };

    match std::fs::write(&resolved_path, &payload.content) {
        Ok(()) => {
            tracing::info!(path = %resolved_path.display(), bytes = payload.content.len(), "memory protocol saved via UI");
            axum::Json(serde_json::json!({
                "ok": true,
                "message": format!("Saved ({} bytes). Restart gateway to apply changes.", payload.content.len()),
            }))
        }
        Err(e) => {
            tracing::error!(path = %resolved_path.display(), error = %e, "failed to save memory protocol");
            axum::Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to save: {e}"),
            }))
        }
    }
}
