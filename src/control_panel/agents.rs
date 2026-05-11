//! Agents management CRUD API handlers.

use super::settings::SettingsResponse;
use super::{ControlPanelState, LogEntry};
use std::sync::Arc;

/// Log an agent management operation to both the audit sink and the agent's log buffer.
pub(crate) async fn audit_agent_op(
    state: &ControlPanelState,
    agent_id: &str,
    action: &str,
    outcome: &str,
    details: Option<&str>,
) {
    let timestamp = chrono::Utc::now();

    state.push_agent_log(
        agent_id,
        LogEntry {
            timestamp: timestamp.to_rfc3339(),
            level: if outcome == "error" {
                "ERROR".into()
            } else {
                "INFO".into()
            },
            message: format!(
                "[{}] {} — {}{}",
                action,
                outcome,
                agent_id,
                details.map(|d| format!(": {}", d)).unwrap_or_default()
            ),
            target: Some("agent_management".into()),
        },
    );

    state.push_log(LogEntry {
        timestamp: timestamp.to_rfc3339(),
        level: if outcome == "error" {
            "ERROR".into()
        } else {
            "INFO".into()
        },
        message: format!(
            "agent.{}: {} — {}{}",
            action,
            outcome,
            agent_id,
            details.map(|d| format!(": {}", d)).unwrap_or_default()
        ),
        target: Some("agent_management".into()),
    });

    if let Some(ref sink) = state.audit_sink {
        let event = crate::audit::AuditEvent {
            timestamp,
            user_id: "system".into(),
            session_id: None,
            channel_type: None,
            event_type: crate::audit::AuditEventType::AgentAccess,
            resource: format!("agent:{}", agent_id),
            outcome: match outcome {
                "allowed" | "success" => crate::audit::AuditOutcome::Allowed,
                "denied" => crate::audit::AuditOutcome::Denied,
                _ => crate::audit::AuditOutcome::Error,
            },
            details: Some(format!(
                "action={}{}",
                action,
                details.map(|d| format!(", {}", d)).unwrap_or_default()
            )),
        };
        if let Err(e) = sink.log_event(event).await {
            tracing::warn!(error = %e, "failed to write audit event");
        }
    }
}

// ── Agent API endpoints ────────────────────────────────────────────

pub(crate) async fn api_agents_list(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let agents = state
        .agent_registry
        .as_ref()
        .map(|r| r.list())
        .unwrap_or_default();

    let list: Vec<serde_json::Value> = agents
        .iter()
        .map(|(id, record)| {
            serde_json::json!({
                "id": id,
                "name": record.config.name,
                "description": record.config.description,
                "agent_type": format!("{:?}", record.config.agent_type),
                "state": format!("{:?}", record.state),
                "port": record.port,
                "model": record.config.model,
                "tools": record.config.tools,
                "instruction": record.config.instruction,
                "api_key_env": record.config.api_key_env,
                "auto_start": record.config.auto_start,
                "channel_bindings": record.config.channel_bindings.iter().map(|b| {
                    serde_json::json!({
                        "channel_type": b.channel_type,
                        "account_id": b.account_id,
                    })
                }).collect::<Vec<_>>(),
                "created_at": record.created_at.to_rfc3339(),
                "updated_at": record.updated_at.to_rfc3339(),
            })
        })
        .collect();

    axum::Json(serde_json::json!(list))
}

#[derive(serde::Deserialize)]
pub(crate) struct CreateAgentPayload {
    name: String,
    model: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    api_key_env: String,
    #[serde(default)]
    instruction: String,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    channel_bindings: Vec<CreateAgentBinding>,
    #[serde(default = "default_agent_type")]
    agent_type: String,
    #[serde(default)]
    auto_start: bool,
}

fn default_agent_type() -> String {
    "Llm".to_string()
}

#[derive(serde::Deserialize)]
pub(crate) struct CreateAgentBinding {
    channel_type: String,
    account_id: Option<String>,
}

pub(crate) async fn api_agents_create(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<CreateAgentPayload>,
) -> axum::Json<SettingsResponse> {
    let registry = match &state.agent_registry {
        Some(r) => r,
        None => {
            return axum::Json(SettingsResponse {
                ok: false,
                message: "Agent registry not available".into(),
            });
        }
    };

    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return axum::Json(SettingsResponse {
            ok: false,
            message: "Agent name is required".into(),
        });
    }

    if payload.model.trim().is_empty() {
        return axum::Json(SettingsResponse {
            ok: false,
            message: "Model is required".into(),
        });
    }

    if name.len() > 128 {
        return axum::Json(SettingsResponse {
            ok: false,
            message: "Agent name must be 128 characters or fewer".into(),
        });
    }

    let id = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>();

    let payload_model = payload.model.clone();
    let config = crate::agent_config::AgentConfig {
        id: id.clone(),
        name,
        description: payload.description,
        agent_type: match payload.agent_type.as_str() {
            "Sequential" => crate::agent_config::AgentType::Sequential,
            "Parallel" => crate::agent_config::AgentType::Parallel,
            "Loop" => crate::agent_config::AgentType::Loop,
            "Router" => crate::agent_config::AgentType::Router,
            "Graph" => crate::agent_config::AgentType::Graph,
            _ => crate::agent_config::AgentType::Llm,
        },
        model: payload.model,
        api_key_env: payload.api_key_env,
        instruction: payload.instruction,
        tools: payload.tools.clone(),
        action_nodes: vec![],
        workflow_edges: vec![],
        sub_agents: vec![],
        role: crate::agent_config::AgentRoleConfig {
            allow: payload.tools,
            deny: vec![],
        },
        channel_bindings: payload
            .channel_bindings
            .into_iter()
            .map(|b| crate::agent_config::ChannelBinding {
                channel_type: b.channel_type,
                account_id: b.account_id,
                peer_filter: None,
            })
            .collect(),
        auto_start: payload.auto_start,
        temperature: None,
        max_output_tokens: None,
        model_override: None,
    };

    match registry.create_agent(config) {
        Ok(agent_id) => {
            tracing::info!(agent_id = %agent_id, "agent created via UI");
            audit_agent_op(
                &state,
                &agent_id,
                "create",
                "success",
                Some(&format!("model={}", payload_model)),
            )
            .await;
            axum::Json(SettingsResponse {
                ok: true,
                message: format!("Agent '{}' created.", agent_id),
            })
        }
        Err(e) => {
            audit_agent_op(&state, &id, "create", "error", Some(&e.to_string())).await;
            axum::Json(SettingsResponse {
                ok: false,
                message: format!("Failed to create agent: {e}"),
            })
        }
    }
}

pub(crate) async fn api_agents_start(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> axum::Json<SettingsResponse> {
    let registry = match &state.agent_registry {
        Some(r) => r,
        None => {
            return axum::Json(SettingsResponse {
                ok: false,
                message: "Agent registry not available".into(),
            });
        }
    };

    match registry.transition(&id, crate::agent_config::LifecycleState::Starting) {
        Ok(()) => {
            tracing::info!(agent_id = %id, "agent start requested via UI");
            audit_agent_op(
                &state,
                &id,
                "start",
                "success",
                Some("transitioning to Starting"),
            )
            .await;

            let msg = format!(
                "Agent '{}' set to Starting. Use the System Agent's agent_start tool to complete the build/spawn pipeline, or configure auto_start: true.",
                id
            );
            audit_agent_op(&state, &id, "start", "success", Some("state=Starting")).await;
            axum::Json(SettingsResponse {
                ok: true,
                message: msg,
            })
        }
        Err(e) => {
            audit_agent_op(&state, &id, "start", "error", Some(&e.to_string())).await;
            axum::Json(SettingsResponse {
                ok: false,
                message: format!("Failed to start agent: {e}"),
            })
        }
    }
}

pub(crate) async fn api_agents_stop(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> axum::Json<SettingsResponse> {
    let registry = match &state.agent_registry {
        Some(r) => r,
        None => {
            return axum::Json(SettingsResponse {
                ok: false,
                message: "Agent registry not available".into(),
            });
        }
    };

    match registry.transition(&id, crate::agent_config::LifecycleState::Stopping) {
        Ok(()) => {
            let _ = registry.transition(&id, crate::agent_config::LifecycleState::Stopped);
            tracing::info!(agent_id = %id, "agent stopped via UI");
            audit_agent_op(&state, &id, "stop", "success", Some("state=Stopped")).await;
            axum::Json(SettingsResponse {
                ok: true,
                message: format!("Agent '{}' stopped.", id),
            })
        }
        Err(e) => {
            audit_agent_op(&state, &id, "stop", "error", Some(&e.to_string())).await;
            axum::Json(SettingsResponse {
                ok: false,
                message: format!("Failed to stop agent: {e}"),
            })
        }
    }
}

pub(crate) async fn api_agents_delete(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> axum::Json<SettingsResponse> {
    let registry = match &state.agent_registry {
        Some(r) => r,
        None => {
            return axum::Json(SettingsResponse {
                ok: false,
                message: "Agent registry not available".into(),
            });
        }
    };

    match registry.delete(&id) {
        Ok(_record) => {
            tracing::info!(agent_id = %id, "agent deleted via UI");
            audit_agent_op(&state, &id, "delete", "success", Some("workspace archived")).await;
            axum::Json(SettingsResponse {
                ok: true,
                message: format!("Agent '{}' deleted and workspace archived.", id),
            })
        }
        Err(e) => {
            audit_agent_op(&state, &id, "delete", "error", Some(&e.to_string())).await;
            axum::Json(SettingsResponse {
                ok: false,
                message: format!("Failed to delete agent: {e}"),
            })
        }
    }
}

/// GET /ui/api/agents/{id}/logs — return recent logs for a specific agent.
pub(crate) async fn api_agents_logs(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> axum::Json<serde_json::Value> {
    let logs = state.agent_logs(&id, 100);
    let entries: Vec<serde_json::Value> = logs
        .iter()
        .map(|l| {
            serde_json::json!({
                "timestamp": l.timestamp,
                "level": l.level,
                "message": l.message,
            })
        })
        .collect();
    axum::Json(serde_json::json!({ "agent_id": id, "logs": entries }))
}

// ── Agent Configure endpoint ───────────────────────────────────────

#[derive(serde::Deserialize)]
pub(crate) struct ConfigureAgentPayload {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    instruction: Option<String>,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default)]
    channel_bindings: Option<String>,
    #[serde(default)]
    auto_start: Option<bool>,
}

/// PUT /ui/api/agents/{id}/configure — update an agent's configuration.
pub(crate) async fn api_agents_configure(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::Json(payload): axum::Json<ConfigureAgentPayload>,
) -> axum::Json<SettingsResponse> {
    let registry = match &state.agent_registry {
        Some(r) => r,
        None => {
            return axum::Json(SettingsResponse {
                ok: false,
                message: "Agent registry not available".into(),
            });
        }
    };

    let record = match registry.get(&id) {
        Some(r) => r.clone(),
        None => {
            return axum::Json(SettingsResponse {
                ok: false,
                message: format!("Agent '{}' not found", id),
            });
        }
    };

    let mut config = record.config.clone();

    if let Some(model) = payload.model {
        if model.trim().is_empty() {
            return axum::Json(SettingsResponse {
                ok: false,
                message: "Model cannot be empty".into(),
            });
        }
        config.model = model;
    }

    if let Some(instruction) = payload.instruction {
        config.instruction = instruction;
    }

    if let Some(tools) = payload.tools {
        config.tools = tools.clone();
        config.role.allow = tools;
    }

    if let Some(auto_start) = payload.auto_start {
        config.auto_start = auto_start;
    }

    if let Some(bindings_str) = payload.channel_bindings {
        let bindings: Vec<crate::agent_config::ChannelBinding> = bindings_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| {
                let parts: Vec<&str> = s.splitn(2, ':').collect();
                crate::agent_config::ChannelBinding {
                    channel_type: parts[0].to_string(),
                    account_id: parts.get(1).map(|a| a.to_string()),
                    peer_filter: None,
                }
            })
            .collect();
        config.channel_bindings = bindings;
    }

    match registry.update_config(&id, config) {
        Ok(()) => {
            tracing::info!(agent_id = %id, "agent configured via UI");
            audit_agent_op(&state, &id, "configure", "success", Some("config updated")).await;
            axum::Json(SettingsResponse {
                ok: true,
                message: format!("Agent '{}' configuration updated.", id),
            })
        }
        Err(e) => {
            audit_agent_op(&state, &id, "configure", "error", Some(&e.to_string())).await;
            axum::Json(SettingsResponse {
                ok: false,
                message: format!("Failed to configure agent: {e}"),
            })
        }
    }
}
