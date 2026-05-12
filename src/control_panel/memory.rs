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

    let resolved_path = resolve_memory_path(&protocol_path, state.config_path.as_deref());

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

    let resolved_path = resolve_memory_path(&protocol_path, state.config_path.as_deref());

    // Ensure parent directory exists
    if let Some(parent) = resolved_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }

    match std::fs::write(&resolved_path, &payload.content) {
        Ok(()) => {
            tracing::info!(path = %resolved_path.display(), bytes = payload.content.len(), "memory protocol saved via UI");
            axum::Json(serde_json::json!({
                "ok": true,
                "message": format!("Saved ({} bytes).", payload.content.len()),
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

/// Resolve the memory protocol file path.
/// If the path is absolute, use it directly.
/// If relative, resolve from the current working directory (where the gateway was started).
fn resolve_memory_path(protocol_path: &str, _config_path: Option<&std::path::Path>) -> std::path::PathBuf {
    let p = std::path::Path::new(protocol_path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        // Resolve relative to CWD (where the gateway binary was started)
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(p)
    }
}
