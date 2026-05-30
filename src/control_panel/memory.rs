//! Memory protocol load/save API handlers.

use super::ControlPanelState;
use std::sync::Arc;

/// GET /ui/api/memory/entities — list all KG entities across all users.
pub(crate) async fn memory_entities(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let kg = match &state.knowledge_graph {
        Some(kg) => kg,
        None => {
            return axum::Json(serde_json::json!({
                "ok": true,
                "data": { "users": [] }
            }));
        }
    };

    let user_ids = kg.user_ids();
    let mut users = Vec::new();

    for uid in &user_ids {
        let (entities, relations) = kg.read_graph(uid);
        let entity_data: Vec<serde_json::Value> = entities
            .iter()
            .map(|e| {
                serde_json::json!({
                    "name": e.name,
                    "entity_type": e.entity_type,
                    "observations": e.observations.iter().map(|o| &o.content).collect::<Vec<_>>(),
                })
            })
            .collect();

        let relation_data: Vec<serde_json::Value> = relations
            .iter()
            .map(|r| {
                serde_json::json!({
                    "source": r.source,
                    "relation_type": r.relation_type,
                    "target": r.target,
                })
            })
            .collect();

        users.push(serde_json::json!({
            "user_id": uid,
            "entity_count": entities.len(),
            "relation_count": relations.len(),
            "entities": entity_data,
            "relations": relation_data,
        }));
    }

    axum::Json(serde_json::json!({
        "ok": true,
        "data": { "users": users }
    }))
}

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
        .unwrap_or_else(|| "context/MEMORY.md".to_string());

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
        .unwrap_or_else(|| "context/MEMORY.md".to_string());

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
/// Checks multiple locations in order:
/// 1. Absolute path → use directly
/// 2. Relative to CWD (project root when started with `cargo run`)
/// 3. Relative to config file parent (e.g. ~/.adk-gateway/)
fn resolve_memory_path(protocol_path: &str, config_path: Option<&std::path::Path>) -> std::path::PathBuf {
    let p = std::path::Path::new(protocol_path);

    // Absolute path — use directly
    if p.is_absolute() {
        return p.to_path_buf();
    }

    // Try relative to CWD first
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let cwd_path = cwd.join(p);
    if cwd_path.exists() {
        return cwd_path;
    }

    // Try relative to config file parent
    if let Some(cfg) = config_path {
        if let Some(parent) = cfg.parent() {
            let cfg_path = parent.join(p);
            if cfg_path.exists() {
                return cfg_path;
            }
        }
    }

    // Default: use CWD-relative (will be created on save)
    cwd_path
}
