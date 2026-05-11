//! AWP protocol integration tests.
//!
//! These tests start a real HTTP server with AWP endpoints enabled and
//! exercise the full request/response cycle for every AWP endpoint.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

/// Build a test router with AWP endpoints from a temporary business.toml.
async fn build_test_router() -> axum::Router {
    let dir = tempfile::tempdir().unwrap();
    let toml_path = dir.path().join("business.toml");
    std::fs::write(
        &toml_path,
        r#"
site_name = "AWP Integration Test"
site_description = "Test site for AWP integration tests"
domain = "test.localhost"

[[capabilities]]
name = "greet"
description = "Greet a visitor"
endpoint = "/api/greet"
method = "GET"
access_level = "anonymous"

[[capabilities]]
name = "order"
description = "Place an order"
endpoint = "/api/order"
method = "POST"
access_level = "known"

[[policies]]
name = "privacy"
description = "Test privacy policy"
policy_type = "privacy"
"#,
    )
    .unwrap();

    let cfg = adk_gateway::awp::AwpConfig {
        enabled: true,
        business_toml: std::path::PathBuf::from("business.toml"),
        hot_reload: false,
        consent_file: std::path::PathBuf::from("consent.json"),
    };
    let state = adk_gateway::awp::build_awp_state(&cfg, dir.path())
        .await
        .unwrap()
        .expect("AWP state should be Some when enabled with valid toml");

    // Leak the tempdir so it lives for the duration of the test.
    // (The router holds an Arc to the loaded context, so the file isn't needed after load.)
    std::mem::forget(dir);

    adk_gateway::awp::merge_awp_routes(axum::Router::new(), Some(state))
}

/// Helper: send a GET request and return (status, body_json).
async fn get(router: &axum::Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = axum::body::to_bytes(resp.into_body(), 1024 * 64)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::json!(null));
    (status, json)
}

/// Helper: send a POST request with JSON body and return (status, body_json).
async fn post(
    router: &axum::Router,
    uri: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 64)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or(serde_json::json!(null));
    (status, json)
}

/// Helper: send a DELETE request and return status.
async fn delete(router: &axum::Router, uri: &str) -> StatusCode {
    let req = Request::builder()
        .method("DELETE")
        .uri(uri)
        .body(Body::empty())
        .unwrap();
    router.clone().oneshot(req).await.unwrap().status()
}

// ── Discovery ──────────────────────────────────────────────────────

#[tokio::test]
async fn test_discovery_endpoint() {
    let router = build_test_router().await;
    let (status, body) = get(&router, "/.well-known/awp.json").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["siteName"], "AWP Integration Test");
    assert_eq!(
        body["siteDescription"],
        "Test site for AWP integration tests"
    );
    assert!(body["capabilityManifestUrl"].is_string());
    assert!(body["a2aEndpointUrl"].is_string());
    assert!(body["healthEndpointUrl"].is_string());
}

// ── Manifest ───────────────────────────────────────────────────────

#[tokio::test]
async fn test_manifest_endpoint() {
    let router = build_test_router().await;
    let (status, body) = get(&router, "/awp/manifest").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["@context"], "https://schema.org");
    assert_eq!(body["@type"], "WebAPI");
    assert_eq!(body["name"], "AWP Integration Test");

    let caps = body["capabilities"].as_array().unwrap();
    assert_eq!(caps.len(), 2);
    assert_eq!(caps[0]["name"], "greet");
    assert_eq!(caps[1]["name"], "order");
    assert_eq!(caps[1]["method"], "POST");
}

// ── Health ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_health_endpoint() {
    let router = build_test_router().await;
    let (status, body) = get(&router, "/awp/health").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"], "healthy");
    assert!(body["message"].is_string());
    assert!(body["timestamp"].is_string());
}

// ── A2A ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_a2a_message() {
    let router = build_test_router().await;
    let (status, body) = post(
        &router,
        "/awp/a2a",
        serde_json::json!({
            "id": "msg-42",
            "type": "awp:InvokeCapability",
            "payload": { "capability": "greet" }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "acknowledged");
    assert_eq!(body["messageId"], "msg-42");
}

// ── Events ─────────────────────────────────────────────────────────

#[tokio::test]
async fn test_event_subscription_lifecycle() {
    let router = build_test_router().await;

    // Create subscription
    let (status, body) = post(
        &router,
        "/awp/events/subscribe",
        serde_json::json!({
            "subscriber": "test-agent",
            "callbackUrl": "https://example.com/hook",
            "eventTypes": ["health.changed"],
            "secret": "test-secret"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let sub_id = body["id"].as_str().unwrap().to_string();
    assert!(!sub_id.is_empty());

    // List subscriptions
    let (status, body) = get(&router, "/awp/events/subscriptions").await;
    assert_eq!(status, StatusCode::OK);
    let subs = body.as_array().unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0]["subscriber"], "test-agent");

    // Delete subscription
    let status = delete(&router, &format!("/awp/events/subscriptions/{sub_id}")).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Verify deleted
    let (_, body) = get(&router, "/awp/events/subscriptions").await;
    assert!(body.as_array().unwrap().is_empty());
}

// ── Consent ────────────────────────────────────────────────────────

#[tokio::test]
async fn test_consent_lifecycle() {
    let router = build_test_router().await;

    // Check — no consent yet
    let (status, body) = get(
        &router,
        "/awp/consent/check?subject=user-1&purpose=analytics",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["consented"], false);

    // Capture consent
    let (status, body) = post(
        &router,
        "/awp/consent",
        serde_json::json!({ "subject": "user-1", "purpose": "analytics" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["status"], "captured");

    // Check — now consented
    let (_, body) = get(
        &router,
        "/awp/consent/check?subject=user-1&purpose=analytics",
    )
    .await;
    assert_eq!(body["consented"], true);

    // Revoke
    let (status, body) = post(
        &router,
        "/awp/consent/revoke",
        serde_json::json!({ "subject": "user-1", "purpose": "analytics" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "revoked");

    // Check — no longer consented
    let (_, body) = get(
        &router,
        "/awp/consent/check?subject=user-1&purpose=analytics",
    )
    .await;
    assert_eq!(body["consented"], false);
}

#[tokio::test]
async fn test_consent_validation() {
    let router = build_test_router().await;

    // Empty subject should fail
    let (status, body) = post(
        &router,
        "/awp/consent",
        serde_json::json!({ "subject": "", "purpose": "analytics" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].is_string());

    // Empty purpose should fail
    let (status, _) = post(
        &router,
        "/awp/consent",
        serde_json::json!({ "subject": "user-1", "purpose": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

// ── Version Negotiation ────────────────────────────────────────────

#[tokio::test]
async fn test_version_negotiation_compatible() {
    let router = build_test_router().await;

    let req = Request::builder()
        .uri("/.well-known/awp.json")
        .header("AWP-Version", "1.0")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers().get("AWP-Version").unwrap().to_str().unwrap(),
        "1.0"
    );
}

#[tokio::test]
async fn test_version_negotiation_incompatible() {
    let router = build_test_router().await;

    let req = Request::builder()
        .uri("/.well-known/awp.json")
        .header("AWP-Version", "2.0")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_ACCEPTABLE);
}

// ── 404 for unknown paths ──────────────────────────────────────────

#[tokio::test]
async fn test_unknown_awp_path_returns_404() {
    let router = build_test_router().await;

    let req = Request::builder()
        .uri("/awp/nonexistent")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
