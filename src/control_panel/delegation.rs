//! Delegation permission CRUD API handlers.
//!
//! Provides endpoints for managing agent-to-agent delegation permissions:
//! - GET /ui/api/delegation — list all delegation permissions
//! - POST /ui/api/delegation — add a delegation permission (with cycle detection)
//! - DELETE /ui/api/delegation — remove a delegation permission

use super::ControlPanelState;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

/// A delegation permission record: caller_id is authorized to delegate to target_id.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DelegationRule {
    pub caller_id: String,
    pub target_id: String,
    pub created_at: String,
}

/// In-memory store for delegation permissions.
/// In production this would be backed by a database, but for the control panel
/// we use a simple RwLock-protected Vec.
pub type DelegationStore = Arc<std::sync::RwLock<Vec<DelegationRule>>>;

/// Create a new empty delegation store.
pub fn new_delegation_store() -> DelegationStore {
    Arc::new(std::sync::RwLock::new(Vec::new()))
}

/// Check if adding a delegation from `caller` to `target` would create a cycle
/// in the existing permission graph.
///
/// A cycle exists if `target` can transitively reach `caller` through existing
/// delegation permissions. We perform BFS from `target` following existing edges.
pub fn would_create_cycle(permissions: &[DelegationRule], caller: &str, target: &str) -> bool {
    // Self-delegation is always a cycle
    if caller == target {
        return true;
    }

    // BFS from target: can we reach caller?
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(target.to_string());

    while let Some(node) = queue.pop_front() {
        if node == caller {
            return true;
        }
        if !visited.insert(node.clone()) {
            continue;
        }
        // Follow edges: if `node` delegates to someone, add that someone
        for rule in permissions {
            if rule.caller_id == node {
                queue.push_back(rule.target_id.clone());
            }
        }
    }

    false
}

// ── API Handlers ───────────────────────────────────────────────────

/// GET /ui/api/delegation — list all delegation permissions.
pub(crate) async fn api_delegation_list(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let permissions = state
        .delegation_store
        .read()
        .map(|store| store.clone())
        .unwrap_or_default();

    axum::Json(serde_json::json!({
        "ok": true,
        "data": permissions
    }))
}

/// Request body for adding a delegation permission.
#[derive(serde::Deserialize)]
pub(crate) struct AddDelegationPayload {
    pub caller_id: String,
    pub target_id: String,
}

/// POST /ui/api/delegation — add a delegation permission with cycle detection.
pub(crate) async fn api_delegation_add(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<AddDelegationPayload>,
) -> axum::Json<serde_json::Value> {
    let caller_id = payload.caller_id.trim().to_string();
    let target_id = payload.target_id.trim().to_string();

    if caller_id.is_empty() || target_id.is_empty() {
        return axum::Json(serde_json::json!({
            "ok": false,
            "message": "Both caller_id and target_id are required"
        }));
    }

    let mut store = state.delegation_store.write().expect("lock poisoned");

    // Check for duplicate
    if store
        .iter()
        .any(|r| r.caller_id == caller_id && r.target_id == target_id)
    {
        return axum::Json(serde_json::json!({
            "ok": false,
            "message": "Delegation permission already exists"
        }));
    }

    // Cycle detection: would adding caller→target create a cycle?
    if would_create_cycle(&store, &caller_id, &target_id) {
        return axum::Json(serde_json::json!({
            "ok": false,
            "message": format!(
                "Cannot add delegation {caller_id} → {target_id}: would create a circular chain"
            )
        }));
    }

    let rule = DelegationRule {
        caller_id: caller_id.clone(),
        target_id: target_id.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    store.push(rule);

    tracing::info!(caller = %caller_id, target = %target_id, "delegation permission added via UI");

    axum::Json(serde_json::json!({
        "ok": true,
        "message": format!("Delegation permission added: {caller_id} → {target_id}")
    }))
}

/// Request body for removing a delegation permission.
#[derive(serde::Deserialize)]
pub(crate) struct RemoveDelegationPayload {
    pub caller_id: String,
    pub target_id: String,
}

/// DELETE /ui/api/delegation — remove a delegation permission.
pub(crate) async fn api_delegation_remove(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<RemoveDelegationPayload>,
) -> axum::Json<serde_json::Value> {
    let caller_id = payload.caller_id.trim().to_string();
    let target_id = payload.target_id.trim().to_string();

    let mut store = state.delegation_store.write().expect("lock poisoned");
    let before = store.len();
    store.retain(|r| !(r.caller_id == caller_id && r.target_id == target_id));
    let removed = store.len() < before;

    if removed {
        tracing::info!(caller = %caller_id, target = %target_id, "delegation permission removed via UI");
        axum::Json(serde_json::json!({
            "ok": true,
            "message": format!("Delegation permission removed: {caller_id} → {target_id}")
        }))
    } else {
        axum::Json(serde_json::json!({
            "ok": false,
            "message": "Delegation permission not found"
        }))
    }
}

// ── Unit Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_cycle_simple() {
        let perms = vec![DelegationRule {
            caller_id: "a".into(),
            target_id: "b".into(),
            created_at: String::new(),
        }];
        // Adding b→c should not create a cycle
        assert!(!would_create_cycle(&perms, "b", "c"));
    }

    #[test]
    fn test_direct_cycle() {
        let perms = vec![DelegationRule {
            caller_id: "a".into(),
            target_id: "b".into(),
            created_at: String::new(),
        }];
        // Adding b→a would create a cycle (b→a, a→b already exists)
        assert!(would_create_cycle(&perms, "b", "a"));
    }

    #[test]
    fn test_transitive_cycle() {
        let perms = vec![
            DelegationRule {
                caller_id: "a".into(),
                target_id: "b".into(),
                created_at: String::new(),
            },
            DelegationRule {
                caller_id: "b".into(),
                target_id: "c".into(),
                created_at: String::new(),
            },
        ];
        // Adding c→a would create a cycle (c→a→b→c)
        assert!(would_create_cycle(&perms, "c", "a"));
    }

    #[test]
    fn test_self_delegation_is_cycle() {
        let perms = vec![];
        assert!(would_create_cycle(&perms, "a", "a"));
    }

    #[test]
    fn test_no_cycle_disconnected() {
        let perms = vec![
            DelegationRule {
                caller_id: "a".into(),
                target_id: "b".into(),
                created_at: String::new(),
            },
            DelegationRule {
                caller_id: "c".into(),
                target_id: "d".into(),
                created_at: String::new(),
            },
        ];
        // Adding d→a should not create a cycle (disconnected components)
        assert!(!would_create_cycle(&perms, "d", "a"));
    }

    #[test]
    fn test_longer_transitive_cycle() {
        let perms = vec![
            DelegationRule {
                caller_id: "a".into(),
                target_id: "b".into(),
                created_at: String::new(),
            },
            DelegationRule {
                caller_id: "b".into(),
                target_id: "c".into(),
                created_at: String::new(),
            },
            DelegationRule {
                caller_id: "c".into(),
                target_id: "d".into(),
                created_at: String::new(),
            },
        ];
        // Adding d→a would create a cycle (d→a→b→c→d)
        assert!(would_create_cycle(&perms, "d", "a"));
        // Adding d→b would also create a cycle (d→b→c→d)
        assert!(would_create_cycle(&perms, "d", "b"));
        // Adding e→a should not create a cycle
        assert!(!would_create_cycle(&perms, "e", "a"));
    }
}
