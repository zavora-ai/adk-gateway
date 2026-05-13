//! MCP (Model Context Protocol) server connection management.
//!
//! Provides [`McpConnectionManager`] for managing connections to external MCP
//! servers. Each server is tracked as an [`McpConnection`] with lifecycle
//! management: connect, disconnect, reconnect with exponential backoff, and
//! hot-reload reconciliation.
//!
//! The actual MCP protocol implementation lives in adk-tool's `McpToolset`.
//! This module manages the connection lifecycle, status tracking, and
//! reconciliation logic that the gateway needs on top of that.

use std::time::Duration;

use dashmap::DashMap;
use serde::{Deserialize, Serialize};

// ─── McpServerConfig ───────────────────────────────────────────────

/// Configuration for a single MCP server connection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Unique identifier for this MCP server.
    pub server_id: String,
    /// How to launch or reach the server.
    pub transport: McpTransport,
    /// Optional authentication credentials.
    pub auth: Option<McpAuth>,
    /// Whether this server is enabled (default: true).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Transport mechanism for reaching an MCP server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpTransport {
    /// Stdio-based transport: launch a subprocess.
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
        env: std::collections::HashMap<String, String>,
    },
    /// SSE/HTTP-based transport: connect to a URL.
    #[serde(alias = "http")]
    Sse { url: String },
}

/// Authentication credentials for an MCP server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum McpAuth {
    /// Bearer token authentication.
    Token { token: String },
    /// OAuth2 client credentials.
    OAuth {
        client_id: String,
        client_secret: String,
        token_url: String,
    },
}

// ─── ConnectionStatus ──────────────────────────────────────────────

/// Current status of an MCP server connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Successfully connected and tools are available.
    Connected,
    /// Connection lost, background reconnection in progress.
    #[allow(dead_code)] // Set during reconnection flow
    Reconnecting,
    /// Connection failed and no reconnection is active.
    #[allow(dead_code)] // Set when reconnection exhausts max attempts
    Failed,
    /// Intentionally disconnected.
    Disconnected,
}

// ─── McpConnection ─────────────────────────────────────────────────

/// Tracks the state of a single MCP server connection.
#[derive(Debug, Clone)]
pub struct McpConnection {
    /// Server identifier (matches [`McpServerConfig::server_id`]).
    #[allow(dead_code)] // Stored for identification; used in debug output and tests
    pub server_id: String,
    /// The configuration used to establish this connection.
    pub config: McpServerConfig,
    /// Current connection status.
    pub status: ConnectionStatus,
    /// Tool names discovered from this server.
    pub discovered_tools: Vec<String>,
    /// Number of reconnection attempts since last successful connect.
    pub reconnect_attempts: u32,
}

impl McpConnection {
    /// Create a new connection entry in [`ConnectionStatus::Disconnected`] state.
    pub fn new(config: McpServerConfig) -> Self {
        let server_id = config.server_id.clone();
        Self {
            server_id,
            config,
            status: ConnectionStatus::Disconnected,
            discovered_tools: Vec::new(),
            reconnect_attempts: 0,
        }
    }
}

// ─── Backoff ───────────────────────────────────────────────────────

/// Initial delay for exponential backoff reconnection.
#[allow(dead_code)] // Used by backoff_duration(); available for reconnection logic
const BACKOFF_INITIAL: Duration = Duration::from_secs(1);
/// Maximum delay cap for exponential backoff reconnection.
#[allow(dead_code)] // Used by backoff_duration(); available for reconnection logic
const BACKOFF_CAP: Duration = Duration::from_secs(60);

/// Calculate the backoff duration for a given attempt number.
///
/// Uses exponential backoff: `min(initial * 2^attempt, cap)`.
#[allow(dead_code)] // Used in tests; available for MCP reconnection scheduling
pub fn backoff_duration(attempt: u32) -> Duration {
    let shift = attempt.min(63); // prevent overflow on large attempt values
    let secs = BACKOFF_INITIAL
        .as_secs()
        .saturating_mul(1u64.checked_shl(shift).unwrap_or(u64::MAX));
    Duration::from_secs(secs.min(BACKOFF_CAP.as_secs()))
}

// ─── McpConnectionManager ──────────────────────────────────────────

/// Manages MCP server connections with lifecycle support.
///
/// Stores connections in a concurrent [`DashMap`] keyed by server ID.
/// Provides connect, disconnect, reconnect (with exponential backoff),
/// and hot-reload reconciliation.
pub struct McpConnectionManager {
    connections: DashMap<String, McpConnection>,
}

impl McpConnectionManager {
    /// Create a new empty connection manager.
    pub fn new() -> Self {
        Self {
            connections: DashMap::new(),
        }
    }

    /// Connect to an MCP server described by `config`.
    ///
    /// For Stdio transport, uses `adk_tool::mcp::McpServerManager` to spawn
    /// the child process and discover tools. For SSE transport, records the
    /// connection as available (actual HTTP-based MCP requires the SSE client
    /// which is handled at request time).
    pub async fn connect(&self, config: &McpServerConfig) -> anyhow::Result<()> {
        if !config.enabled {
            tracing::info!(server_id = %config.server_id, "MCP server disabled, skipping");
            return Ok(());
        }

        tracing::info!(server_id = %config.server_id, "connecting to MCP server");

        let mut conn = McpConnection::new(config.clone());

        match &config.transport {
            McpTransport::Stdio { command, args, env } => {
                // Convert our config to adk-tool's McpServerConfig format
                let adk_config = adk_tool::mcp::McpServerConfig {
                    command: command.clone(),
                    args: args.clone(),
                    env: env.clone(),
                    disabled: false,
                    auto_approve: vec![],
                    restart_policy: None,
                };

                let mut configs = std::collections::HashMap::new();
                configs.insert(config.server_id.clone(), adk_config);

                let manager = adk_tool::mcp::McpServerManager::new(configs)
                    .with_name(format!("gateway-mcp-{}", config.server_id));

                // Try to start the server and discover tools
                match manager.start_server(&config.server_id).await {
                    Ok(()) => {
                        // Check server status to confirm it's running
                        let status = manager.server_status(&config.server_id).await;
                        let is_running = matches!(status, Ok(adk_tool::mcp::ServerStatus::Running));

                        conn.status = if is_running {
                            ConnectionStatus::Connected
                        } else {
                            ConnectionStatus::Failed
                        };
                        conn.reconnect_attempts = 0;

                        tracing::info!(
                            server_id = %config.server_id,
                            running = is_running,
                            "MCP server started via stdio"
                        );
                    }
                    Err(e) => {
                        conn.status = ConnectionStatus::Failed;
                        tracing::warn!(
                            server_id = %config.server_id,
                            error = %e,
                            "MCP server failed to start — tools will not be available"
                        );
                    }
                }
            }
            McpTransport::Sse { url } => {
                // SSE transport: mark as connected, tools discovered at request time
                conn.status = ConnectionStatus::Connected;
                conn.reconnect_attempts = 0;
                tracing::info!(
                    server_id = %config.server_id,
                    url = %url,
                    "MCP server registered (SSE transport)"
                );
            }
        }

        self.connections.insert(config.server_id.clone(), conn);
        Ok(())
    }

    /// Disconnect from an MCP server, removing it from the manager.
    pub fn disconnect(&self, server_id: &str) {
        if let Some((_, mut conn)) = self.connections.remove(server_id) {
            conn.status = ConnectionStatus::Disconnected;
            tracing::info!(server_id = %server_id, "MCP server disconnected");
        } else {
            tracing::warn!(server_id = %server_id, "disconnect called for unknown MCP server");
        }
    }

    /// Attempt reconnection with exponential backoff.
    ///
    /// Disconnects the old connection and re-connects with the stored config.
    #[allow(dead_code)]
    pub async fn reconnect(&self, server_id: &str) -> anyhow::Result<()> {
        let config = {
            let mut entry = self
                .connections
                .get_mut(server_id)
                .ok_or_else(|| anyhow::anyhow!("unknown MCP server: {server_id}"))?;
            entry.status = ConnectionStatus::Reconnecting;
            entry.reconnect_attempts += 1;
            let attempt = entry.reconnect_attempts;
            let delay = backoff_duration(attempt.saturating_sub(1));
            tracing::info!(
                server_id = %server_id,
                attempt = attempt,
                delay_secs = delay.as_secs(),
                "scheduling MCP reconnection"
            );
            entry.config.clone()
        };

        // Wait for backoff delay
        let attempt = self
            .connections
            .get(server_id)
            .map(|c| c.reconnect_attempts)
            .unwrap_or(1);
        let delay = backoff_duration(attempt.saturating_sub(1));
        tokio::time::sleep(delay).await;

        // Remove old connection and re-connect
        self.connections.remove(server_id);
        self.connect(&config).await
    }

    /// Get the current connection status for a server.
    #[allow(dead_code)] // Used in tests; available for status reporting
    pub fn get_status(&self, server_id: &str) -> Option<ConnectionStatus> {
        self.connections.get(server_id).map(|c| c.status)
    }

    /// Check whether tools from a given server are currently available.
    ///
    /// Returns `true` only when the server is in [`ConnectionStatus::Connected`]
    /// state. During reconnection, failed, or disconnected states, tools are
    /// not available and should return "temporarily unavailable".
    #[allow(dead_code)] // Used in tests; available for tool availability checks
    pub fn is_tool_available(&self, server_id: &str) -> bool {
        self.connections
            .get(server_id)
            .map(|c| c.status == ConnectionStatus::Connected)
            .unwrap_or(false)
    }

    /// Get the list of discovered tool names for a server.
    pub fn discovered_tools(&self, server_id: &str) -> Vec<String> {
        self.connections
            .get(server_id)
            .map(|c| c.discovered_tools.clone())
            .unwrap_or_default()
    }

    /// Return all currently tracked server IDs.
    pub fn server_ids(&self) -> Vec<String> {
        self.connections.iter().map(|e| e.key().clone()).collect()
    }

    /// Return the number of tracked connections.
    #[allow(dead_code)] // Used in tests; available for diagnostic reporting
    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    /// Reconcile MCP connections against a new set of configs (R19.7).
    ///
    /// - Servers present in `new_configs` but not currently connected → connect.
    /// - Servers currently connected but absent from `new_configs` → disconnect.
    /// - Servers present in both with unchanged config → leave active.
    /// - Servers present in both with changed config → disconnect old, connect new.
    pub async fn reconcile(&self, new_configs: &[McpServerConfig]) {
        let new_ids: std::collections::HashSet<&str> =
            new_configs.iter().map(|c| c.server_id.as_str()).collect();

        // Disconnect removed servers
        let current_ids: Vec<String> = self.server_ids();
        for id in &current_ids {
            if !new_ids.contains(id.as_str()) {
                tracing::info!(server_id = %id, "removing MCP server (no longer in config)");
                self.disconnect(id);
            }
        }

        // Connect new or changed servers
        for config in new_configs {
            if !config.enabled {
                // If it exists but is now disabled, disconnect it
                if self.connections.contains_key(&config.server_id) {
                    tracing::info!(
                        server_id = %config.server_id,
                        "disabling MCP server"
                    );
                    self.disconnect(&config.server_id);
                }
                continue;
            }

            let needs_connect = match self.connections.get(&config.server_id) {
                Some(existing) => existing.config != *config,
                None => true,
            };

            if needs_connect {
                // Disconnect old if it exists (config changed)
                if self.connections.contains_key(&config.server_id) {
                    tracing::info!(
                        server_id = %config.server_id,
                        "reconnecting MCP server (config changed)"
                    );
                    self.disconnect(&config.server_id);
                }

                if let Err(e) = self.connect(config).await {
                    tracing::warn!(
                        server_id = %config.server_id,
                        error = %e,
                        "failed to connect MCP server during reconciliation"
                    );
                }
            }
        }
    }
}

impl Default for McpConnectionManager {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(id: &str) -> McpServerConfig {
        McpServerConfig {
            server_id: id.to_string(),
            transport: McpTransport::Sse {
                url: format!("http://localhost:808{}", id.len()),
            },
            auth: None,
            enabled: true,
        }
    }

    fn test_stdio_config(id: &str) -> McpServerConfig {
        McpServerConfig {
            server_id: id.to_string(),
            transport: McpTransport::Stdio {
                command: "mcp-server".to_string(),
                args: vec!["--port".to_string(), "8080".to_string()],
                env: std::collections::HashMap::new(),
            },
            auth: None,
            enabled: true,
        }
    }

    // ── Backoff ────────────────────────────────────────────────────

    #[test]
    fn backoff_starts_at_one_second() {
        assert_eq!(backoff_duration(0), Duration::from_secs(1));
    }

    #[test]
    fn backoff_doubles_each_attempt() {
        assert_eq!(backoff_duration(0), Duration::from_secs(1));
        assert_eq!(backoff_duration(1), Duration::from_secs(2));
        assert_eq!(backoff_duration(2), Duration::from_secs(4));
        assert_eq!(backoff_duration(3), Duration::from_secs(8));
        assert_eq!(backoff_duration(4), Duration::from_secs(16));
        assert_eq!(backoff_duration(5), Duration::from_secs(32));
    }

    #[test]
    fn backoff_caps_at_sixty_seconds() {
        assert_eq!(backoff_duration(6), Duration::from_secs(60));
        assert_eq!(backoff_duration(7), Duration::from_secs(60));
        assert_eq!(backoff_duration(100), Duration::from_secs(60));
    }

    // ── McpConnection ──────────────────────────────────────────────

    #[test]
    fn new_connection_starts_disconnected() {
        let conn = McpConnection::new(test_config("test-server"));
        assert_eq!(conn.status, ConnectionStatus::Disconnected);
        assert_eq!(conn.reconnect_attempts, 0);
        assert!(conn.discovered_tools.is_empty());
        assert_eq!(conn.server_id, "test-server");
    }

    // ── McpConnectionManager: connect/disconnect ───────────────────

    #[tokio::test]
    async fn connect_sets_status_to_connected() {
        let mgr = McpConnectionManager::new();
        let config = test_config("srv-1");
        mgr.connect(&config).await.unwrap();

        assert_eq!(mgr.get_status("srv-1"), Some(ConnectionStatus::Connected));
        assert!(mgr.is_tool_available("srv-1"));
    }

    #[tokio::test]
    async fn connect_disabled_server_is_noop() {
        let mgr = McpConnectionManager::new();
        let mut config = test_config("srv-disabled");
        config.enabled = false;
        mgr.connect(&config).await.unwrap();

        assert_eq!(mgr.get_status("srv-disabled"), None);
        assert!(!mgr.is_tool_available("srv-disabled"));
        assert_eq!(mgr.connection_count(), 0);
    }

    #[tokio::test]
    async fn disconnect_removes_connection() {
        let mgr = McpConnectionManager::new();
        mgr.connect(&test_config("srv-1")).await.unwrap();
        assert_eq!(mgr.connection_count(), 1);

        mgr.disconnect("srv-1");
        assert_eq!(mgr.connection_count(), 0);
        assert_eq!(mgr.get_status("srv-1"), None);
        assert!(!mgr.is_tool_available("srv-1"));
    }

    #[test]
    fn disconnect_unknown_server_is_safe() {
        let mgr = McpConnectionManager::new();
        mgr.disconnect("nonexistent"); // should not panic
        assert_eq!(mgr.connection_count(), 0);
    }

    #[tokio::test]
    async fn multiple_servers_tracked_independently() {
        let mgr = McpConnectionManager::new();
        mgr.connect(&test_config("srv-a")).await.unwrap();
        mgr.connect(&test_config("srv-b")).await.unwrap();
        // Use SSE config for srv-c since stdio requires a real binary
        mgr.connect(&test_config("srv-c")).await.unwrap();

        assert_eq!(mgr.connection_count(), 3);
        assert!(mgr.is_tool_available("srv-a"));
        assert!(mgr.is_tool_available("srv-b"));
        assert!(mgr.is_tool_available("srv-c"));

        mgr.disconnect("srv-b");
        assert_eq!(mgr.connection_count(), 2);
        assert!(mgr.is_tool_available("srv-a"));
        assert!(!mgr.is_tool_available("srv-b"));
        assert!(mgr.is_tool_available("srv-c"));
    }

    // ── McpConnectionManager: reconnect ────────────────────────────

    #[tokio::test]
    async fn reconnect_transitions_through_reconnecting() {
        let mgr = McpConnectionManager::new();
        mgr.connect(&test_config("srv-1")).await.unwrap();

        // Simulate connection drop
        mgr.connections.get_mut("srv-1").unwrap().status = ConnectionStatus::Failed;
        assert!(!mgr.is_tool_available("srv-1"));

        // Reconnect should restore to Connected
        mgr.reconnect("srv-1").await.unwrap();
        assert_eq!(mgr.get_status("srv-1"), Some(ConnectionStatus::Connected));
        assert!(mgr.is_tool_available("srv-1"));
    }

    #[tokio::test]
    async fn reconnect_resets_attempt_counter() {
        let mgr = McpConnectionManager::new();
        mgr.connect(&test_config("srv-1")).await.unwrap();

        // Simulate failed state with attempts
        {
            let mut entry = mgr.connections.get_mut("srv-1").unwrap();
            entry.status = ConnectionStatus::Failed;
            entry.reconnect_attempts = 5;
        }

        mgr.reconnect("srv-1").await.unwrap();

        let conn = mgr.connections.get("srv-1").unwrap();
        assert_eq!(conn.reconnect_attempts, 0);
        assert_eq!(conn.status, ConnectionStatus::Connected);
    }

    #[tokio::test]
    async fn reconnect_unknown_server_returns_error() {
        let mgr = McpConnectionManager::new();
        let result = mgr.reconnect("nonexistent").await;
        assert!(result.is_err());
    }

    // ── McpConnectionManager: tool availability ────────────────────

    #[tokio::test]
    async fn tools_unavailable_during_reconnecting() {
        let mgr = McpConnectionManager::new();
        mgr.connect(&test_config("srv-1")).await.unwrap();

        mgr.connections.get_mut("srv-1").unwrap().status = ConnectionStatus::Reconnecting;
        assert!(!mgr.is_tool_available("srv-1"));
    }

    #[tokio::test]
    async fn tools_unavailable_when_failed() {
        let mgr = McpConnectionManager::new();
        mgr.connect(&test_config("srv-1")).await.unwrap();

        mgr.connections.get_mut("srv-1").unwrap().status = ConnectionStatus::Failed;
        assert!(!mgr.is_tool_available("srv-1"));
    }

    #[test]
    fn tools_unavailable_for_unknown_server() {
        let mgr = McpConnectionManager::new();
        assert!(!mgr.is_tool_available("nonexistent"));
    }

    // ── McpConnectionManager: reconcile ────────────────────────────

    #[tokio::test]
    async fn reconcile_connects_new_servers() {
        let mgr = McpConnectionManager::new();
        let configs = vec![test_config("srv-1"), test_config("srv-2")];

        mgr.reconcile(&configs).await;

        assert_eq!(mgr.connection_count(), 2);
        assert!(mgr.is_tool_available("srv-1"));
        assert!(mgr.is_tool_available("srv-2"));
    }

    #[tokio::test]
    async fn reconcile_disconnects_removed_servers() {
        let mgr = McpConnectionManager::new();
        mgr.connect(&test_config("srv-1")).await.unwrap();
        mgr.connect(&test_config("srv-2")).await.unwrap();
        mgr.connect(&test_config("srv-3")).await.unwrap();

        // New config only has srv-1 and srv-3
        let new_configs = vec![test_config("srv-1"), test_config("srv-3")];
        mgr.reconcile(&new_configs).await;

        assert_eq!(mgr.connection_count(), 2);
        assert!(mgr.is_tool_available("srv-1"));
        assert!(!mgr.is_tool_available("srv-2"));
        assert!(mgr.is_tool_available("srv-3"));
    }

    #[tokio::test]
    async fn reconcile_leaves_unchanged_servers_active() {
        let mgr = McpConnectionManager::new();
        let config = test_config("srv-1");
        mgr.connect(&config).await.unwrap();

        // Same config — should not disconnect/reconnect
        mgr.reconcile(&[config.clone()]).await;

        assert_eq!(mgr.connection_count(), 1);
        assert!(mgr.is_tool_available("srv-1"));
    }

    #[tokio::test]
    async fn reconcile_reconnects_changed_config() {
        let mgr = McpConnectionManager::new();
        let original = test_config("srv-1");
        mgr.connect(&original).await.unwrap();

        // Change the URL
        let mut updated = original.clone();
        updated.transport = McpTransport::Sse {
            url: "http://new-host:9090".to_string(),
        };

        mgr.reconcile(&[updated.clone()]).await;

        assert_eq!(mgr.connection_count(), 1);
        assert!(mgr.is_tool_available("srv-1"));
        // Verify config was updated
        let conn = mgr.connections.get("srv-1").unwrap();
        assert_eq!(conn.config, updated);
    }

    #[tokio::test]
    async fn reconcile_disables_previously_enabled_server() {
        let mgr = McpConnectionManager::new();
        mgr.connect(&test_config("srv-1")).await.unwrap();
        assert!(mgr.is_tool_available("srv-1"));

        let mut disabled = test_config("srv-1");
        disabled.enabled = false;

        mgr.reconcile(&[disabled]).await;

        assert_eq!(mgr.connection_count(), 0);
        assert!(!mgr.is_tool_available("srv-1"));
    }

    #[tokio::test]
    async fn reconcile_empty_configs_disconnects_all() {
        let mgr = McpConnectionManager::new();
        mgr.connect(&test_config("srv-1")).await.unwrap();
        mgr.connect(&test_config("srv-2")).await.unwrap();

        mgr.reconcile(&[]).await;

        assert_eq!(mgr.connection_count(), 0);
    }

    #[tokio::test]
    async fn reconcile_from_empty_to_new_servers() {
        let mgr = McpConnectionManager::new();
        assert_eq!(mgr.connection_count(), 0);

        // Use SSE configs since stdio requires a real binary
        let configs = vec![test_config("srv-1"), test_config("srv-2")];
        mgr.reconcile(&configs).await;

        assert_eq!(mgr.connection_count(), 2);
        assert!(mgr.is_tool_available("srv-1"));
        assert!(mgr.is_tool_available("srv-2"));
    }

    // ── Serde ──────────────────────────────────────────────────────

    #[test]
    fn config_serde_roundtrip_sse() {
        let config = McpServerConfig {
            server_id: "test".to_string(),
            transport: McpTransport::Sse {
                url: "http://localhost:8080".to_string(),
            },
            auth: Some(McpAuth::Token {
                token: "secret".to_string(),
            }),
            enabled: true,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn config_serde_roundtrip_stdio() {
        let config = McpServerConfig {
            server_id: "local".to_string(),
            transport: McpTransport::Stdio {
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "@mcp/server".to_string()],
                env: std::collections::HashMap::new(),
            },
            auth: None,
            enabled: true,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn config_serde_oauth_auth() {
        let config = McpServerConfig {
            server_id: "oauth-srv".to_string(),
            transport: McpTransport::Sse {
                url: "https://mcp.example.com".to_string(),
            },
            auth: Some(McpAuth::OAuth {
                client_id: "my-client".to_string(),
                client_secret: "my-secret".to_string(),
                token_url: "https://auth.example.com/token".to_string(),
            }),
            enabled: true,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: McpServerConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, parsed);
    }

    #[test]
    fn server_ids_returns_all_tracked() {
        let mgr = McpConnectionManager::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            mgr.connect(&test_config("a")).await.unwrap();
            mgr.connect(&test_config("b")).await.unwrap();
        });

        let mut ids = mgr.server_ids();
        ids.sort();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn discovered_tools_empty_for_unknown() {
        let mgr = McpConnectionManager::new();
        assert!(mgr.discovered_tools("nonexistent").is_empty());
    }

    #[tokio::test]
    async fn stdio_connect_with_missing_binary_sets_failed() {
        let mgr = McpConnectionManager::new();
        let config = test_stdio_config("stdio-srv");
        // connect() should not error — it tracks the connection as Failed
        mgr.connect(&config).await.unwrap();
        assert_eq!(mgr.connection_count(), 1);
        // Status should be Failed since the binary doesn't exist
        assert_eq!(mgr.get_status("stdio-srv"), Some(ConnectionStatus::Failed));
        assert!(!mgr.is_tool_available("stdio-srv"));
    }
}
