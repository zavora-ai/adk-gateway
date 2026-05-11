//! Standalone test that starts a real HTTP server with the control panel
//! routes and verifies them via HTTP requests.
//!
//! Tests JSON API endpoints since the control panel now serves a React SPA
//! for HTML and JSON API at /ui/api/*.

use adk_gateway::config::{AuthConfig, AuthMode, GatewayConfig};
use adk_gateway::control_panel::{ChannelInfo, ControlPanelState, LogEntry, SessionInfo};
use arc_swap::ArcSwap;
use std::sync::Arc;

/// Helper: build a control panel state with password auth enabled.
fn make_auth_state(password: &str) -> Arc<ControlPanelState> {
    let mut config = GatewayConfig::default();
    config.auth = Some(AuthConfig {
        mode: AuthMode::Password,
        password: Some(password.to_string()),
        token: None,
        roles: vec![],
        user_mappings: vec![],
        channel_overrides: Default::default(),
        audit: None,
        sso: None,
    });
    Arc::new(ControlPanelState::new(Arc::new(ArcSwap::from_pointee(
        config,
    ))))
}

/// Helper: build a control panel state with auth disabled (mode: none).
fn make_no_auth_state() -> Arc<ControlPanelState> {
    let config = GatewayConfig::default(); // auth is None by default
    Arc::new(ControlPanelState::new(Arc::new(ArcSwap::from_pointee(
        config,
    ))))
}

/// Helper: start a test server and return (base_url, state).
async fn start_test_server(state: Arc<ControlPanelState>) -> String {
    let app = adk_gateway::control_panel::build_routes(state.clone()).with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    format!("http://127.0.0.1:{port}")
}

/// Helper: start a test server that also includes a /health route (simulating gateway).
async fn start_test_server_with_health(state: Arc<ControlPanelState>) -> String {
    use axum::routing::get;

    let control_panel_routes =
        adk_gateway::control_panel::build_routes(state.clone()).with_state(state);

    let app = axum::Router::new()
        .route(
            "/health",
            get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }),
        )
        .merge(control_panel_routes);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn verify_control_panel_endpoints() {
    // Build minimal state
    let config = GatewayConfig::default();
    let control_panel = Arc::new(ControlPanelState::new(Arc::new(ArcSwap::from_pointee(
        config.clone(),
    ))));

    // Seed the control panel with test data
    control_panel.update_channels(vec![
        ChannelInfo {
            channel_type: "telegram".into(),
            account_id: "bot1".into(),
            status: "connected".into(),
        },
        ChannelInfo {
            channel_type: "slack".into(),
            account_id: "team-alpha".into(),
            status: "reconnecting".into(),
        },
    ]);
    control_panel.update_sessions(vec![
        SessionInfo {
            session_id: "sess-001".into(),
            user_id: "telegram:12345".into(),
            channel_type: "telegram".into(),
            last_activity: "2026-03-29T10:00:00Z".into(),
        },
        SessionInfo {
            session_id: "sess-002".into(),
            user_id: "slack:U0ABC".into(),
            channel_type: "slack".into(),
            last_activity: "2026-03-29T09:30:00Z".into(),
        },
    ]);
    for i in 0..5 {
        control_panel.push_log(LogEntry {
            timestamp: format!("2026-03-29T10:00:0{i}Z"),
            level: "INFO".into(),
            message: format!("Test log entry {i}"),
            target: Some("gateway".into()),
        });
    }

    // Build the control panel router with state
    let app = adk_gateway::control_panel::build_routes(control_panel.clone())
        .with_state(control_panel.clone());

    // Start a real TCP listener
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let port = addr.port();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Give the server a moment to start
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // ── Test /ui/api/auth/check (public, no auth required) ─────────
    let resp = client
        .get(format!("{base}/ui/api/auth/check"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "/ui/api/auth/check should return 200");
    let auth_check: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        auth_check["authenticated"], true,
        "should be authenticated when auth mode is none"
    );
    assert_eq!(
        auth_check["mode"], "none",
        "auth mode should be none for default config"
    );

    // ── Test /ui/api/dashboard (JSON) ──────────────────────────────
    let resp = client
        .get(format!("{base}/ui/api/dashboard"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "/ui/api/dashboard should return 200");
    let dashboard: serde_json::Value = resp.json().await.unwrap();
    assert!(
        dashboard["uptime_secs"].is_number(),
        "dashboard should have uptime_secs"
    );
    assert_eq!(
        dashboard["active_session_count"], 2,
        "should have 2 sessions"
    );
    assert_eq!(
        dashboard["connected_channels"].as_array().unwrap().len(),
        2,
        "should have 2 channels"
    );
    // Verify multi-account channels are listed separately (R12.4)
    let channels = dashboard["connected_channels"].as_array().unwrap();
    assert_eq!(channels[0]["channel_type"], "telegram");
    assert_eq!(channels[0]["account_id"], "bot1");
    assert_eq!(channels[1]["channel_type"], "slack");
    assert_eq!(channels[1]["account_id"], "team-alpha");

    // ── Test /ui/api/sessions (JSON) ───────────────────────────────
    let resp = client
        .get(format!("{base}/ui/api/sessions"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "/ui/api/sessions should return 200");
    let sessions: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(sessions.len(), 2, "should have 2 sessions");
    assert_eq!(sessions[0]["session_id"], "sess-001");
    assert_eq!(sessions[0]["user_id"], "telegram:12345");
    assert_eq!(sessions[0]["channel_type"], "telegram");
    assert!(sessions[0]["last_activity"].is_string());

    // ── Test /ui/api/config (redacted JSON) ────────────────────────
    let resp = client
        .get(format!("{base}/ui/api/config"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "/ui/api/config should return 200");
    let config_json: serde_json::Value = resp.json().await.unwrap();
    assert!(config_json.is_object(), "config should be a JSON object");

    // ── Test /ui/api/logs (JSON) ───────────────────────────────────
    let resp = client
        .get(format!("{base}/ui/api/logs"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "/ui/api/logs should return 200");
    let logs: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(logs.len(), 5, "should have 5 log entries");
    assert_eq!(logs[0]["level"], "INFO");
    assert!(logs[0]["message"]
        .as_str()
        .unwrap()
        .contains("Test log entry"));

    // ── Test /ui/api/agents (JSON) ─────────────────────────────────
    let resp = client
        .get(format!("{base}/ui/api/agents"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "/ui/api/agents should return 200");
    let agents: serde_json::Value = resp.json().await.unwrap();
    assert!(agents.is_array(), "agents should be an array");

    // ── Test /ui/api/memory (JSON) ─────────────────────────────────
    let resp = client
        .get(format!("{base}/ui/api/memory"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "/ui/api/memory should return 200");
    let memory: serde_json::Value = resp.json().await.unwrap();
    assert!(
        memory.get("content").is_some(),
        "memory should have content field"
    );

    // ── Test /ui/api/awp returns 404 when AWP is disabled ──────────
    let resp = client
        .get(format!("{base}/ui/api/awp"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        404,
        "/ui/api/awp should return 404 when AWP is disabled"
    );
    let awp: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(awp["ok"], false);

    // ── Test /ui/api/integrations/mcp ──────────────────────────────
    let resp = client
        .get(format!("{base}/ui/api/integrations/mcp"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "/ui/api/integrations/mcp should return 200"
    );
    let mcp: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(mcp["ok"], true);

    // ── Test /ui/api/integrations/cron ─────────────────────────────
    let resp = client
        .get(format!("{base}/ui/api/integrations/cron"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "/ui/api/integrations/cron should return 200"
    );
    let cron: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(cron["ok"], true);

    // ── Test /ui/api/integrations/tools ────────────────────────────
    let resp = client
        .get(format!("{base}/ui/api/integrations/tools"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "/ui/api/integrations/tools should return 200"
    );
    let tools: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(tools["ok"], true);

    // ── Test session terminate ──────────────────────────────────────
    let resp = client
        .post(format!("{base}/ui/api/sessions/sess-001/terminate"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "session terminate should return 200");
    let result: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(result["ok"], true, "session terminate should succeed");

    // Verify session was removed
    let resp = client
        .get(format!("{base}/ui/api/sessions"))
        .send()
        .await
        .unwrap();
    let sessions: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert_eq!(sessions.len(), 1, "should have 1 session after termination");
    assert_eq!(sessions[0]["session_id"], "sess-002");

    // ── Test terminate non-existent session ─────────────────────────
    let resp = client
        .post(format!("{base}/ui/api/sessions/nonexistent/terminate"))
        .send()
        .await
        .unwrap();
    let result: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        result["ok"], false,
        "terminating non-existent session should fail"
    );
}

// ════════════════════════════════════════════════════════════════════
// Security Regression Tests (Task 4)
// ════════════════════════════════════════════════════════════════════

/// 4.2: Password auth enabled, GET /ui/api/dashboard without cookie returns 401.
#[tokio::test]
async fn auth_enabled_dashboard_without_cookie_returns_401() {
    let state = make_auth_state("secret123");
    let base = start_test_server(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/ui/api/dashboard"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "GET /ui/api/dashboard without auth cookie should return 401"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false);
    assert!(body["message"]
        .as_str()
        .unwrap()
        .contains("Authentication required"));
}

/// 4.3: POST /ui/api/login with correct password returns session cookie,
/// then GET /ui/api/dashboard with cookie returns 200.
#[tokio::test]
async fn login_grants_access_to_protected_routes() {
    let state = make_auth_state("mypassword");
    let base = start_test_server(state).await;

    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();

    // Login with correct password
    let login_resp = client
        .post(format!("{base}/ui/api/login"))
        .json(&serde_json::json!({"password": "mypassword"}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        login_resp.status(),
        200,
        "login with correct password should return 200"
    );
    let login_body: serde_json::Value = login_resp.json().await.unwrap();
    assert_eq!(
        login_body["ok"], true,
        "login response should have ok: true"
    );

    // Now access protected route with the session cookie
    let dashboard_resp = client
        .get(format!("{base}/ui/api/dashboard"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        dashboard_resp.status(),
        200,
        "GET /ui/api/dashboard with valid session cookie should return 200"
    );
    let dashboard: serde_json::Value = dashboard_resp.json().await.unwrap();
    assert!(
        dashboard["uptime_secs"].is_number(),
        "dashboard should have uptime_secs"
    );
}

/// 4.3 (negative): POST /ui/api/login with wrong password returns 401.
#[tokio::test]
async fn login_with_wrong_password_returns_401() {
    let state = make_auth_state("correct_password");
    let base = start_test_server(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/ui/api/login"))
        .json(&serde_json::json!({"password": "wrong_password"}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        401,
        "login with wrong password should return 401"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false);
}

/// 4.4: POST /ui/api/logout invalidates session, subsequent request returns 401.
#[tokio::test]
async fn logout_invalidates_session() {
    let state = make_auth_state("logouttest");
    let base = start_test_server(state).await;

    let client = reqwest::Client::builder()
        .cookie_store(true)
        .build()
        .unwrap();

    // Login first
    let login_resp = client
        .post(format!("{base}/ui/api/login"))
        .json(&serde_json::json!({"password": "logouttest"}))
        .send()
        .await
        .unwrap();
    assert_eq!(login_resp.status(), 200);

    // Verify access works
    let resp = client
        .get(format!("{base}/ui/api/dashboard"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "should have access after login");

    // Logout
    let logout_resp = client
        .post(format!("{base}/ui/api/logout"))
        .send()
        .await
        .unwrap();
    assert_eq!(logout_resp.status(), 200, "logout should return 200");
    let logout_body: serde_json::Value = logout_resp.json().await.unwrap();
    assert_eq!(logout_body["ok"], true);

    // Subsequent request should be rejected
    let resp = client
        .get(format!("{base}/ui/api/dashboard"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "GET /ui/api/dashboard after logout should return 401"
    );
}

/// 4.5: Auth disabled, GET /ui/api/auth/check returns auth_required: false.
#[tokio::test]
async fn auth_disabled_auth_check_returns_not_required() {
    let state = make_no_auth_state();
    let base = start_test_server(state).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/ui/api/auth/check"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    // When auth is disabled, mode is "none" and authenticated is true (no auth required)
    assert_eq!(
        body["mode"], "none",
        "mode should be 'none' when auth is disabled"
    );
    assert_eq!(
        body["authenticated"], true,
        "should report authenticated when auth is disabled"
    );
}

/// 4.6: Public routes /health, /ui/api/auth/check, /ui/api/login remain accessible without auth.
#[tokio::test]
async fn public_routes_accessible_without_auth() {
    let state = make_auth_state("protected123");
    let base = start_test_server_with_health(state).await;

    let client = reqwest::Client::new();

    // /health should be accessible (it's on the unprotected gateway router)
    let resp = client.get(format!("{base}/health")).send().await.unwrap();
    assert_eq!(
        resp.status(),
        200,
        "/health should be accessible without auth"
    );

    // /ui/api/auth/check should be accessible
    let resp = client
        .get(format!("{base}/ui/api/auth/check"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        200,
        "/ui/api/auth/check should be accessible without auth"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["mode"], "password",
        "mode should reflect password auth is configured"
    );

    // /ui/api/login should be accessible (POST)
    // Even with wrong credentials, the endpoint itself should be reachable (not 401 from middleware)
    let resp = client
        .post(format!("{base}/ui/api/login"))
        .json(&serde_json::json!({"password": "wrong"}))
        .send()
        .await
        .unwrap();
    // Login with wrong password returns 401 from the handler, not from the auth guard
    assert_eq!(
        resp.status(),
        401,
        "/ui/api/login should be reachable (401 is from handler, not guard)"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["message"], "Invalid credentials",
        "should get handler-level rejection, not guard-level"
    );

    // Meanwhile, a protected route should return 401 with "Authentication required"
    let resp = client
        .get(format!("{base}/ui/api/dashboard"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Authentication required"),
        "protected route should be blocked by auth guard"
    );
}

/// 4.7: Config save with invalid content is rejected and file remains unchanged.
#[tokio::test]
async fn config_save_invalid_content_rejected() {
    // Create a temp file with valid initial config
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("gateway.json");

    let initial_config = GatewayConfig::default();
    let initial_json = serde_json::to_string_pretty(&initial_config).unwrap();
    std::fs::write(&config_path, &initial_json).unwrap();

    // Build state with config_path set (no auth so we can access the endpoint)
    let state = Arc::new(
        ControlPanelState::new(Arc::new(ArcSwap::from_pointee(initial_config.clone())))
            .with_config_path(config_path.clone()),
    );
    let base = start_test_server(state).await;

    let client = reqwest::Client::new();

    // Try to save completely invalid JSON
    let resp = client
        .post(format!("{base}/ui/api/config"))
        .json(&serde_json::json!({"content": "this is not valid json at all {{{"}))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        200,
        "endpoint should return 200 with error in body"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], false, "invalid config should be rejected");
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("Invalid configuration"),
        "error message should indicate invalid configuration"
    );

    // Verify file remains unchanged
    let file_content = std::fs::read_to_string(&config_path).unwrap();
    assert_eq!(
        file_content, initial_json,
        "config file should remain unchanged after rejected save"
    );

    // Try to save JSON that parses but fails semantic validation (port = 0)
    let mut bad_config = initial_config.clone();
    bad_config.gateway.port = 0;
    let bad_json = serde_json::to_string(&bad_config).unwrap();

    let resp = client
        .post(format!("{base}/ui/api/config"))
        .json(&serde_json::json!({"content": bad_json}))
        .send()
        .await
        .unwrap();

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["ok"], false,
        "semantically invalid config should be rejected"
    );
    assert!(
        body["message"]
            .as_str()
            .unwrap()
            .contains("validation failed"),
        "error should mention validation failure"
    );

    // Verify file still unchanged
    let file_content = std::fs::read_to_string(&config_path).unwrap();
    assert_eq!(
        file_content, initial_json,
        "config file should remain unchanged after validation failure"
    );
}

/// 4.7 (positive): Config save with valid content succeeds and updates file.
#[tokio::test]
async fn config_save_valid_content_succeeds() {
    let tmp_dir = tempfile::tempdir().unwrap();
    let config_path = tmp_dir.path().join("gateway.json");

    let initial_config = GatewayConfig::default();
    let initial_json = serde_json::to_string_pretty(&initial_config).unwrap();
    std::fs::write(&config_path, &initial_json).unwrap();

    let state = Arc::new(
        ControlPanelState::new(Arc::new(ArcSwap::from_pointee(initial_config.clone())))
            .with_config_path(config_path.clone()),
    );
    let base = start_test_server(state).await;

    let client = reqwest::Client::new();

    // Save a valid config (just the default serialized)
    let valid_json = serde_json::to_string(&initial_config).unwrap();
    let resp = client
        .post(format!("{base}/ui/api/config"))
        .json(&serde_json::json!({"content": valid_json}))
        .send()
        .await
        .unwrap();

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true, "valid config save should succeed");
    assert!(
        body["message"].as_str().unwrap().contains("saved"),
        "success message should mention saved"
    );

    // Verify file was updated (it will be re-serialized with pretty printing)
    let file_content = std::fs::read_to_string(&config_path).unwrap();
    let saved_config: GatewayConfig = serde_json::from_str(&file_content).unwrap();
    assert_eq!(
        saved_config, initial_config,
        "saved config should match the submitted config"
    );
}
