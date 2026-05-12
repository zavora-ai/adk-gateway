//! Web-based control panel served via axum.
//!
//! Serves a React SPA at `/ui/*` and JSON API endpoints at `/ui/api/*`
//! for programmatic access. WebSocket at `/ws/events` for live updates.
//!
//! Design: ControlPanel [R5.1–R5.7, R12.4, R24.20, R25.10]

use axum::response::IntoResponse;

pub mod agent_setup;
pub mod agents;
pub mod api;
pub mod auth;
pub mod channels;
pub mod config_page;
pub mod dashboard;
pub mod delegation;
pub mod logs;
pub mod memory;
pub mod sessions;
pub mod settings;
pub mod ws;

use crate::config::GatewayConfig;
use crate::telemetry::redact_config;
use serde::Serialize;
use std::sync::Arc;
use std::time::Instant;

// ── Data types ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct DashboardData {
    pub uptime_secs: u64,
    pub connected_channels: Vec<ChannelInfo>,
    pub active_session_count: u64,
    pub memory_status: Option<SubsystemStatus>,
    pub rag_status: Option<SubsystemStatus>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChannelInfo {
    pub channel_type: String,
    pub account_id: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubsystemStatus {
    pub backend_type: String,
    pub healthy: bool,
    pub details: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub session_id: String,
    pub user_id: String,
    pub channel_type: String,
    pub last_activity: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub target: Option<String>,
}

// ── API Response envelope ──────────────────────────────────────────

/// Generic API response envelope for control panel endpoints.
/// Used by API handlers to return consistent JSON responses.
#[allow(dead_code)]
#[derive(Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[allow(dead_code)]
impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            ok: true,
            data: Some(data),
            message: None,
        }
    }
}

#[allow(dead_code)]
impl ApiResponse<()> {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            message: Some(message.into()),
        }
    }

    pub fn ok_msg(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            data: None,
            message: Some(message.into()),
        }
    }
}

// ── State ──────────────────────────────────────────────────────────

pub struct ControlPanelState {
    pub start_time: Instant,
    pub config: Arc<arc_swap::ArcSwap<GatewayConfig>>,
    pub config_path: Option<std::path::PathBuf>,
    pub sessions: Arc<std::sync::RwLock<Vec<SessionInfo>>>,
    pub log_buffer: Arc<std::sync::RwLock<Vec<LogEntry>>>,
    pub channels: Arc<std::sync::RwLock<Vec<ChannelInfo>>>,
    /// Optional reference to the RAG pipeline for live health checks.
    pub rag_pipeline: Option<Arc<crate::rag::RagPipeline>>,
    /// Optional reference to the knowledge graph for entity stats.
    pub knowledge_graph: Option<Arc<crate::knowledge_graph::KnowledgeGraph>>,
    /// Optional reference to the agent registry for the agents management page.
    pub agent_registry: Option<Arc<crate::agent_registry::AgentRegistry>>,
    /// Audit sink for logging agent management operations.
    pub audit_sink: Option<Arc<dyn crate::audit::AuditSink + Send + Sync>>,
    /// Per-agent log buffers keyed by agent ID.
    pub agent_logs: Arc<dashmap::DashMap<String, Vec<LogEntry>>>,
    /// AWP protocol state (None when AWP is disabled).
    pub awp_state: Option<adk_awp::AwpState>,
    /// MCP connection manager for server status and tool discovery.
    pub mcp_manager: Option<Arc<crate::mcp::McpConnectionManager>>,
    /// Cron scheduler for job listing and cancellation.
    pub cron_scheduler: Option<Arc<tokio::sync::Mutex<Option<crate::cron::CronScheduler>>>>,
    /// Tool registry for listing all registered tools by source.
    pub tool_registry: Option<Arc<crate::tool_registry::ToolRegistry>>,
    /// Plugin manager for listing loaded plugins.
    pub plugin_manager: Option<Arc<crate::plugin_manager::PluginManager>>,
    /// Session bridge for session termination.
    pub session_bridge: Option<Arc<crate::session_bridge::SessionBridge>>,
    /// Broadcast channel sender for WebSocket live updates.
    pub ws_broadcast: tokio::sync::broadcast::Sender<ws::WsEvent>,
    /// Gateway bind address for display in the sidebar footer.
    pub bind_address: Option<String>,
    /// Active UI sessions keyed by session token.
    pub ui_sessions: Arc<dashmap::DashMap<String, auth::UiSession>>,
    /// Delegation permission store.
    pub delegation_store: delegation::DelegationStore,
}

impl std::fmt::Debug for ControlPanelState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlPanelState")
            .field("start_time", &self.start_time)
            .field("has_audit_sink", &self.audit_sink.is_some())
            .finish()
    }
}

impl ControlPanelState {
    pub fn new(config: Arc<arc_swap::ArcSwap<GatewayConfig>>) -> Self {
        let (ws_broadcast, _) = tokio::sync::broadcast::channel(256);
        Self {
            start_time: Instant::now(),
            config,
            config_path: None,
            sessions: Arc::new(std::sync::RwLock::new(Vec::new())),
            log_buffer: Arc::new(std::sync::RwLock::new(Vec::new())),
            channels: Arc::new(std::sync::RwLock::new(Vec::new())),
            rag_pipeline: None,
            knowledge_graph: None,
            agent_registry: None,
            audit_sink: None,
            agent_logs: Arc::new(dashmap::DashMap::new()),
            awp_state: None,
            mcp_manager: None,
            cron_scheduler: None,
            tool_registry: None,
            plugin_manager: None,
            session_bridge: None,
            ws_broadcast,
            bind_address: None,
            ui_sessions: Arc::new(dashmap::DashMap::new()),
            delegation_store: delegation::new_delegation_store(),
        }
    }

    pub fn with_config_path(mut self, path: std::path::PathBuf) -> Self {
        self.config_path = Some(path);
        self
    }

    pub fn with_rag_pipeline(mut self, pipeline: Arc<crate::rag::RagPipeline>) -> Self {
        self.rag_pipeline = Some(pipeline);
        self
    }

    pub fn with_knowledge_graph(mut self, kg: Arc<crate::knowledge_graph::KnowledgeGraph>) -> Self {
        self.knowledge_graph = Some(kg);
        self
    }

    pub fn with_agent_registry(
        mut self,
        registry: Arc<crate::agent_registry::AgentRegistry>,
    ) -> Self {
        self.agent_registry = Some(registry);
        self
    }

    pub fn with_audit_sink(mut self, sink: Arc<dyn crate::audit::AuditSink + Send + Sync>) -> Self {
        self.audit_sink = Some(sink);
        self
    }

    pub fn with_awp_state(mut self, awp_state: adk_awp::AwpState) -> Self {
        self.awp_state = Some(awp_state);
        self
    }

    pub fn with_mcp_manager(mut self, mcp_manager: Arc<crate::mcp::McpConnectionManager>) -> Self {
        self.mcp_manager = Some(mcp_manager);
        self
    }

    pub fn with_cron_scheduler(
        mut self,
        cron_scheduler: Arc<tokio::sync::Mutex<Option<crate::cron::CronScheduler>>>,
    ) -> Self {
        self.cron_scheduler = Some(cron_scheduler);
        self
    }

    pub fn with_tool_registry(
        mut self,
        tool_registry: Arc<crate::tool_registry::ToolRegistry>,
    ) -> Self {
        self.tool_registry = Some(tool_registry);
        self
    }

    pub fn with_plugin_manager(
        mut self,
        plugin_manager: Arc<crate::plugin_manager::PluginManager>,
    ) -> Self {
        self.plugin_manager = Some(plugin_manager);
        self
    }

    pub fn with_session_bridge(
        mut self,
        session_bridge: Arc<crate::session_bridge::SessionBridge>,
    ) -> Self {
        self.session_bridge = Some(session_bridge);
        self
    }

    pub fn with_bind_address(mut self, bind_address: String) -> Self {
        self.bind_address = Some(bind_address);
        self
    }

    /// Push a log entry to an agent's log buffer (capped at 200 entries).
    pub fn push_agent_log(&self, agent_id: &str, entry: LogEntry) {
        let mut logs = self.agent_logs.entry(agent_id.to_string()).or_default();
        logs.push(entry);
        if logs.len() > 200 {
            let drain = logs.len() - 200;
            logs.drain(..drain);
        }
    }

    /// Get recent logs for a specific agent.
    pub fn agent_logs(&self, agent_id: &str, limit: usize) -> Vec<LogEntry> {
        self.agent_logs
            .get(agent_id)
            .map(|logs| {
                let start = logs.len().saturating_sub(limit);
                logs[start..].to_vec()
            })
            .unwrap_or_default()
    }

    pub fn dashboard(&self) -> DashboardData {
        let channels = self.channels.read().map(|c| c.clone()).unwrap_or_default();
        let session_count = self.sessions.read().map(|s| s.len() as u64).unwrap_or(0);
        let config = self.config.load();

        let memory_status = config.memory.as_ref().map(|m| SubsystemStatus {
            backend_type: format!("{:?}", m.backend),
            healthy: true,
            details: format!(
                "embedding: {}{}",
                m.embedding.provider,
                m.embedding
                    .model
                    .as_deref()
                    .map(|model| format!(" ({})", model))
                    .unwrap_or_default()
            ),
        });

        let rag_status = config.rag.as_ref().map(|r| {
            let healthy = self
                .rag_pipeline
                .as_ref()
                .map(|rp| rp.diagnostics().integrity_ok)
                .unwrap_or(true);
            SubsystemStatus {
                backend_type: format!("{:?}", r.vector_store),
                healthy,
                details: format!(
                    "embedding: {}{}, chunk_size: {}",
                    r.embedding.provider,
                    r.embedding
                        .model
                        .as_deref()
                        .map(|model| format!(" ({})", model))
                        .unwrap_or_default(),
                    r.chunk_size.unwrap_or(512)
                ),
            }
        });

        DashboardData {
            uptime_secs: self.start_time.elapsed().as_secs(),
            connected_channels: channels,
            active_session_count: session_count,
            memory_status,
            rag_status,
        }
    }

    pub fn sessions_list(&self) -> Vec<SessionInfo> {
        self.sessions.read().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn redacted_config(&self) -> serde_json::Value {
        let config = self.config.load();
        let json = serde_json::to_value(config.as_ref()).unwrap_or(serde_json::Value::Null);
        redact_config(&json)
    }

    pub fn recent_logs(&self, limit: usize) -> Vec<LogEntry> {
        self.log_buffer
            .read()
            .map(|logs| {
                let start = logs.len().saturating_sub(limit);
                logs[start..].to_vec()
            })
            .unwrap_or_default()
    }

    pub fn push_log(&self, entry: LogEntry) {
        // Broadcast the log event to WebSocket clients
        let _ = self.ws_broadcast.send(ws::WsEvent::Log {
            timestamp: entry.timestamp.clone(),
            level: entry.level.clone(),
            message: entry.message.clone(),
            target: entry.target.clone(),
        });

        if let Ok(mut logs) = self.log_buffer.write() {
            logs.push(entry);
            if logs.len() > 1000 {
                let drain = logs.len() - 1000;
                logs.drain(..drain);
            }
        }
    }

    pub fn update_channels(&self, channels: Vec<ChannelInfo>) {
        if let Ok(mut ch) = self.channels.write() {
            *ch = channels;
        }
    }

    pub fn update_sessions(&self, sessions: Vec<SessionInfo>) {
        if let Ok(mut s) = self.sessions.write() {
            *s = sessions;
        }
    }
}

// ── Routes ─────────────────────────────────────────────────────────

pub fn build_routes(state: Arc<ControlPanelState>) -> axum::Router<Arc<ControlPanelState>> {
    // Public routes — accessible without authentication
    let public_routes = axum::Router::new()
        .route("/ui/api/auth/check", axum::routing::get(api::auth_check))
        .route("/ui/api/login", axum::routing::post(api::api_login))
        .route("/ui/api/logout", axum::routing::post(api::api_logout));

    // Protected JSON API routes — auth guard middleware applied.
    // NOTE: The auth guard uses the router's own state (Arc<ControlPanelState>)
    // which is the same state that api_login writes sessions to. This ensures
    // the guard checks against the real ui_sessions map.
    let protected_routes = axum::Router::new()
        // Dashboard
        .route("/ui/api/dashboard", axum::routing::get(api::dashboard_json))
        // Sessions
        .route("/ui/api/sessions", axum::routing::get(api::sessions_json))
        .route(
            "/ui/api/sessions/{id}/terminate",
            axum::routing::post(api::session_terminate),
        )
        // Config
        .route(
            "/ui/api/config",
            axum::routing::get(api::config_json).post(api::config_save),
        )
        // Logs
        .route("/ui/api/logs", axum::routing::get(api::logs_json))
        // Settings
        .route("/ui/api/settings", axum::routing::post(api::settings_save))
        .route(
            "/ui/api/settings/session-status",
            axum::routing::get(api::session_status),
        )
        // Channels
        .route("/ui/api/channels", axum::routing::get(api::channels_get).post(api::channels_save))
        .route(
            "/ui/api/channels/telegram/probe",
            axum::routing::post(api::telegram_probe),
        )
        // Agent setup
        .route(
            "/ui/api/agent",
            axum::routing::get(api::agent_get).post(api::agent_save),
        )
        // Memory
        .route(
            "/ui/api/memory",
            axum::routing::get(api::memory_load).post(api::memory_save),
        )
        // Agents CRUD
        .route(
            "/ui/api/agents",
            axum::routing::get(api::api_agents_list).post(api::api_agents_create),
        )
        .route(
            "/ui/api/agents/{id}/start",
            axum::routing::post(api::api_agents_start),
        )
        .route(
            "/ui/api/agents/{id}/stop",
            axum::routing::post(api::api_agents_stop),
        )
        .route(
            "/ui/api/agents/{id}/delete",
            axum::routing::post(api::api_agents_delete),
        )
        .route(
            "/ui/api/agents/{id}/logs",
            axum::routing::get(api::api_agents_logs),
        )
        .route(
            "/ui/api/agents/{id}/configure",
            axum::routing::post(api::api_agents_configure),
        )
        // AWP endpoints
        .route("/ui/api/awp", axum::routing::get(api::awp_summary))
        .route("/ui/api/awp/health", axum::routing::get(api::awp_health))
        .route(
            "/ui/api/awp/capabilities",
            axum::routing::get(api::awp_capabilities),
        )
        .route(
            "/ui/api/awp/subscriptions",
            axum::routing::get(api::awp_subscriptions),
        )
        .route(
            "/ui/api/awp/subscriptions/{id}",
            axum::routing::delete(api::awp_subscription_delete),
        )
        .route("/ui/api/awp/consent", axum::routing::get(api::awp_consent))
        // Integrations endpoints
        .route(
            "/ui/api/integrations/mcp",
            axum::routing::get(api::integrations_mcp),
        )
        .route(
            "/ui/api/integrations/cron",
            axum::routing::get(api::integrations_cron),
        )
        .route(
            "/ui/api/integrations/tools",
            axum::routing::get(api::integrations_tools),
        )
        // WebSocket
        .route("/ws/events", axum::routing::get(ws::ws_events_handler))
        // Delegation permissions
        .route(
            "/ui/api/delegation",
            axum::routing::get(delegation::api_delegation_list)
                .post(delegation::api_delegation_add)
                .delete(delegation::api_delegation_remove),
        )
        // Auth guard middleware — uses the router's own Arc<ControlPanelState>
        .route_layer(axum::middleware::from_fn_with_state(
            state,
            auth::auth_guard,
        ));

    // SPA fallback: serve embedded UI assets for all /ui/* routes.
    // When built with `ui/dist/` present, assets are compiled into the binary.
    // Falls back to index.html for client-side routing (React SPA).
    let spa = axum::Router::new()
        .fallback(embedded_ui_handler);

    public_routes
        .merge(protected_routes)
        .nest_service("/ui", spa)
}

// ── Embedded UI Assets ─────────────────────────────────────────────

#[derive(rust_embed::Embed)]
#[folder = "ui/dist/"]
#[prefix = ""]
struct UiAssets;

/// Serve embedded UI assets, falling back to index.html for SPA routing.
async fn embedded_ui_handler(
    uri: axum::http::Uri,
) -> impl axum::response::IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // Try to serve the exact file
    if let Some(file) = UiAssets::get(path) {
        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();
        return (
            [(axum::http::header::CONTENT_TYPE, mime)],
            file.data.to_vec(),
        )
            .into_response();
    }

    // SPA fallback: serve index.html for all unmatched routes
    match UiAssets::get("index.html") {
        Some(index) => (
            [(axum::http::header::CONTENT_TYPE, "text/html".to_string())],
            index.data.to_vec(),
        )
            .into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            "UI not found. Build with: cd ui && npm run build",
        )
            .into_response(),
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GatewayConfig;

    fn make_state() -> Arc<ControlPanelState> {
        let config = Arc::new(arc_swap::ArcSwap::from_pointee(GatewayConfig::default()));
        Arc::new(ControlPanelState::new(config))
    }

    #[test]
    fn test_dashboard_data() {
        let state = make_state();
        let data = state.dashboard();
        assert_eq!(data.active_session_count, 0);
        assert!(data.connected_channels.is_empty());
    }

    #[test]
    fn test_sessions_list() {
        let state = make_state();
        state.update_sessions(vec![SessionInfo {
            session_id: "s1".into(),
            user_id: "u1".into(),
            channel_type: "telegram".into(),
            last_activity: "2024-01-01T00:00:00Z".into(),
        }]);
        let sessions = state.sessions_list();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].user_id, "u1");
    }

    #[test]
    fn test_redacted_config() {
        let state = make_state();
        let redacted = state.redacted_config();
        assert!(redacted.is_object());
    }

    #[test]
    fn test_log_buffer() {
        let state = make_state();
        for i in 0..5 {
            state.push_log(LogEntry {
                timestamp: format!("2024-01-01T00:00:0{i}Z"),
                level: "INFO".into(),
                message: format!("log {i}"),
                target: None,
            });
        }
        let logs = state.recent_logs(3);
        assert_eq!(logs.len(), 3);
        assert_eq!(logs[0].message, "log 2");
    }

    #[test]
    fn test_log_buffer_cap() {
        let state = make_state();
        for i in 0..1050 {
            state.push_log(LogEntry {
                timestamp: String::new(),
                level: "INFO".into(),
                message: format!("msg {i}"),
                target: None,
            });
        }
        let logs = state.recent_logs(2000);
        assert!(logs.len() <= 1000);
    }

    #[test]
    fn test_update_channels() {
        let state = make_state();
        state.update_channels(vec![
            ChannelInfo {
                channel_type: "telegram".into(),
                account_id: "default".into(),
                status: "connected".into(),
            },
            ChannelInfo {
                channel_type: "slack".into(),
                account_id: "team1".into(),
                status: "reconnecting".into(),
            },
        ]);
        let data = state.dashboard();
        assert_eq!(data.connected_channels.len(), 2);
    }

    #[test]
    fn test_build_routes() {
        let state = make_state();
        let _router = build_routes(state);
    }
}
