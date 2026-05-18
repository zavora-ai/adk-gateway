//! REST API handlers for coding agent management in the control panel.
//!
//! Provides endpoints for listing, registering, unregistering, and managing
//! coding agents, their tasks, costs, and configuration.
//!
//! Endpoints:
//! - GET  /ui/api/coding-agents              — list all registered agents with status
//! - GET  /ui/api/coding-agents/:id          — get agent details
//! - POST /ui/api/coding-agents              — register a new agent
//! - DELETE /ui/api/coding-agents/:id        — unregister an agent
//! - GET  /ui/api/coding-agents/:id/tasks    — get task history (paginated)
//! - GET  /ui/api/coding-agents/:id/tasks/:task_id — get task detail
//! - POST /ui/api/coding-agents/:id/tasks    — delegate a task from UI
//! - POST /ui/api/coding-agents/:id/tasks/:task_id/cancel — cancel a running task
//! - GET  /ui/api/coding-agents/:id/costs    — get cost statistics
//! - PUT  /ui/api/coding-agents/:id/config   — update agent configuration

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use super::ControlPanelState;
use crate::coding_agent::config::CodingAgentInstanceConfig;
use crate::coding_agent::cost::CostTracker;
use crate::coding_agent::delegator::TaskDelegator;
use crate::coding_agent::history::TaskHistory;
use crate::coding_agent::models::{ReplyTarget, TaskRequest, TaskTrigger};
use crate::coding_agent::registry::CodingAgentRegistry;

// ── Shared State Extension ─────────────────────────────────────────

/// Extended control panel state that includes coding agent subsystem references.
/// This is stored alongside the main ControlPanelState.
pub struct CodingAgentPanelState {
    pub registry: Arc<CodingAgentRegistry>,
    pub delegator: Arc<TaskDelegator>,
    pub cost_tracker: Arc<CostTracker>,
    pub task_history: Arc<TaskHistory>,
}

impl std::fmt::Debug for CodingAgentPanelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodingAgentPanelState").finish()
    }
}

// ── Request / Response Types ───────────────────────────────────────

/// Query parameters for paginated task history.
#[derive(Debug, Deserialize)]
pub struct TaskHistoryQuery {
    /// Maximum number of tasks to return (default: 50).
    pub limit: Option<usize>,
}

/// Request body for registering a new coding agent.
#[derive(Debug, Deserialize)]
pub struct RegisterAgentRequest {
    pub id: String,
    #[serde(rename = "backendType")]
    pub backend_type: String,
    pub endpoint: String,
    pub workspaces: Vec<String>,
    #[serde(rename = "timeoutSecs")]
    pub timeout_secs: Option<u64>,
    #[serde(rename = "costCapUsd")]
    pub cost_cap_usd: Option<f64>,
    #[serde(rename = "monthlyBudgetUsd")]
    pub monthly_budget_usd: Option<f64>,
    pub alias: Option<String>,
}

/// Request body for delegating a task from the UI.
#[derive(Debug, Deserialize)]
pub struct DelegateTaskRequest {
    pub description: String,
    pub workspace: Option<String>,
    pub file_context: Option<Vec<String>>,
}

/// Request body for updating agent configuration.
#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
    #[serde(rename = "costCapUsd")]
    pub cost_cap_usd: Option<f64>,
    #[serde(rename = "monthlyBudgetUsd")]
    pub monthly_budget_usd: Option<f64>,
    #[serde(rename = "timeoutSecs")]
    pub timeout_secs: Option<u64>,
    pub workspaces: Option<Vec<String>>,
    pub alias: Option<String>,
}

/// Serializable agent summary for list responses.
#[derive(Debug, Serialize)]
pub struct AgentSummary {
    pub id: String,
    #[serde(rename = "backendType")]
    pub backend_type: String,
    pub status: serde_json::Value,
    pub endpoint: String,
    pub alias: Option<String>,
    #[serde(rename = "lastSuccessfulTask")]
    pub last_successful_task: Option<String>,
}

/// Serializable agent detail response.
#[derive(Debug, Serialize)]
pub struct AgentDetail {
    pub id: String,
    #[serde(rename = "backendType")]
    pub backend_type: String,
    pub status: serde_json::Value,
    pub endpoint: String,
    pub alias: Option<String>,
    pub workspaces: Vec<String>,
    #[serde(rename = "timeoutSecs")]
    pub timeout_secs: Option<u64>,
    #[serde(rename = "costCapUsd")]
    pub cost_cap_usd: Option<f64>,
    #[serde(rename = "monthlyBudgetUsd")]
    pub monthly_budget_usd: Option<f64>,
    #[serde(rename = "lastSuccessfulTask")]
    pub last_successful_task: Option<String>,
}

// ── Handlers ───────────────────────────────────────────────────────

/// GET /ui/api/coding-agents — list all registered agents with status.
pub(crate) async fn list_coding_agents(
    State(state): State<Arc<ControlPanelState>>,
) -> Json<serde_json::Value> {
    let Some(ca_state) = state.coding_agent_state.as_ref() else {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Coding agent subsystem is not enabled"
        }));
    };

    let agents = ca_state.registry.list_agents();
    let summaries: Vec<AgentSummary> = agents
        .into_iter()
        .map(|agent| AgentSummary {
            id: agent.id,
            backend_type: agent.backend_type,
            status: serde_json::to_value(&agent.status).unwrap_or_default(),
            endpoint: agent.endpoint,
            alias: agent.config.alias,
            last_successful_task: agent.last_successful_task.map(|t| t.to_rfc3339()),
        })
        .collect();

    Json(serde_json::json!({
        "ok": true,
        "data": summaries
    }))
}

/// GET /ui/api/coding-agents/:id — get agent details.
pub(crate) async fn get_coding_agent(
    State(state): State<Arc<ControlPanelState>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let Some(ca_state) = state.coding_agent_state.as_ref() else {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Coding agent subsystem is not enabled"
        }));
    };

    let Some(agent) = ca_state.registry.get_agent(&id) else {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Agent '{}' not found", id)
        }));
    };

    let detail = AgentDetail {
        id: agent.id,
        backend_type: agent.backend_type,
        status: serde_json::to_value(&agent.status).unwrap_or_default(),
        endpoint: agent.endpoint,
        alias: agent.config.alias,
        workspaces: agent
            .config
            .workspaces
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        timeout_secs: agent.config.timeout_secs,
        cost_cap_usd: agent.config.cost_cap_usd,
        monthly_budget_usd: agent.config.monthly_budget_usd,
        last_successful_task: agent.last_successful_task.map(|t| t.to_rfc3339()),
    };

    Json(serde_json::json!({
        "ok": true,
        "data": detail
    }))
}

/// POST /ui/api/coding-agents — register a new agent (from onboarding wizard).
pub(crate) async fn register_coding_agent(
    State(state): State<Arc<ControlPanelState>>,
    Json(payload): Json<RegisterAgentRequest>,
) -> Json<serde_json::Value> {
    let Some(ca_state) = state.coding_agent_state.as_ref() else {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Coding agent subsystem is not enabled"
        }));
    };

    let config = CodingAgentInstanceConfig {
        id: payload.id.clone(),
        backend_type: payload.backend_type,
        endpoint: payload.endpoint,
        workspaces: payload
            .workspaces
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect(),
        timeout_secs: payload.timeout_secs,
        cost_cap_usd: payload.cost_cap_usd,
        monthly_budget_usd: payload.monthly_budget_usd,
        alias: payload.alias,
        auth: None,
    };

    match ca_state.registry.register_agent(config) {
        Ok(()) => Json(serde_json::json!({
            "ok": true,
            "message": format!("Agent '{}' registered successfully", payload.id)
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to register agent: {}", e)
        })),
    }
}

/// DELETE /ui/api/coding-agents/:id — unregister an agent.
pub(crate) async fn unregister_coding_agent(
    State(state): State<Arc<ControlPanelState>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let Some(ca_state) = state.coding_agent_state.as_ref() else {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Coding agent subsystem is not enabled"
        }));
    };

    match ca_state.registry.unregister_agent(&id) {
        Ok(_) => Json(serde_json::json!({
            "ok": true,
            "message": format!("Agent '{}' unregistered successfully", id)
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to unregister agent: {}", e)
        })),
    }
}

/// GET /ui/api/coding-agents/:id/tasks — get task history for agent (paginated, default 50).
pub(crate) async fn get_agent_tasks(
    State(state): State<Arc<ControlPanelState>>,
    Path(id): Path<String>,
    Query(query): Query<TaskHistoryQuery>,
) -> Json<serde_json::Value> {
    let Some(ca_state) = state.coding_agent_state.as_ref() else {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Coding agent subsystem is not enabled"
        }));
    };

    // Verify agent exists
    if ca_state.registry.get_agent(&id).is_none() {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Agent '{}' not found", id)
        }));
    }

    let limit = query.limit.unwrap_or(50);
    let tasks = ca_state.task_history.get_recent(&id, limit);

    let task_entries: Vec<serde_json::Value> = tasks
        .into_iter()
        .map(|entry| {
            serde_json::json!({
                "taskId": entry.task_id,
                "description": entry.description,
                "trigger": entry.trigger,
                "state": entry.state,
                "workspace": entry.workspace.display().to_string(),
                "createdAt": entry.created_at.to_rfc3339(),
            })
        })
        .collect();

    Json(serde_json::json!({
        "ok": true,
        "data": task_entries
    }))
}

/// GET /ui/api/coding-agents/:id/tasks/:task_id — get task detail view.
pub(crate) async fn get_agent_task_detail(
    State(state): State<Arc<ControlPanelState>>,
    Path((id, task_id)): Path<(String, String)>,
) -> Json<serde_json::Value> {
    let Some(ca_state) = state.coding_agent_state.as_ref() else {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Coding agent subsystem is not enabled"
        }));
    };

    // Verify agent exists
    if ca_state.registry.get_agent(&id).is_none() {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Agent '{}' not found", id)
        }));
    }

    let Some(task) = ca_state.task_history.get_task(&task_id) else {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Task '{}' not found", task_id)
        }));
    };

    // Verify the task belongs to the specified agent
    if task.agent_id != id {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Task '{}' does not belong to agent '{}'", task_id, id)
        }));
    }

    Json(serde_json::json!({
        "ok": true,
        "data": {
            "taskId": task.task_id,
            "agentId": task.agent_id,
            "description": task.description,
            "trigger": task.trigger,
            "state": task.state,
            "workspace": task.workspace.display().to_string(),
            "createdAt": task.created_at.to_rfc3339(),
        }
    }))
}

/// POST /ui/api/coding-agents/:id/tasks — delegate a task from UI.
pub(crate) async fn delegate_task(
    State(state): State<Arc<ControlPanelState>>,
    Path(id): Path<String>,
    Json(payload): Json<DelegateTaskRequest>,
) -> Json<serde_json::Value> {
    let Some(ca_state) = state.coding_agent_state.as_ref() else {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Coding agent subsystem is not enabled"
        }));
    };

    let task_request = TaskRequest {
        description: payload.description,
        trigger: TaskTrigger::ControlPanel {
            user_id: "ui-user".to_string(),
        },
        workspace: payload.workspace.map(std::path::PathBuf::from),
        file_context: payload
            .file_context
            .map(|paths| paths.into_iter().map(std::path::PathBuf::from).collect()),
        reply_to: ReplyTarget {
            channel_type: "control_panel".to_string(),
            channel_id: "ui".to_string(),
            message_id: None,
        },
    };

    match ca_state.delegator.delegate(&id, task_request).await {
        Ok(task_id) => Json(serde_json::json!({
            "ok": true,
            "data": {
                "taskId": task_id
            },
            "message": format!("Task delegated to agent '{}'", id)
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to delegate task: {}", e)
        })),
    }
}

/// POST /ui/api/coding-agents/:id/tasks/:task_id/cancel — cancel a running task.
pub(crate) async fn cancel_task(
    State(state): State<Arc<ControlPanelState>>,
    Path((_id, task_id)): Path<(String, String)>,
) -> Json<serde_json::Value> {
    let Some(ca_state) = state.coding_agent_state.as_ref() else {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Coding agent subsystem is not enabled"
        }));
    };

    match ca_state.delegator.cancel_task(&task_id).await {
        Ok(()) => Json(serde_json::json!({
            "ok": true,
            "message": format!("Task '{}' cancelled", task_id)
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to cancel task: {}", e)
        })),
    }
}

/// GET /ui/api/coding-agents/:id/costs — get cost statistics for agent.
pub(crate) async fn get_agent_costs(
    State(state): State<Arc<ControlPanelState>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let Some(ca_state) = state.coding_agent_state.as_ref() else {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Coding agent subsystem is not enabled"
        }));
    };

    // Verify agent exists
    if ca_state.registry.get_agent(&id).is_none() {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Agent '{}' not found", id)
        }));
    }

    let stats = ca_state.cost_tracker.get_agent_stats(&id);

    match stats {
        Some(record) => Json(serde_json::json!({
            "ok": true,
            "data": {
                "agentId": record.agent_id,
                "totalInputTokens": record.total_input_tokens,
                "totalOutputTokens": record.total_output_tokens,
                "estimatedTotalCostUsd": record.estimated_total_cost_usd,
                "taskCount": record.task_count,
                "averageCostPerTask": if record.task_count > 0 {
                    record.estimated_total_cost_usd / record.task_count as f64
                } else {
                    0.0
                },
                "periodStart": record.period_start.to_rfc3339(),
            }
        })),
        None => Json(serde_json::json!({
            "ok": true,
            "data": {
                "agentId": id,
                "totalInputTokens": 0,
                "totalOutputTokens": 0,
                "estimatedTotalCostUsd": 0.0,
                "taskCount": 0,
                "averageCostPerTask": 0.0,
                "periodStart": null,
            }
        })),
    }
}

/// PUT /ui/api/coding-agents/:id/config — update agent configuration.
pub(crate) async fn update_agent_config(
    State(state): State<Arc<ControlPanelState>>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateConfigRequest>,
) -> Json<serde_json::Value> {
    let Some(ca_state) = state.coding_agent_state.as_ref() else {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Coding agent subsystem is not enabled"
        }));
    };

    // Get the current agent to verify it exists
    let Some(current_agent) = ca_state.registry.get_agent(&id) else {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Agent '{}' not found", id)
        }));
    };

    // Build updated config by merging with existing values
    let updated_config = CodingAgentInstanceConfig {
        id: current_agent.config.id.clone(),
        backend_type: current_agent.config.backend_type.clone(),
        endpoint: current_agent.config.endpoint.clone(),
        workspaces: payload
            .workspaces
            .map(|ws| ws.into_iter().map(std::path::PathBuf::from).collect())
            .unwrap_or(current_agent.config.workspaces.clone()),
        timeout_secs: payload.timeout_secs.or(current_agent.config.timeout_secs),
        cost_cap_usd: payload.cost_cap_usd.or(current_agent.config.cost_cap_usd),
        monthly_budget_usd: payload
            .monthly_budget_usd
            .or(current_agent.config.monthly_budget_usd),
        alias: payload.alias.or(current_agent.config.alias.clone()),
        auth: current_agent.config.auth.clone(),
    };

    // Unregister and re-register with updated config
    if let Err(e) = ca_state.registry.unregister_agent(&id) {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to update agent config: {}", e)
        }));
    }

    match ca_state.registry.register_agent(updated_config) {
        Ok(()) => Json(serde_json::json!({
            "ok": true,
            "message": format!("Agent '{}' configuration updated", id)
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to re-register agent with updated config: {}", e)
        })),
    }
}
