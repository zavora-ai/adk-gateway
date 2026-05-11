//! Property-based tests for webhook handler.
//!
//! Feature: gateway-production-maturity
//! - Property 21: Webhook token validation
//!   **Validates: Requirements R11.3, R11.4**
//! - Property 22: Webhook response routing
//!   **Validates: Requirements R11.2, R11.5**

use adk_gateway::channel::MessageSource;
use adk_gateway::config::HooksConfig;
use adk_gateway::webhook::{WebhookHandler, WebhookRequest};
use arc_swap::ArcSwap;
use proptest::prelude::*;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Strategy for generating non-empty ASCII tokens.
fn token_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_\\-]{4,32}".prop_map(|s| s)
}

/// Strategy for generating webhook request text.
fn text_strategy() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 ]{1,100}".prop_map(|s| s)
}

// ── Property 21: Webhook token validation ──────────────────────────
// **Validates: Requirements 11.3, 11.4**
//
// For any webhook request:
// - If hooks config specifies a token, requests with matching
//   `Authorization: Bearer <token>` should be accepted.
// - Requests with invalid or missing tokens should receive rejection.
// - If no token is configured, all requests should be accepted.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn webhook_token_validation_accepts_correct_token(
        token in token_strategy()
    ) {
        let config = HooksConfig {
            enabled: true,
            token: Some(token.clone()),
            path: None,
        };
        let (tx, _rx) = mpsc::channel(1);
        let handler = WebhookHandler::new(
            Arc::new(ArcSwap::new(Arc::new(config))),
            tx,
        );

        let auth_header = format!("Bearer {}", token);
        prop_assert!(
            handler.validate_token(Some(&auth_header)).is_ok(),
            "valid token should be accepted"
        );
    }

    #[test]
    fn webhook_token_validation_rejects_wrong_token(
        expected in token_strategy(),
        provided in token_strategy()
    ) {
        prop_assume!(expected != provided);

        let config = HooksConfig {
            enabled: true,
            token: Some(expected),
            path: None,
        };
        let (tx, _rx) = mpsc::channel(1);
        let handler = WebhookHandler::new(
            Arc::new(ArcSwap::new(Arc::new(config))),
            tx,
        );

        let auth_header = format!("Bearer {}", provided);
        prop_assert!(
            handler.validate_token(Some(&auth_header)).is_err(),
            "wrong token should be rejected"
        );
    }

    #[test]
    fn webhook_token_validation_rejects_missing_token(
        token in token_strategy()
    ) {
        let config = HooksConfig {
            enabled: true,
            token: Some(token),
            path: None,
        };
        let (tx, _rx) = mpsc::channel(1);
        let handler = WebhookHandler::new(
            Arc::new(ArcSwap::new(Arc::new(config))),
            tx,
        );

        // Missing Authorization header
        prop_assert!(
            handler.validate_token(None).is_err(),
            "missing token should be rejected when token is configured"
        );
    }

    #[test]
    fn webhook_no_token_configured_accepts_all(
        auth_value in proptest::option::of("[a-zA-Z0-9 ]{0,50}")
    ) {
        let config = HooksConfig {
            enabled: true,
            token: None,
            path: None,
        };
        let (tx, _rx) = mpsc::channel(1);
        let handler = WebhookHandler::new(
            Arc::new(ArcSwap::new(Arc::new(config))),
            tx,
        );

        prop_assert!(
            handler.validate_token(auth_value.as_deref()).is_ok(),
            "all requests should be accepted when no token is configured"
        );
    }
}

// ── Property 22: Webhook response routing ──────────────────────────
// **Validates: Requirements 11.2, 11.5**
//
// For any webhook-triggered message:
// - If the request specifies `channel` and `target`, the InboundMessage
//   metadata should contain those values for downstream delivery routing.
// - If no delivery target is specified, metadata should not contain
//   routing keys (response returned in HTTP body).
proptest! {
    #![proptest_config(ProptestConfig::with_cases(80))]

    #[test]
    fn webhook_with_delivery_target_includes_routing_metadata(
        text in text_strategy(),
        channel in "telegram|slack|discord",
        target in "[a-zA-Z0-9_]{3,20}"
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = HooksConfig {
                enabled: true,
                token: None,
                path: None,
            };
            let (tx, mut rx) = mpsc::channel(16);
            let handler = WebhookHandler::new(
                Arc::new(ArcSwap::new(Arc::new(config))),
                tx,
            );

            let req = WebhookRequest {
                text: text.clone(),
                channel: Some(channel.clone()),
                target: Some(target.clone()),
                metadata: None,
            };

            let request_id = handler.process_request(req).await.unwrap();
            prop_assert!(!request_id.is_empty());

            let msg = rx.recv().await.unwrap();
            prop_assert_eq!(&msg.text, &text);

            // Verify routing metadata is present
            let meta_channel = msg.metadata.get("webhook_channel")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            prop_assert_eq!(meta_channel, channel.as_str());

            let meta_target = msg.metadata.get("webhook_target")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            prop_assert_eq!(meta_target, target.as_str());

            // Verify source is Webhook
            match &msg.source {
                MessageSource::Webhook { request_id: rid } => {
                    prop_assert_eq!(rid, &request_id);
                }
                other => {
                    prop_assert!(false, "expected Webhook source, got {:?}", other);
                }
            }

            Ok(())
        })?;
    }

    #[test]
    fn webhook_without_delivery_target_omits_routing_metadata(
        text in text_strategy()
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let config = HooksConfig {
                enabled: true,
                token: None,
                path: None,
            };
            let (tx, mut rx) = mpsc::channel(16);
            let handler = WebhookHandler::new(
                Arc::new(ArcSwap::new(Arc::new(config))),
                tx,
            );

            let req = WebhookRequest {
                text: text.clone(),
                channel: None,
                target: None,
                metadata: None,
            };

            handler.process_request(req).await.unwrap();
            let msg = rx.recv().await.unwrap();

            prop_assert_eq!(&msg.text, &text);
            prop_assert!(
                !msg.metadata.contains_key("webhook_channel"),
                "should not have webhook_channel when no channel specified"
            );
            prop_assert!(
                !msg.metadata.contains_key("webhook_target"),
                "should not have webhook_target when no target specified"
            );

            Ok(())
        })?;
    }
}
