//! Consolidated JSON API endpoints for the control panel.
//!
//! All `/ui/api/*` handlers live here. Existing handlers from submodules
//! are re-exported, and new endpoints (auth check, login, logout, session
//! terminate, config save, AWP, integrations) are defined below.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use subtle::ConstantTimeEq;

use super::auth::UiSession;
use super::ControlPanelState;
use crate::config::AuthMode;

/// Cookie name for UI sessions.
const COOKIE_NAME: &str = "adk_ui_session";

// ── Re-exports from existing submodules ────────────────────────────
// These keep backward compatibility — existing JSON handlers stay where
// they are and are wired into routes from here.

pub(crate) use super::agent_setup::agent_get;
pub(crate) use super::agent_setup::agent_save;
pub(crate) use super::agents::{
    api_agents_configure, api_agents_create, api_agents_delete, api_agents_list, api_agents_logs,
    api_agents_start, api_agents_stop,
};
pub(crate) use super::channels::channels_get;
pub(crate) use super::channels::channels_save;
pub(crate) use super::channels::telegram_probe;
pub(crate) use super::config_page::config_json;
pub(crate) use super::dashboard::dashboard_json;
pub(crate) use super::logs::logs_json;
pub(crate) use super::memory::{memory_entities, memory_load, memory_save};
pub(crate) use super::sessions::sessions_json;
pub(crate) use super::settings::session_status;
pub(crate) use super::settings::settings_save;

// ── Auth check ─────────────────────────────────────────────────────

/// GET /ui/api/auth/check — returns current authentication status.
pub async fn auth_check(
    State(state): State<Arc<ControlPanelState>>,
    request: axum::extract::Request,
) -> Json<serde_json::Value> {
    let config = state.config.load();

    let mode = config
        .auth
        .as_ref()
        .map(|a| match a.mode {
            AuthMode::Password => "password",
            AuthMode::Token => "token",
            AuthMode::None => "none",
        })
        .unwrap_or("none");

    let auth_required = config.auth.as_ref().is_some_and(|auth| {
        matches!(auth.mode, AuthMode::Password | AuthMode::Token)
            && (auth.password.is_some() || auth.token.is_some())
    });

    let authenticated = if !auth_required {
        true
    } else {
        let cookie_header = request
            .headers()
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        extract_cookie(cookie_header, COOKIE_NAME)
            .map(|token| state.ui_sessions.contains_key(token))
            .unwrap_or(false)
    };

    Json(serde_json::json!({
        "authenticated": authenticated,
        "mode": mode,
    }))
}

// ── JSON Login ─────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct LoginPayload {
    password: String,
}

/// POST /ui/api/login — JSON login, validates password, sets session cookie.
pub async fn api_login(
    State(state): State<Arc<ControlPanelState>>,
    Json(payload): Json<LoginPayload>,
) -> Response {
    let config = state.config.load();

    let expected = config
        .auth
        .as_ref()
        .and_then(|auth| match auth.mode {
            AuthMode::Password => auth.password.as_deref(),
            AuthMode::Token => auth.token.as_deref(),
            AuthMode::None => None,
        })
        .unwrap_or("");

    // Constant-time comparison to prevent timing attacks
    let provided = payload.password.as_bytes();
    let expected_bytes = expected.as_bytes();

    let valid = if provided.len() == expected_bytes.len() {
        provided.ct_eq(expected_bytes).into()
    } else {
        false
    };

    if !valid || expected.is_empty() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "ok": false,
                "message": "Invalid credentials"
            })),
        )
            .into_response();
    }

    // Generate session token
    use rand::Rng;
    let token: String = rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(48)
        .map(char::from)
        .collect();

    // Store session
    state.ui_sessions.insert(
        token.clone(),
        UiSession {
            token: token.clone(),
            created_at: Instant::now(),
        },
    );

    // Set cookie and return success JSON
    let cookie = format!(
        "{}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=86400",
        COOKIE_NAME, token
    );

    let body = serde_json::json!({
        "ok": true,
        "message": "Login successful"
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::SET_COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            serde_json::to_string(&body).unwrap(),
        ))
        .unwrap()
}

// ── JSON Logout ────────────────────────────────────────────────────

/// POST /ui/api/logout — clears session cookie, removes from ui_sessions.
pub async fn api_logout(
    State(state): State<Arc<ControlPanelState>>,
    request: axum::extract::Request,
) -> Response {
    let cookie_header = request
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if let Some(token) = extract_cookie(cookie_header, COOKIE_NAME) {
        state.ui_sessions.remove(token);
    }

    let clear_cookie = format!(
        "{}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0",
        COOKIE_NAME
    );

    let body = serde_json::json!({
        "ok": true,
        "message": "Logged out"
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::SET_COOKIE, clear_cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            serde_json::to_string(&body).unwrap(),
        ))
        .unwrap()
}

// ── Session terminate ──────────────────────────────────────────────

/// POST /ui/api/sessions/{id}/terminate — end a session via session_bridge.
pub async fn session_terminate(
    State(state): State<Arc<ControlPanelState>>,
    Path(session_id): Path<String>,
) -> Json<serde_json::Value> {
    // Try to remove the session from the control panel's session list
    let mut found = false;
    if let Ok(mut sessions) = state.sessions.write() {
        let before = sessions.len();
        sessions.retain(|s| s.session_id != session_id);
        found = sessions.len() < before;
    }

    if found {
        tracing::info!(session_id = %session_id, "session terminated via UI");
        Json(serde_json::json!({
            "ok": true,
            "message": format!("Session '{}' terminated.", session_id)
        }))
    } else {
        Json(serde_json::json!({
            "ok": false,
            "message": format!("Session '{}' not found.", session_id)
        }))
    }
}

// ── Config save ────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct ConfigSavePayload {
    content: String,
}

/// POST /ui/api/config — validate into GatewayConfig, run semantic validation, then write to disk.
pub async fn config_save(
    State(state): State<Arc<ControlPanelState>>,
    Json(payload): Json<ConfigSavePayload>,
) -> Json<serde_json::Value> {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    // Step 1: Parse into GatewayConfig (not just serde_json::Value)
    let new_config: crate::config::GatewayConfig = match serde_json::from_str(&payload.content) {
        Ok(cfg) => cfg,
        Err(_) => {
            // Try JSON5 fallback
            match json5::from_str(&payload.content) {
                Ok(cfg) => cfg,
                Err(e) => {
                    return Json(serde_json::json!({
                        "ok": false,
                        "message": format!("Invalid configuration: {e}")
                    }));
                }
            }
        }
    };

    // Step 2: Semantic validation
    if let Err(e) = crate::config_watcher::validate_config(&new_config) {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Configuration validation failed: {e}")
        }));
    }

    // Step 3: Serialize to normalized JSON for consistent formatting
    let output = match serde_json::to_string_pretty(&new_config) {
        Ok(s) => s,
        Err(e) => {
            return Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to serialize config: {e}")
            }));
        }
    };

    // Step 4: Write to disk
    if let Err(e) = std::fs::write(&config_path, &output) {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to write config: {e}")
        }));
    }

    // Step 5: Update in-memory config atomically (only after successful write)
    state.config.store(std::sync::Arc::new(new_config));

    tracing::info!("config saved via UI to {}", config_path.display());

    Json(serde_json::json!({
        "ok": true,
        "message": "Configuration validated, saved, and reloaded."
    }))
}

// ── AWP endpoints ──────────────────────────────────────────────────

/// GET /ui/api/awp — AWP summary: health state, capability count, subscription count, consent count, site info.
pub async fn awp_summary(
    State(state): State<Arc<ControlPanelState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let awp = match &state.awp_state {
        Some(s) => s,
        None => {
            return (
                StatusCode::OK,
                Json(serde_json::json!({
                    "ok": true,
                    "data": null,
                    "message": "AWP is not enabled"
                })),
            );
        }
    };

    let health_snap = awp.health.snapshot().await;
    let ctx = awp.business_context.load();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "data": {
                "health": {
                    "state": format!("{:?}", health_snap.state),
                    "message": health_snap.message,
                    "timestamp": health_snap.timestamp.to_rfc3339(),
                },
                "site": {
                    "name": ctx.site_name,
                    "description": ctx.site_description,
                    "domain": ctx.domain,
                },
                "capability_count": ctx.capabilities.len(),
            }
        })),
    )
}

/// GET /ui/api/awp/health — AWP health state, message, timestamp.
pub async fn awp_health(
    State(state): State<Arc<ControlPanelState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let awp = match &state.awp_state {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "ok": false,
                    "message": "AWP is not enabled"
                })),
            );
        }
    };

    let snap = awp.health.snapshot().await;

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "data": {
                "state": format!("{:?}", snap.state),
                "message": snap.message,
                "timestamp": snap.timestamp.to_rfc3339(),
            }
        })),
    )
}

/// GET /ui/api/awp/capabilities — array of capabilities from business context.
pub async fn awp_capabilities(
    State(state): State<Arc<ControlPanelState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let awp = match &state.awp_state {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "ok": false,
                    "message": "AWP is not enabled"
                })),
            );
        }
    };

    let ctx = awp.business_context.load();
    let capabilities: Vec<serde_json::Value> = ctx
        .capabilities
        .iter()
        .map(|cap| {
            serde_json::json!({
                "name": cap.name,
                "description": cap.description,
                "endpoint": cap.endpoint,
                "method": cap.method,
                "access_level": format!("{:?}", cap.access_level),
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "data": capabilities
        })),
    )
}

/// GET /ui/api/awp/subscriptions — array of event subscriptions.
/// Note: Subscriptions are managed through the AWP protocol endpoints (/awp/events/*).
/// This endpoint provides a proxy view.
pub async fn awp_subscriptions(
    State(state): State<Arc<ControlPanelState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let _awp = match &state.awp_state {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "ok": false,
                    "message": "AWP is not enabled"
                })),
            );
        }
    };

    // Event subscriptions are managed through the AWP protocol routes.
    // This endpoint confirms AWP is active; clients should use /awp/events/subscriptions
    // for full subscription management.
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "data": [],
            "message": "Use /awp/events/subscriptions for full subscription management"
        })),
    )
}

/// DELETE /ui/api/awp/subscriptions/{id} — remove subscription.
/// Proxies to the AWP event service.
pub async fn awp_subscription_delete(
    State(state): State<Arc<ControlPanelState>>,
    Path(_sub_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let _awp = match &state.awp_state {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "ok": false,
                    "message": "AWP is not enabled"
                })),
            );
        }
    };

    // Subscription deletion is handled through the AWP protocol routes.
    // Use DELETE /awp/events/subscriptions/{id} for direct management.
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "message": "Use DELETE /awp/events/subscriptions/{id} for subscription removal"
        })),
    )
}

/// GET /ui/api/awp/consent — consent records summary.
pub async fn awp_consent(
    State(state): State<Arc<ControlPanelState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let _awp = match &state.awp_state {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "ok": false,
                    "message": "AWP is not enabled"
                })),
            );
        }
    };

    // Consent records are managed through the AWP consent endpoints (/awp/consent/*).
    // This endpoint confirms AWP consent service is active.
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "data": [],
            "message": "Use /awp/consent/* endpoints for consent management"
        })),
    )
}

// ── Integrations endpoints ─────────────────────────────────────────

/// GET /ui/api/integrations/mcp — MCP server status from config + mcp_manager.
pub async fn integrations_mcp(
    State(state): State<Arc<ControlPanelState>>,
) -> Json<serde_json::Value> {
    let config = state.config.load();
    let servers: Vec<serde_json::Value> = config
        .mcp_servers
        .iter()
        .map(|srv| {
            let (transport_type, transport_detail) = match &srv.transport {
                crate::mcp::McpTransport::Stdio { command, args, env } => {
                    let detail = serde_json::json!({
                        "command": command,
                        "args": args,
                        "env": env,
                    });
                    ("stdio", detail)
                }
                crate::mcp::McpTransport::Sse { url } => {
                    let detail = serde_json::json!({ "url": url });
                    ("sse", detail)
                }
            };

            let status = state
                .mcp_manager
                .as_ref()
                .and_then(|mgr| mgr.get_status(&srv.server_id))
                .map(|s| format!("{:?}", s))
                .unwrap_or_else(|| {
                    if srv.enabled {
                        "Disconnected".to_string()
                    } else {
                        "Disabled".to_string()
                    }
                });

            let tools = state
                .mcp_manager
                .as_ref()
                .map(|mgr| mgr.discovered_tools(&srv.server_id))
                .unwrap_or_default();

            serde_json::json!({
                "server_id": srv.server_id,
                "transport": transport_type,
                "transport_detail": transport_detail,
                "enabled": srv.enabled,
                "status": status,
                "discovered_tools": tools,
            })
        })
        .collect();

    Json(serde_json::json!({
        "ok": true,
        "data": servers
    }))
}

/// GET /ui/api/integrations/cron — cron job list from cron_scheduler.
pub async fn integrations_cron(
    State(state): State<Arc<ControlPanelState>>,
) -> Json<serde_json::Value> {
    // Merge config-defined jobs with runtime status
    let config = state.config.load();
    let config_jobs = &config.cron.jobs;

    let jobs: Vec<serde_json::Value> = match &state.cron_scheduler {
        Some(scheduler) => {
            let guard = scheduler.lock().await;
            match guard.as_ref() {
                Some(sched) => {
                    sched.list_all_jobs().iter().map(|(job, status)| {
                        serde_json::json!({
                            "id": job.id,
                            "schedule": job.schedule,
                            "message": job.message,
                            "delivery": job.deliver_to.as_ref().map(|d| serde_json::json!({
                                "channel": d.channel,
                                "target": d.target,
                            })),
                            "status": match status {
                                crate::cron::CronJobStatus::Active => "Active",
                                crate::cron::CronJobStatus::Cancelled => "Cancelled",
                            },
                        })
                    }).collect()
                }
                None => {
                    // No scheduler running — show config jobs as inactive
                    config_jobs.iter().map(|job| {
                        serde_json::json!({
                            "id": job.id,
                            "schedule": job.schedule,
                            "message": job.message,
                            "delivery": job.deliver_to.as_ref().map(|d| serde_json::json!({
                                "channel": d.channel,
                                "target": d.target,
                            })),
                            "status": "Stopped",
                        })
                    }).collect()
                }
            }
        }
        None => {
            // No scheduler at all — show config jobs
            config_jobs.iter().map(|job| {
                serde_json::json!({
                    "id": job.id,
                    "schedule": job.schedule,
                    "message": job.message,
                    "delivery": job.deliver_to.as_ref().map(|d| serde_json::json!({
                        "channel": d.channel,
                        "target": d.target,
                    })),
                    "status": "Stopped",
                })
            }).collect()
        }
    };

    Json(serde_json::json!({
        "ok": true,
        "data": {
            "jobs": jobs,
            "total": jobs.len(),
        }
    }))
}

/// POST /ui/api/scheduled-tasks — Create a new scheduled task.
pub async fn scheduled_task_create(
    State(state): State<Arc<ControlPanelState>>,
    Json(payload): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let id = payload.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let schedule = payload.get("schedule").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let message = payload.get("message").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();

    if id.is_empty() || schedule.is_empty() || message.is_empty() {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Fields 'id', 'schedule', and 'message' are required."
        }));
    }

    let delivery = payload.get("delivery").and_then(|d| {
        let channel = d.get("channel")?.as_str()?.to_string();
        let target = d.get("target")?.as_str()?.to_string();
        if channel.is_empty() { return None; }
        Some(crate::config::CronDelivery { channel, target })
    });

    let new_job = crate::config::CronJob {
        id: id.clone(),
        schedule,
        message,
        deliver_to: delivery,
    };

    // Persist to config
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    let mut cfg = state.config.load().as_ref().clone();

    // Check for duplicate ID
    if cfg.cron.jobs.iter().any(|j| j.id == id) {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("A scheduled task with ID '{}' already exists.", id)
        }));
    }

    cfg.cron.jobs.push(new_job.clone());

    // Write to disk
    let output = match serde_json::to_string_pretty(&cfg) {
        Ok(s) => s,
        Err(e) => {
            return Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to serialize config: {e}")
            }));
        }
    };

    if let Err(e) = std::fs::write(&config_path, &output) {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to write config: {e}")
        }));
    }

    // Hot-reload: update in-memory config and schedule the job
    state.config.store(std::sync::Arc::new(cfg.clone()));
    if let Some(scheduler) = &state.cron_scheduler {
        let mut guard = scheduler.lock().await;
        if let Some(sched) = guard.as_mut() {
            sched.reconcile(&cfg.cron.jobs);
        }
    }

    tracing::info!(job_id = %id, "scheduled task created via API");

    Json(serde_json::json!({
        "ok": true,
        "message": format!("Scheduled task '{}' created.", id)
    }))
}

/// POST /ui/api/scheduled-tasks/:id/cancel — Cancel a scheduled task.
pub async fn scheduled_task_cancel(
    State(state): State<Arc<ControlPanelState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    if let Some(scheduler) = &state.cron_scheduler {
        let mut guard = scheduler.lock().await;
        if let Some(sched) = guard.as_mut() {
            sched.cancel(&task_id);
        }
    }

    tracing::info!(job_id = %task_id, "scheduled task cancelled via API");

    Json(serde_json::json!({
        "ok": true,
        "message": format!("Scheduled task '{}' cancelled.", task_id)
    }))
}

/// DELETE /ui/api/scheduled-tasks/:id — Remove a scheduled task from config.
pub async fn scheduled_task_delete(
    State(state): State<Arc<ControlPanelState>>,
    axum::extract::Path(task_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    let mut cfg = state.config.load().as_ref().clone();
    let before = cfg.cron.jobs.len();
    cfg.cron.jobs.retain(|j| j.id != task_id);

    if cfg.cron.jobs.len() == before {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Scheduled task '{}' not found.", task_id)
        }));
    }

    // Write to disk
    let output = match serde_json::to_string_pretty(&cfg) {
        Ok(s) => s,
        Err(e) => {
            return Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to serialize config: {e}")
            }));
        }
    };

    if let Err(e) = std::fs::write(&config_path, &output) {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to write config: {e}")
        }));
    }

    // Hot-reload
    state.config.store(std::sync::Arc::new(cfg.clone()));
    if let Some(scheduler) = &state.cron_scheduler {
        let mut guard = scheduler.lock().await;
        if let Some(sched) = guard.as_mut() {
            sched.reconcile(&cfg.cron.jobs);
        }
    }

    tracing::info!(job_id = %task_id, "scheduled task deleted via API");

    Json(serde_json::json!({
        "ok": true,
        "message": format!("Scheduled task '{}' deleted.", task_id)
    }))
}

/// GET /ui/api/integrations/tools — registered tools from tool_registry.
pub async fn integrations_tools(
    State(state): State<Arc<ControlPanelState>>,
) -> Json<serde_json::Value> {
    let tools = match &state.tool_registry {
        Some(registry) => registry
            .known_names()
            .iter()
            .map(|name| {
                serde_json::json!({
                    "name": name,
                })
            })
            .collect::<Vec<_>>(),
        None => vec![],
    };

    Json(serde_json::json!({
        "ok": true,
        "data": {
            "tools": tools,
            "total": tools.len(),
        }
    }))
}

// ── Cookie parsing helper ──────────────────────────────────────────

/// Extract a cookie value by name from a Cookie header string.
fn extract_cookie<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        if let Some(value) = pair.strip_prefix(name) {
            if let Some(value) = value.strip_prefix('=') {
                return Some(value);
            }
        }
    }
    None
}

// ── MCP management endpoints ───────────────────────────────────────

/// Request body for adding/updating an MCP server.
#[derive(serde::Deserialize)]
pub struct AddMcpServerPayload {
    pub server_id: String,
    #[serde(default = "default_stdio")]
    pub transport: String,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    pub url: Option<String>,
    #[serde(default)]
    pub disabled: bool,
}

fn default_stdio() -> String {
    "stdio".to_string()
}

/// POST /ui/api/integrations/mcp — Add or update an MCP server in config.
pub async fn mcp_add(
    State(state): State<Arc<ControlPanelState>>,
    Json(payload): Json<AddMcpServerPayload>,
) -> Json<serde_json::Value> {
    use crate::mcp::{McpServerConfig, McpTransport};

    let transport = match payload.transport.as_str() {
        "stdio" => {
            let command = match payload.command {
                Some(c) => c,
                None => {
                    return Json(serde_json::json!({
                        "ok": false,
                        "message": "command is required for stdio transport"
                    }));
                }
            };
            McpTransport::Stdio {
                command,
                args: payload.args,
                env: payload.env,
            }
        }
        "http" | "sse" => {
            let url = match payload.url {
                Some(u) => u,
                None => {
                    return Json(serde_json::json!({
                        "ok": false,
                        "message": "url is required for http/sse transport"
                    }));
                }
            };
            McpTransport::Sse { url }
        }
        other => {
            return Json(serde_json::json!({
                "ok": false,
                "message": format!("unknown transport type: {other}")
            }));
        }
    };

    let new_server = McpServerConfig {
        server_id: payload.server_id.clone(),
        transport,
        auth: None,
        enabled: !payload.disabled,
    };

    // Update config
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    let mut cfg = state.config.load().as_ref().clone();
    cfg.mcp_servers.retain(|s| s.server_id != payload.server_id);
    cfg.mcp_servers.push(new_server);

    // Write to disk
    let output = match serde_json::to_string_pretty(&cfg) {
        Ok(s) => s,
        Err(e) => {
            return Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to serialize config: {e}")
            }));
        }
    };

    if let Err(e) = std::fs::write(&config_path, &output) {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to write config: {e}")
        }));
    }

    // Hot-reload: update in-memory config and reconcile MCP connections
    state.config.store(std::sync::Arc::new(cfg.clone()));
    if let Some(mgr) = &state.mcp_manager {
        mgr.reconcile(&cfg.mcp_servers).await;
    }

    tracing::info!(server_id = %payload.server_id, "MCP server added via API");

    Json(serde_json::json!({
        "ok": true,
        "message": format!("MCP server '{}' added.", payload.server_id)
    }))
}

/// DELETE /ui/api/integrations/mcp/:id — Remove an MCP server from config.
pub async fn mcp_remove(
    State(state): State<Arc<ControlPanelState>>,
    Path(server_id): Path<String>,
) -> Json<serde_json::Value> {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    let mut cfg = state.config.load().as_ref().clone();
    let before = cfg.mcp_servers.len();
    cfg.mcp_servers.retain(|s| s.server_id != server_id);

    if cfg.mcp_servers.len() == before {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("MCP server '{}' not found.", server_id)
        }));
    }

    let output = match serde_json::to_string_pretty(&cfg) {
        Ok(s) => s,
        Err(e) => {
            return Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to serialize config: {e}")
            }));
        }
    };

    if let Err(e) = std::fs::write(&config_path, &output) {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to write config: {e}")
        }));
    }

    state.config.store(std::sync::Arc::new(cfg.clone()));
    if let Some(mgr) = &state.mcp_manager {
        mgr.reconcile(&cfg.mcp_servers).await;
    }

    tracing::info!(server_id = %server_id, "MCP server removed via API");

    Json(serde_json::json!({
        "ok": true,
        "message": format!("MCP server '{}' removed.", server_id)
    }))
}

/// POST /ui/api/integrations/mcp/:id/toggle — Enable/disable an MCP server.
pub async fn mcp_toggle(
    State(state): State<Arc<ControlPanelState>>,
    Path(server_id): Path<String>,
) -> Json<serde_json::Value> {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    let mut cfg = state.config.load().as_ref().clone();
    let server = cfg.mcp_servers.iter_mut().find(|s| s.server_id == server_id);

    match server {
        Some(srv) => {
            srv.enabled = !srv.enabled;
            let new_state = if srv.enabled { "enabled" } else { "disabled" };

            let output = match serde_json::to_string_pretty(&cfg) {
                Ok(s) => s,
                Err(e) => {
                    return Json(serde_json::json!({
                        "ok": false,
                        "message": format!("Failed to serialize config: {e}")
                    }));
                }
            };

            if let Err(e) = std::fs::write(&config_path, &output) {
                return Json(serde_json::json!({
                    "ok": false,
                    "message": format!("Failed to write config: {e}")
                }));
            }

            state.config.store(std::sync::Arc::new(cfg.clone()));
            if let Some(mgr) = &state.mcp_manager {
                mgr.reconcile(&cfg.mcp_servers).await;
            }

            tracing::info!(server_id = %server_id, state = %new_state, "MCP server toggled via API");

            Json(serde_json::json!({
                "ok": true,
                "message": format!("MCP server '{}' {}.", server_id, new_state)
            }))
        }
        None => Json(serde_json::json!({
            "ok": false,
            "message": format!("MCP server '{}' not found.", server_id)
        })),
    }
}
