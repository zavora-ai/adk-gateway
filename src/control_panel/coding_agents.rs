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
    pub history_db: Arc<crate::coding_agent::history_db::PersistentTaskHistory>,
    pub session_pool: Arc<crate::coding_agent::acp_client::AcpSessionPool>,
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
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub transport: Option<crate::coding_agent::config::AgentTransport>,
    pub workspaces: Vec<String>,
    #[serde(rename = "timeoutSecs")]
    pub timeout_secs: Option<u64>,
    #[serde(rename = "costCapUsd")]
    pub cost_cap_usd: Option<f64>,
    #[serde(rename = "monthlyBudgetUsd")]
    pub monthly_budget_usd: Option<f64>,
    pub alias: Option<String>,
    pub auth: Option<crate::coding_agent::config::AgentAuthConfig>,
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
        transport: payload.transport,
        workspaces: payload
            .workspaces
            .into_iter()
            .map(std::path::PathBuf::from)
            .collect(),
        timeout_secs: payload.timeout_secs,
        cost_cap_usd: payload.cost_cap_usd,
        monthly_budget_usd: payload.monthly_budget_usd,
        alias: payload.alias,
        auth: payload.auth,
    };

    match ca_state.registry.register_agent(config.clone()) {
        Ok(()) => {
            // Persist to config file so agent survives restarts
            if let Some(config_path) = &state.config_path {
                if let Err(e) = persist_agent_to_config(config_path, &config) {
                    tracing::warn!(
                        agent_id = %payload.id,
                        error = %e,
                        "Agent registered in memory but config persistence failed"
                    );
                    return Json(serde_json::json!({
                        "ok": true,
                        "message": format!("Agent '{}' registered but may not persist across restarts: {}", payload.id, e)
                    }));
                }
            }
            Json(serde_json::json!({
                "ok": true,
                "message": format!("Agent '{}' registered successfully", payload.id)
            }))
        }
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
        Ok(_) => {
            // Also remove from config file so it doesn't come back on restart
            if let Some(config_path) = &state.config_path {
                if let Err(e) = remove_agent_from_config(config_path, &id) {
                    tracing::warn!(agent_id = %id, error = %e, "Agent unregistered but config persistence failed");
                }
            }
            Json(serde_json::json!({
                "ok": true,
                "message": format!("Agent '{}' unregistered successfully", id)
            }))
        }
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
    // Try persistent DB first, fall back to in-memory
    let tasks = {
        let db_tasks = ca_state.history_db.get_recent(&id, limit);
        if db_tasks.is_empty() {
            ca_state.task_history.get_recent(&id, limit)
        } else {
            db_tasks
        }
    };

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

    let Some(task) = ca_state.history_db.get_task(&task_id).or_else(|| ca_state.task_history.get_task(&task_id)) else {
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
        transport: current_agent.config.transport.clone(),
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

// ── Connect / Disconnect ───────────────────────────────────────────

/// POST /ui/api/coding-agents/:id/connect — start an ACP session for the agent.
pub(crate) async fn connect_agent(
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

    // Check if agent has stdio transport
    if agent.config.transport.is_none() {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Agent has no stdio transport configured. Cannot connect."
        }));
    }

    // Try to create/get a session (this spawns the process)
    match ca_state.session_pool.get_or_create(&id, &agent.config).await {
        Ok(_) => Json(serde_json::json!({
            "ok": true,
            "message": format!("Agent '{}' connected successfully", id)
        })),
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to connect agent '{}': {:?}", id, e)
        })),
    }
}

/// POST /ui/api/coding-agents/:id/disconnect — close the ACP session for the agent.
pub(crate) async fn disconnect_agent(
    State(state): State<Arc<ControlPanelState>>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let Some(ca_state) = state.coding_agent_state.as_ref() else {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Coding agent subsystem is not enabled"
        }));
    };

    if !ca_state.session_pool.has_session(&id) {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Agent '{}' has no active session", id)
        }));
    }

    // Close the session (kills the process)
    ca_state.session_pool.close_session(&id).await;

    // Update status
    let _ = ca_state.registry.update_status(
        &id,
        crate::coding_agent::status::AgentConnectionStatus::Disconnected {
            since: chrono::Utc::now(),
        },
    );

    Json(serde_json::json!({
        "ok": true,
        "message": format!("Agent '{}' disconnected", id)
    }))
}

// ── Persistence Helper ─────────────────────────────────────────────

/// Persist a new agent configuration to the gateway config file.
///
/// Reads the existing config JSON, appends the agent to `codingAgents.agents[]`,
/// and writes it back. This ensures agents survive gateway restarts.
fn persist_agent_to_config(
    config_path: &std::path::Path,
    agent_config: &CodingAgentInstanceConfig,
) -> Result<(), String> {
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| format!("Failed to read config file: {}", e))?;

    let mut config_value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("Failed to parse config file: {}", e))?;

    // Ensure codingAgents.agents array exists
    let coding_agents = config_value
        .as_object_mut()
        .ok_or("Config is not a JSON object")?
        .entry("codingAgents")
        .or_insert_with(|| serde_json::json!({"enabled": true, "agents": []}));

    let agents_arr = coding_agents
        .as_object_mut()
        .ok_or("codingAgents is not an object")?
        .entry("agents")
        .or_insert_with(|| serde_json::json!([]));

    let arr = agents_arr
        .as_array_mut()
        .ok_or("codingAgents.agents is not an array")?;

    // Remove existing agent with same ID (in case of re-registration)
    arr.retain(|v| v.get("id").and_then(|id| id.as_str()) != Some(&agent_config.id));

    // Serialize and append the new agent config
    let agent_value = serde_json::to_value(agent_config)
        .map_err(|e| format!("Failed to serialize agent config: {}", e))?;
    arr.push(agent_value);

    // Write back
    let output = serde_json::to_string_pretty(&config_value)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;

    std::fs::write(config_path, output)
        .map_err(|e| format!("Failed to write config file: {}", e))?;

    tracing::info!(agent_id = %agent_config.id, "Agent config persisted to disk");
    Ok(())
}

/// Remove an agent from the persisted config file.
fn remove_agent_from_config(
    config_path: &std::path::Path,
    agent_id: &str,
) -> Result<(), String> {
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| format!("Failed to read config file: {}", e))?;

    let mut config_value: serde_json::Value = serde_json::from_str(&raw)
        .map_err(|e| format!("Failed to parse config file: {}", e))?;

    let Some(coding_agents) = config_value.get_mut("codingAgents") else {
        return Ok(()); // No codingAgents section — nothing to remove
    };

    let Some(agents_arr) = coding_agents.get_mut("agents").and_then(|v| v.as_array_mut()) else {
        return Ok(()); // No agents array
    };

    let before = agents_arr.len();
    agents_arr.retain(|v| v.get("id").and_then(|id| id.as_str()) != Some(agent_id));

    if agents_arr.len() == before {
        return Ok(()); // Agent wasn't in config
    }

    let output = serde_json::to_string_pretty(&config_value)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;

    std::fs::write(config_path, output)
        .map_err(|e| format!("Failed to write config file: {}", e))?;

    tracing::info!(agent_id = %agent_id, "Agent removed from config file");
    Ok(())
}
