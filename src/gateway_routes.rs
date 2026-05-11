//! HTTP route handlers for the gateway server.

use crate::control_panel;
use crate::gateway_state::GatewayState;
use crate::jwt::jwt_auth_middleware;
use crate::metrics::{GatewayMetrics, MessageStatus};
use crate::session_bridge::SessionBridge;
use crate::webhook::{WebhookHandler, WebhookRequest, WebhookResponse};

use std::sync::Arc;

/// Build the full axum router with all endpoints.
pub fn build_router(
    state: &Arc<GatewayState>,
    webhook_handler: Arc<WebhookHandler>,
) -> axum::Router {
    let metrics = state.metrics.clone();
    let session_bridge = state.session_bridge.clone();
    let channel_map = state.channel_map.clone();
    let control_panel = state.control_panel.clone();

    // Unprotected routes — always accessible without JWT
    let unprotected = axum::Router::new()
        .route(
            "/health",
            axum::routing::get(health_handler(session_bridge.clone(), channel_map.clone())),
        )
        .route("/metrics", axum::routing::get(metrics_handler(metrics)));

    // Protected routes — JWT middleware applied when jwt_validator is configured
    let protected = axum::Router::new()
        .route(
            "/status",
            axum::routing::get(status_handler(state.clone(), webhook_handler.clone())),
        )
        .route(
            "/hooks/inbound",
            axum::routing::post(webhook_route(webhook_handler)),
        )
        .route(
            "/cron",
            axum::routing::get(cron_status_handler(state.clone())),
        )
        .route(
            "/cron/{job_id}/cancel",
            axum::routing::post(cron_cancel_handler(state.clone())),
        )
        .route(
            "/rag/search",
            axum::routing::post(rag_search_handler(state.clone())),
        )
        .route(
            "/rag/hybrid",
            axum::routing::post(rag_hybrid_handler(state.clone())),
        )
        .route(
            "/rag/filtered",
            axum::routing::post(rag_filtered_handler(state.clone())),
        )
        .route(
            "/rag/diagnostics",
            axum::routing::get(rag_diagnostics_handler(state.clone())),
        )
        .route(
            "/rag/ingest",
            axum::routing::post(rag_ingest_handler(state.clone())),
        )
        .route(
            "/pairing/generate",
            axum::routing::post(pairing_generate_handler(state.clone())),
        )
        .route(
            "/pairing/status",
            axum::routing::get(pairing_status_handler(state.clone())),
        )
        .merge(control_panel::build_routes(control_panel.clone()).with_state(control_panel));

    let protected = if let Some(ref validator) = state.jwt_validator {
        let jwt_layer =
            axum::middleware::from_fn_with_state(validator.clone(), jwt_auth_middleware);
        protected.layer(jwt_layer)
    } else {
        protected
    };

    // Apply global request body size limit (1 MiB) and timeout (60s)
    let router =
        crate::awp::merge_awp_routes(unprotected.merge(protected), state.awp_state.clone());

    router
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(60),
        ))
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
}

fn health_handler(
    sb: Arc<SessionBridge>,
    cm: Arc<dashmap::DashMap<crate::channel::ChannelKey, Arc<dyn crate::channel::Channel>>>,
) -> impl Clone
       + Fn() -> std::pin::Pin<
    Box<dyn std::future::Future<Output = axum::Json<serde_json::Value>> + Send>,
> {
    move || {
        let sb = sb.clone();
        let cm = cm.clone();
        Box::pin(async move {
            let mut channel_health = Vec::new();
            let channels: Vec<_> = cm
                .iter()
                .map(|entry| {
                    let channel = Arc::clone(entry.value());
                    let channel_name = channel.channel_type().to_string();
                    let account_id = channel.account_id().to_owned();
                    (channel, channel_name, account_id)
                })
                .collect();
            for (channel, channel_name, account_id) in channels {
                let health = match channel.health_check().await {
                    Ok(h) => serde_json::json!({
                        "channel": &channel_name,
                        "account_id": &account_id,
                        "status": h.status,
                        "last_connected": h.last_connected,
                        "reconnect_attempts": h.reconnect_attempts,
                        "error": h.error,
                    }),
                    Err(e) => serde_json::json!({
                        "channel": &channel_name,
                        "account_id": &account_id,
                        "status": "failed",
                        "error": e.to_string(),
                    }),
                };
                channel_health.push(health);
            }
            axum::Json(serde_json::json!({
                "status": "healthy",
                "active_sessions": sb.active_sessions().len(),
                "channels": channel_health,
            }))
        })
    }
}

fn status_handler(
    state: Arc<GatewayState>,
    webhook_handler: Arc<WebhookHandler>,
) -> impl Clone
       + Fn() -> std::pin::Pin<
    Box<dyn std::future::Future<Output = axum::Json<serde_json::Value>> + Send>,
> {
    move || {
        let state = state.clone();
        let webhook_handler = webhook_handler.clone();
        Box::pin(async move {
            // Per-channel health with metrics
            let mut channel_info = Vec::new();
            let channels: Vec<_> = state
                .channel_map
                .iter()
                .map(|entry| {
                    let channel = Arc::clone(entry.value());
                    let channel_name = channel.channel_type().to_string();
                    let account_id = channel.account_id().to_owned();
                    (channel, channel_name, account_id)
                })
                .collect();
            for (channel, channel_name, account_id) in channels {
                let health = match channel.health_check().await {
                    Ok(h) => serde_json::json!({
                        "channel": &channel_name,
                        "account_id": &account_id,
                        "status": h.status,
                        "last_connected": h.last_connected,
                        "reconnect_attempts": h.reconnect_attempts,
                        "error": h.error,
                    }),
                    Err(e) => serde_json::json!({
                        "channel": &channel_name,
                        "account_id": &account_id,
                        "status": "failed",
                        "error": e.to_string(),
                    }),
                };

                let messages_total_success = state
                    .metrics
                    .get_messages_total(&channel_name, &MessageStatus::Success);
                let messages_total_failure = state
                    .metrics
                    .get_messages_total(&channel_name, &MessageStatus::Failure);
                let error_rate = state.metrics.get_error_rate(&channel_name);

                channel_info.push(serde_json::json!({
                    "health": health,
                    "messages_total": {
                        "success": messages_total_success,
                        "failure": messages_total_failure,
                    },
                    "error_rate": error_rate,
                }));
            }

            // Pairing
            let paired_count = state.pairing_service.paired_count();

            // Plugins
            let plugin_count = state.plugin_manager.plugin_count();
            let plugin_names: Vec<String> = state
                .plugin_manager
                .plugin_names()
                .into_iter()
                .map(|s| s.to_string())
                .collect();

            // RAG
            let rag = state.rag_pipeline.as_ref().map(|rp| {
                let diag = rp.diagnostics();
                serde_json::json!({
                    "document_count": diag.document_count,
                    "chunk_count": diag.chunk_count,
                    "integrity_ok": diag.integrity_ok,
                })
            });

            // Skill index
            let skill_count = state.skill_index.len();
            let skill_names: Vec<String> =
                state.skill_index.all().map(|s| s.name.clone()).collect();

            // Session service diagnostic
            let active_sessions = state.session_bridge.active_sessions();
            let session_backend = format!("{:?}", state.config.load().session.backend);

            // Shutdown coordinator
            let in_flight_count = state.shutdown_coordinator.in_flight_count();

            // Known tool names from registry
            let known_tool_names = state.tool_registry.known_names();

            // Cron scheduler
            let cron_info = {
                let guard = state.cron_scheduler.lock().await;
                match guard.as_ref() {
                    Some(scheduler) => serde_json::json!({
                        "total_jobs": scheduler.job_count(),
                        "active_job_ids": scheduler.active_job_ids(),
                    }),
                    None => serde_json::json!({
                        "total_jobs": 0,
                        "active_job_ids": [],
                    }),
                }
            };

            // Access control diagnostics
            let access_control_info = {
                let ac = state.access_control.read().await;
                let role_names: Vec<String> = ac.role_names();
                let paired_sample: Vec<String> =
                    ac.paired_user_ids().into_iter().take(10).collect();
                serde_json::json!({
                    "role_count": role_names.len(),
                    "role_names": role_names,
                    "paired_user_sample": paired_sample,
                })
            };

            // Diagnostic paths
            let webhook_path = webhook_handler.path();
            let config_path = state.config_path.display().to_string();

            axum::Json(serde_json::json!({
                "status": "running",
                "active_sessions": active_sessions.len(),
                "channels": channel_info,
                "paired_count": paired_count,
                "plugin_count": plugin_count,
                "plugin_names": plugin_names,
                "rag": rag,
                "skills": {
                    "count": skill_count,
                    "names": skill_names,
                },
                "session_service": {
                    "backend": session_backend,
                    "active_count": active_sessions.len(),
                },
                "known_tool_names": known_tool_names,
                "in_flight_count": in_flight_count,
                "cron": cron_info,
                "access_control": access_control_info,
                "webhook_path": webhook_path,
                "config_path": config_path,
            }))
        })
    }
}

fn metrics_handler(
    metrics: Arc<GatewayMetrics>,
) -> impl Clone + Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>> {
    move || {
        let m = metrics.clone();
        Box::pin(async move { m.render_prometheus() })
    }
}

fn cron_status_handler(
    state: Arc<GatewayState>,
) -> impl Clone
       + Fn() -> std::pin::Pin<
    Box<dyn std::future::Future<Output = axum::Json<serde_json::Value>> + Send>,
> {
    move || {
        let state = state.clone();
        Box::pin(async move {
            let guard = state.cron_scheduler.lock().await;
            match guard.as_ref() {
                Some(scheduler) => {
                    let active_ids = scheduler.active_job_ids();
                    let job_statuses: Vec<serde_json::Value> = active_ids
                        .iter()
                        .map(|id| {
                            serde_json::json!({
                                "job_id": id,
                                "active": scheduler.is_active(id),
                            })
                        })
                        .collect();
                    axum::Json(serde_json::json!({
                        "total_jobs": scheduler.job_count(),
                        "active_job_ids": active_ids,
                        "jobs": job_statuses,
                    }))
                }
                None => axum::Json(serde_json::json!({
                    "total_jobs": 0,
                    "active_job_ids": [],
                    "jobs": [],
                })),
            }
        })
    }
}

fn cron_cancel_handler(
    state: Arc<GatewayState>,
) -> impl Clone
       + Fn(
    axum::extract::Path<String>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = (axum::http::StatusCode, axum::Json<serde_json::Value>)>
            + Send,
    >,
> {
    move |axum::extract::Path(job_id): axum::extract::Path<String>| {
        let state = state.clone();
        Box::pin(async move {
            let mut guard = state.cron_scheduler.lock().await;
            match guard.as_mut() {
                Some(scheduler) => {
                    if !scheduler.is_active(&job_id) {
                        return (
                            axum::http::StatusCode::NOT_FOUND,
                            axum::Json(serde_json::json!({
                                "status": "not_found",
                                "job_id": job_id,
                            })),
                        );
                    }
                    scheduler.cancel(&job_id);
                    (
                        axum::http::StatusCode::OK,
                        axum::Json(serde_json::json!({
                            "status": "cancelled",
                            "job_id": job_id,
                        })),
                    )
                }
                None => (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    axum::Json(serde_json::json!({
                        "status": "scheduler_not_initialized",
                    })),
                ),
            }
        })
    }
}

// ── RAG API handlers ───────────────────────────────────────────────

fn rag_search_handler(
    state: Arc<GatewayState>,
) -> impl Clone
       + Fn(
    axum::Json<serde_json::Value>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = (axum::http::StatusCode, axum::Json<serde_json::Value>)>
            + Send,
    >,
> {
    move |axum::Json(body): axum::Json<serde_json::Value>| {
        let state = state.clone();
        Box::pin(async move {
            let pipeline = match &state.rag_pipeline {
                Some(p) => p,
                None => {
                    return (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        axum::Json(serde_json::json!({"error": "RAG pipeline not configured"})),
                    )
                }
            };
            let query = body["query"].as_str().unwrap_or_default();
            let top_k = body["top_k"].as_u64().unwrap_or(5).min(100) as usize;
            if query.is_empty() {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error": "query is required"})),
                );
            }
            let results = pipeline.search(query, top_k);
            (
                axum::http::StatusCode::OK,
                axum::Json(serde_json::json!({"results": results})),
            )
        })
    }
}

fn rag_hybrid_handler(
    state: Arc<GatewayState>,
) -> impl Clone
       + Fn(
    axum::Json<serde_json::Value>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = (axum::http::StatusCode, axum::Json<serde_json::Value>)>
            + Send,
    >,
> {
    move |axum::Json(body): axum::Json<serde_json::Value>| {
        let state = state.clone();
        Box::pin(async move {
            let pipeline = match &state.rag_pipeline {
                Some(p) => p,
                None => {
                    return (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        axum::Json(serde_json::json!({"error": "RAG pipeline not configured"})),
                    )
                }
            };
            let query = body["query"].as_str().unwrap_or_default();
            let top_k = body["top_k"].as_u64().unwrap_or(5).min(100) as usize;
            let alpha = body["alpha"].as_f64().unwrap_or(0.65).clamp(0.0, 1.0) as f32;
            let embedding: Vec<f32> = body["embedding"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_f64().map(|f| f as f32))
                        .collect()
                })
                .unwrap_or_default();
            if query.is_empty() {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error": "query is required"})),
                );
            }
            if embedding.is_empty() {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error": "embedding is required"})),
                );
            }
            let results = pipeline.hybrid_search(query, embedding, top_k, alpha);
            (
                axum::http::StatusCode::OK,
                axum::Json(serde_json::json!({"results": results})),
            )
        })
    }
}

fn rag_filtered_handler(
    state: Arc<GatewayState>,
) -> impl Clone
       + Fn(
    axum::Json<serde_json::Value>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = (axum::http::StatusCode, axum::Json<serde_json::Value>)>
            + Send,
    >,
> {
    move |axum::Json(body): axum::Json<serde_json::Value>| {
        let state = state.clone();
        Box::pin(async move {
            let pipeline = match &state.rag_pipeline {
                Some(p) => p,
                None => {
                    return (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        axum::Json(serde_json::json!({"error": "RAG pipeline not configured"})),
                    )
                }
            };
            let query = body["query"].as_str().unwrap_or_default();
            let top_k = body["top_k"].as_u64().unwrap_or(5).min(100) as usize;
            let filters: std::collections::HashMap<String, String> = body["filters"]
                .as_object()
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();
            if query.is_empty() {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error": "query is required"})),
                );
            }
            let results = pipeline.filtered_search(query, top_k, filters);
            (
                axum::http::StatusCode::OK,
                axum::Json(serde_json::json!({"results": results})),
            )
        })
    }
}

fn rag_diagnostics_handler(
    state: Arc<GatewayState>,
) -> impl Clone
       + Fn() -> std::pin::Pin<
    Box<dyn std::future::Future<Output = axum::Json<serde_json::Value>> + Send>,
> {
    move || {
        let state = state.clone();
        Box::pin(async move {
            match &state.rag_pipeline {
                Some(p) => {
                    let diag = p.diagnostics();
                    axum::Json(serde_json::json!({
                        "enabled": true,
                        "document_count": p.document_count(),
                        "chunk_count": p.chunk_count(),
                        "integrity_ok": p.integrity_ok(),
                        "diagnostics": {
                            "document_count": diag.document_count,
                            "chunk_count": diag.chunk_count,
                            "integrity_ok": diag.integrity_ok,
                        },
                    }))
                }
                None => axum::Json(serde_json::json!({"enabled": false})),
            }
        })
    }
}

fn rag_ingest_handler(
    state: Arc<GatewayState>,
) -> impl Clone
       + Fn(
    axum::Json<serde_json::Value>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = (axum::http::StatusCode, axum::Json<serde_json::Value>)>
            + Send,
    >,
> {
    move |axum::Json(body): axum::Json<serde_json::Value>| {
        let state = state.clone();
        Box::pin(async move {
            let pipeline = match &state.rag_pipeline {
                Some(p) => p,
                None => {
                    return (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        axum::Json(serde_json::json!({"error": "RAG pipeline not configured"})),
                    )
                }
            };
            let path = body["path"].as_str().unwrap_or_default();
            if path.is_empty() {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error": "path is required"})),
                );
            }
            // Prevent path traversal attacks — reject paths with ".." components
            let path_obj = std::path::Path::new(path);
            if path_obj
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({"error": "path traversal not allowed"})),
                );
            }
            match pipeline.ingest(path_obj) {
                Ok(count) => (
                    axum::http::StatusCode::OK,
                    axum::Json(serde_json::json!({"chunks_ingested": count})),
                ),
                Err(e) => (
                    axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    axum::Json(serde_json::json!({"error": e.to_string()})),
                ),
            }
        })
    }
}

// ── Pairing API handlers ───────────────────────────────────────────

fn pairing_generate_handler(
    state: Arc<GatewayState>,
) -> impl Clone
       + Fn() -> std::pin::Pin<
    Box<dyn std::future::Future<Output = axum::Json<serde_json::Value>> + Send>,
> {
    move || {
        let state = state.clone();
        Box::pin(async move {
            let code = state.pairing_service.generate_code();
            axum::Json(serde_json::json!({
                "code": code,
                "expires_in": "24 hours",
                "instructions": "Share this code with the user. They send it to the bot to pair.",
            }))
        })
    }
}

fn pairing_status_handler(
    state: Arc<GatewayState>,
) -> impl Clone
       + Fn() -> std::pin::Pin<
    Box<dyn std::future::Future<Output = axum::Json<serde_json::Value>> + Send>,
> {
    move || {
        let state = state.clone();
        Box::pin(async move {
            let paired = state.pairing_service.paired_users();
            let active_codes = state.pairing_service.active_codes();
            axum::Json(serde_json::json!({
                "paired_count": paired.len(),
                "paired_users": paired,
                "active_codes": active_codes.len(),
            }))
        })
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access_control::{AccessControlBridge, ChainedScopeResolver, StaticScopeResolver};
    use crate::action_executor::ActionExecutor;
    use crate::agent_codegen::AgentCodegen;
    use crate::agent_registry::AgentRegistry;
    use crate::audit::NullAuditSink;
    use crate::config::GatewayConfig;
    use crate::control_panel::ControlPanelState;
    use crate::knowledge_graph::KnowledgeGraph;
    use crate::mcp::McpConnectionManager;
    use crate::pairing::DmPairingService;
    use crate::process_manager::ProcessManager;
    use crate::proxy_pool::RemoteAgentProxyPool;
    use crate::rbac_bridge::RbacBridge;
    use crate::router::MessageRouter;
    use crate::shutdown::ShutdownCoordinator;
    use crate::skill_loader::SkillLoader;
    use crate::tool_registry::ToolRegistry;
    use crate::webhook::WebhookHandler;
    use arc_swap::ArcSwap;
    use std::path::PathBuf;
    use tokio::sync::RwLock;
    use tokio_util::sync::CancellationToken;

    /// Build a minimal GatewayState for route testing.
    fn test_state() -> Arc<crate::gateway_state::GatewayState> {
        let config = GatewayConfig::default();
        let session_service: Arc<dyn adk_session::SessionService> =
            Arc::new(adk_session::InMemorySessionService::new());
        let session_bridge = Arc::new(crate::session_bridge::SessionBridge::new(
            config.session.clone(),
            "test".into(),
            session_service.clone(),
        ));
        let router = Arc::new(ArcSwap::from_pointee(MessageRouter::new(
            &config.routing,
            "main".into(),
        )));
        let control_panel = Arc::new(ControlPanelState::new(Arc::new(ArcSwap::from_pointee(
            config.clone(),
        ))));
        let shutdown = CancellationToken::new();
        Arc::new(crate::gateway_state::GatewayState {
            config: Arc::new(ArcSwap::from_pointee(config.clone())),
            session_bridge,
            router,
            session_service,
            channel_map: Arc::new(dashmap::DashMap::new()),
            agents: Arc::new(dashmap::DashMap::new()),
            tool_registry: Arc::new(ToolRegistry::new()),
            plugin_manager: Arc::new(crate::plugin_manager::PluginManager::load_plugins(
                &[],
                |_| None,
            )),
            access_control: Arc::new(RwLock::new(AccessControlBridge::new(&config))),
            pairing_service: Arc::new(DmPairingService::new()),
            shutdown_coordinator: Arc::new(ShutdownCoordinator::new(shutdown.clone())),
            metrics: Arc::new(GatewayMetrics::new()),
            knowledge_graph: Arc::new(KnowledgeGraph::new()),
            rag_pipeline: None,
            control_panel,
            shutdown,
            graph_workflow: None,
            action_executor: Arc::new(ActionExecutor::new()),
            mcp_manager: Arc::new(McpConnectionManager::new()),
            scope_resolver: Arc::new(ChainedScopeResolver::new(vec![Box::new(
                StaticScopeResolver::new(std::collections::HashMap::new()),
            )])),
            kg_toolset: None,
            jwt_validator: None,
            audit_sink: Arc::new(NullAuditSink),
            skill_index: Arc::new(SkillLoader::build_index(vec![])),
            config_path: PathBuf::from("test-config.json"),
            memory_summaries: Arc::new(dashmap::DashMap::new()),
            cron_scheduler: Arc::new(tokio::sync::Mutex::new(None)),
            agent_registry: Arc::new(AgentRegistry::new(PathBuf::from("/tmp/test-registry"))),
            process_manager: Arc::new(ProcessManager::with_defaults()),
            agent_codegen: Arc::new(AgentCodegen::new(PathBuf::from("/tmp/test-gw"), None)),
            rbac: Arc::new(RbacBridge::new()),
            proxy_pool: Arc::new(RemoteAgentProxyPool::new()),
            awp_state: None,
            agent_management_tools: vec![],
            fallback_chain: Arc::new(
                crate::fallback_chain::FallbackModelChain::new_empty_for_test(),
            ),
            agent_instruction: Arc::new(String::new()),
        })
    }

    fn test_webhook_handler() -> Arc<WebhookHandler> {
        let config = crate::config::HooksConfig {
            enabled: true,
            token: None,
            path: None,
        };
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        Arc::new(WebhookHandler::new(
            Arc::new(ArcSwap::new(Arc::new(config))),
            tx,
        ))
    }

    #[tokio::test]
    async fn health_endpoint_returns_json_with_status_field() {
        let state = test_state();
        let wh = test_webhook_handler();
        let app = build_router(&state, wh);

        let req = axum::http::Request::builder()
            .uri("/health")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["status"], "healthy");
        assert!(json.get("active_sessions").is_some());
        assert!(json.get("channels").is_some());
        assert!(json["channels"].is_array());
    }

    #[tokio::test]
    async fn status_endpoint_returns_json_with_expected_fields() {
        let state = test_state();
        let wh = test_webhook_handler();
        let app = build_router(&state, wh);

        let req = axum::http::Request::builder()
            .uri("/status")
            .body(axum::body::Body::empty())
            .unwrap();

        let resp = tower::ServiceExt::oneshot(app, req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["status"], "running");
        assert!(json.get("paired_count").is_some());
        assert!(json.get("plugin_count").is_some());
        assert!(json.get("plugin_names").is_some());
        assert!(
            json.get("rag").is_some(),
            "/status must include 'rag' field"
        );
        assert!(json.get("skills").is_some());
        assert!(json.get("session_service").is_some());
        assert!(json.get("in_flight_count").is_some());
        assert!(json.get("known_tool_names").is_some());
        assert!(json.get("cron").is_some());
        assert!(json.get("webhook_path").is_some());
        assert!(json.get("config_path").is_some());
        assert!(json.get("channels").is_some());
    }
}

fn webhook_route(
    wh: Arc<WebhookHandler>,
) -> impl Clone
       + Fn(
    axum::http::HeaderMap,
    axum::Json<WebhookRequest>,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<Output = (axum::http::StatusCode, axum::Json<WebhookResponse>)>
            + Send,
    >,
> {
    // Simple sliding-window rate limiter: max 60 requests per 60 seconds
    let rate_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let rate_window_start = Arc::new(std::sync::Mutex::new(std::time::Instant::now()));
    let max_requests_per_minute: u64 = 60;

    move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<WebhookRequest>| {
        let wh = wh.clone();
        let rate_counter = rate_counter.clone();
        let rate_window_start = rate_window_start.clone();
        Box::pin(async move {
            // Rate limiting check
            {
                let mut start = rate_window_start.lock().unwrap_or_else(|e| e.into_inner());
                if start.elapsed() > std::time::Duration::from_secs(60) {
                    *start = std::time::Instant::now();
                    rate_counter.store(0, std::sync::atomic::Ordering::Relaxed);
                }
                let count = rate_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if count >= max_requests_per_minute {
                    return (
                        axum::http::StatusCode::TOO_MANY_REQUESTS,
                        axum::Json(WebhookResponse {
                            status: "rate limited".into(),
                            response: None,
                            request_id: None,
                        }),
                    );
                }
            }

            if !wh.is_enabled() {
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    axum::Json(WebhookResponse {
                        status: "disabled".into(),
                        response: None,
                        request_id: None,
                    }),
                );
            }

            let auth = headers
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok());
            if wh.validate_token(auth).is_err() {
                return (
                    axum::http::StatusCode::UNAUTHORIZED,
                    axum::Json(WebhookResponse {
                        status: "unauthorized".into(),
                        response: None,
                        request_id: None,
                    }),
                );
            }

            match wh.process_request(body).await {
                Ok(rid) => (
                    axum::http::StatusCode::OK,
                    axum::Json(WebhookResponse {
                        status: "ok".into(),
                        response: None,
                        request_id: Some(rid),
                    }),
                ),
                Err(e) => {
                    let status_code = match &e {
                        crate::webhook::WebhookError::PayloadTooLarge { .. } => {
                            axum::http::StatusCode::PAYLOAD_TOO_LARGE
                        }
                        _ => axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                    };
                    (
                        status_code,
                        axum::Json(WebhookResponse {
                            status: format!("error: {e}"),
                            response: None,
                            request_id: None,
                        }),
                    )
                }
            }
        })
    }
}
