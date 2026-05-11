//! Unit tests for the Telegram channel probe functionality.
//!
//! Uses `wiremock` to mock HTTP responses from the Telegram Bot API
//! and verifies each `ProbeResult` variant is produced correctly.

use adk_gateway::channel::telegram::{ProbeResult, TelegramChannel};
use adk_gateway::config::TelegramConfig;
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Helper to create a minimal TelegramConfig with the given bot token.
fn test_config(bot_token: &str) -> TelegramConfig {
    TelegramConfig {
        enabled: true,
        account_id: "test".to_string(),
        bot_token: bot_token.to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn probe_connected_returns_bot_username() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path_regex(r"/bot.+/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "id": 123456789,
                "is_bot": true,
                "first_name": "TestBot",
                "username": "test_bot"
            }
        })))
        .mount(&mock_server)
        .await;

    let config = test_config("fake-token-123");
    let channel = TelegramChannel::new(config);
    let result = channel.probe_with_base_url(&mock_server.uri()).await;

    assert_eq!(
        result,
        ProbeResult::Connected {
            bot_username: "test_bot".to_string()
        }
    );
}

#[tokio::test]
async fn probe_invalid_token_returns_invalid_token() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path_regex(r"/bot.+/getMe"))
        .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
            "ok": false,
            "error_code": 401,
            "description": "Unauthorized"
        })))
        .mount(&mock_server)
        .await;

    let config = test_config("invalid-token");
    let channel = TelegramChannel::new(config);
    let result = channel.probe_with_base_url(&mock_server.uri()).await;

    assert_eq!(result, ProbeResult::InvalidToken);
}

#[tokio::test]
async fn probe_unreachable_on_connection_error() {
    // Use a URL that will fail to connect (port that's not listening)
    let config = test_config("some-token");
    let channel = TelegramChannel::new(config);
    // Use a non-routable address to trigger a connection error quickly
    let result = channel.probe_with_base_url("http://127.0.0.1:1").await;

    match result {
        ProbeResult::Unreachable { timeout_ms } => {
            assert_eq!(timeout_ms, 10000);
        }
        other => panic!("expected Unreachable, got {:?}", other),
    }
}

#[tokio::test]
async fn probe_error_on_unexpected_status() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path_regex(r"/bot.+/getMe"))
        .respond_with(ResponseTemplate::new(500).set_body_json(serde_json::json!({
            "ok": false,
            "error_code": 500,
            "description": "Internal Server Error"
        })))
        .mount(&mock_server)
        .await;

    let config = test_config("some-token");
    let channel = TelegramChannel::new(config);
    let result = channel.probe_with_base_url(&mock_server.uri()).await;

    match result {
        ProbeResult::Error { message } => {
            assert!(
                message.contains("500"),
                "expected error message to contain '500', got: {message}"
            );
        }
        other => panic!("expected Error, got {:?}", other),
    }
}

#[tokio::test]
async fn probe_connected_with_missing_username_defaults_to_unknown() {
    let mock_server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path_regex(r"/bot.+/getMe"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "ok": true,
            "result": {
                "id": 123456789,
                "is_bot": true,
                "first_name": "TestBot"
            }
        })))
        .mount(&mock_server)
        .await;

    let config = test_config("fake-token-123");
    let channel = TelegramChannel::new(config);
    let result = channel.probe_with_base_url(&mock_server.uri()).await;

    assert_eq!(
        result,
        ProbeResult::Connected {
            bot_username: "unknown".to_string()
        }
    );
}
