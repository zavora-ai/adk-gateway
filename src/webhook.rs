//! Webhook handler — inbound HTTP endpoint for external systems.
//!
//! Exposes a POST endpoint (default: `/hooks/inbound`) that accepts JSON
//! payloads and routes them to agents via the inbound message pipeline.
//!
//! Design: WebhookHandler [R11.1–R11.5]

use crate::channel::{ChannelType, InboundMessage, MessageSource};
use crate::config::HooksConfig;
use arc_swap::ArcSwap;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::mpsc;
use uuid::Uuid;

/// Maximum allowed text length for webhook payloads (64 KiB).
const MAX_WEBHOOK_TEXT_LENGTH: usize = 65_536;
/// Maximum number of metadata entries allowed.
const MAX_METADATA_ENTRIES: usize = 32;
/// Maximum length for a single metadata key or string value.
const MAX_METADATA_STRING_LENGTH: usize = 1024;

/// Webhook request body schema.
#[derive(Debug, Clone, Deserialize)]
pub struct WebhookRequest {
    pub text: String,
    pub channel: Option<String>,
    pub target: Option<String>,
    #[serde(default)]
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Webhook response body.
#[derive(Debug, Clone, Serialize)]
pub struct WebhookResponse {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// Handles inbound webhook HTTP requests with auth validation and message routing.
pub struct WebhookHandler {
    config: Arc<ArcSwap<HooksConfig>>,
    inbound_tx: mpsc::Sender<InboundMessage>,
}

impl WebhookHandler {
    /// Create a new WebhookHandler.
    pub fn new(
        config: Arc<ArcSwap<HooksConfig>>,
        inbound_tx: mpsc::Sender<InboundMessage>,
    ) -> Self {
        Self { config, inbound_tx }
    }

    /// Update the config (for hot-reload).
    pub fn update_config(&self, new_config: HooksConfig) {
        self.config.store(Arc::new(new_config));
    }

    /// Get the configured webhook path (default: `/hooks/inbound`).
    pub fn path(&self) -> String {
        let cfg = self.config.load();
        cfg.path
            .clone()
            .unwrap_or_else(|| "/hooks/inbound".to_string())
    }

    /// Validate the bearer token from an Authorization header value.
    ///
    /// Returns `Ok(())` if the token is valid or no token is configured.
    /// Returns `Err(())` if the token is invalid or missing when required.
    pub fn validate_token(&self, auth_header: Option<&str>) -> Result<(), ()> {
        let cfg = self.config.load();
        match cfg.token.as_deref() {
            None => Ok(()), // No token configured — accept all
            Some(expected) => {
                let provided = auth_header.and_then(|v| v.strip_prefix("Bearer "));
                match provided {
                    Some(token) => {
                        // Constant-time comparison to prevent timing attacks
                        if token.as_bytes().ct_eq(expected.as_bytes()).into() {
                            Ok(())
                        } else {
                            Err(())
                        }
                    }
                    None => Err(()),
                }
            }
        }
    }

    /// Process a validated webhook request: build an InboundMessage and route it.
    ///
    /// Returns the request_id assigned to this webhook message.
    /// Validate the webhook request payload for size and content limits.
    pub fn validate_request(req: &WebhookRequest) -> Result<(), WebhookError> {
        if req.text.len() > MAX_WEBHOOK_TEXT_LENGTH {
            return Err(WebhookError::PayloadTooLarge {
                field: "text".into(),
                max: MAX_WEBHOOK_TEXT_LENGTH,
            });
        }
        if let Some(ref meta) = req.metadata {
            if meta.len() > MAX_METADATA_ENTRIES {
                return Err(WebhookError::PayloadTooLarge {
                    field: "metadata entries".into(),
                    max: MAX_METADATA_ENTRIES,
                });
            }
            for (k, v) in meta {
                if k.len() > MAX_METADATA_STRING_LENGTH {
                    return Err(WebhookError::PayloadTooLarge {
                        field: format!("metadata key '{}'", &k[..k.len().min(32)]),
                        max: MAX_METADATA_STRING_LENGTH,
                    });
                }
                if let Some(s) = v.as_str() {
                    if s.len() > MAX_METADATA_STRING_LENGTH {
                        return Err(WebhookError::PayloadTooLarge {
                            field: format!("metadata value for '{}'", &k[..k.len().min(32)]),
                            max: MAX_METADATA_STRING_LENGTH,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Process a validated webhook request: build an InboundMessage and route it.
    ///
    /// Returns the request_id assigned to this webhook message.
    pub async fn process_request(&self, req: WebhookRequest) -> Result<String, WebhookError> {
        // Validate payload before processing
        Self::validate_request(&req)?;

        let request_id = Uuid::new_v4().to_string();

        let mut metadata = req.metadata.unwrap_or_default();
        if let Some(ref channel) = req.channel {
            metadata.insert(
                "webhook_channel".to_string(),
                serde_json::Value::String(channel.clone()),
            );
        }
        if let Some(ref target) = req.target {
            metadata.insert(
                "webhook_target".to_string(),
                serde_json::Value::String(target.clone()),
            );
        }

        let msg = InboundMessage {
            channel_type: ChannelType::Webhook,
            account_id: String::new(),
            sender_id: "webhook".to_string(),
            sender_name: Some("webhook".to_string()),
            text: req.text,
            is_group: false,
            group_id: None,
            is_mention: false,
            platform_message_id: request_id.clone(),
            attachments: vec![],
            metadata,
            source: MessageSource::Webhook {
                request_id: request_id.clone(),
            },
            timestamp: Utc::now(),
        };

        self.inbound_tx
            .send(msg)
            .await
            .map_err(|_| WebhookError::ChannelClosed)?;

        Ok(request_id)
    }

    /// Check whether webhooks are enabled in the current config.
    pub fn is_enabled(&self) -> bool {
        self.config.load().enabled
    }
}

/// Errors that can occur during webhook processing.
#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error("inbound channel closed")]
    ChannelClosed,
    #[error("unauthorized")]
    #[allow(dead_code)] // Available for webhook auth error reporting
    Unauthorized,
    #[error("webhooks disabled")]
    #[allow(dead_code)] // Available for webhook disabled error reporting
    Disabled,
    #[error("payload too large: {field} exceeds max {max}")]
    PayloadTooLarge { field: String, max: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_handler() -> (WebhookHandler, mpsc::Receiver<InboundMessage>) {
        let config = HooksConfig {
            enabled: true,
            token: Some("secret-token".to_string()),
            path: None,
        };
        let (tx, rx) = mpsc::channel(16);
        let handler = WebhookHandler::new(Arc::new(ArcSwap::new(Arc::new(config))), tx);
        (handler, rx)
    }

    #[test]
    fn test_validate_token_valid() {
        let (handler, _rx) = make_handler();
        assert!(handler.validate_token(Some("Bearer secret-token")).is_ok());
    }

    #[test]
    fn test_validate_token_invalid() {
        let (handler, _rx) = make_handler();
        assert!(handler.validate_token(Some("Bearer wrong-token")).is_err());
    }

    #[test]
    fn test_validate_token_missing() {
        let (handler, _rx) = make_handler();
        assert!(handler.validate_token(None).is_err());
    }

    #[test]
    fn test_validate_token_no_bearer_prefix() {
        let (handler, _rx) = make_handler();
        assert!(handler.validate_token(Some("secret-token")).is_err());
    }

    #[test]
    fn test_validate_token_no_config_token() {
        let config = HooksConfig {
            enabled: true,
            token: None,
            path: None,
        };
        let (tx, _rx) = mpsc::channel(16);
        let handler = WebhookHandler::new(Arc::new(ArcSwap::new(Arc::new(config))), tx);
        // No token configured — all requests accepted
        assert!(handler.validate_token(None).is_ok());
        assert!(handler.validate_token(Some("Bearer anything")).is_ok());
    }

    #[test]
    fn test_default_path() {
        let (handler, _rx) = make_handler();
        assert_eq!(handler.path(), "/hooks/inbound");
    }

    #[test]
    fn test_custom_path() {
        let config = HooksConfig {
            enabled: true,
            token: None,
            path: Some("/api/webhooks".to_string()),
        };
        let (tx, _rx) = mpsc::channel(16);
        let handler = WebhookHandler::new(Arc::new(ArcSwap::new(Arc::new(config))), tx);
        assert_eq!(handler.path(), "/api/webhooks");
    }

    #[tokio::test]
    async fn test_process_request_routes_message() {
        let (handler, mut rx) = make_handler();
        let req = WebhookRequest {
            text: "hello from webhook".to_string(),
            channel: Some("telegram".to_string()),
            target: Some("user123".to_string()),
            metadata: None,
        };

        let request_id = handler.process_request(req).await.unwrap();
        assert!(!request_id.is_empty());

        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.text, "hello from webhook");
        assert_eq!(msg.channel_type, ChannelType::Webhook);
        assert!(matches!(msg.source, MessageSource::Webhook { .. }));
        assert_eq!(
            msg.metadata.get("webhook_channel").and_then(|v| v.as_str()),
            Some("telegram")
        );
        assert_eq!(
            msg.metadata.get("webhook_target").and_then(|v| v.as_str()),
            Some("user123")
        );
    }

    #[tokio::test]
    async fn test_process_request_no_delivery_target() {
        let (handler, mut rx) = make_handler();
        let req = WebhookRequest {
            text: "just a question".to_string(),
            channel: None,
            target: None,
            metadata: Some(HashMap::from([(
                "source".to_string(),
                serde_json::Value::String("ci".to_string()),
            )])),
        };

        let _request_id = handler.process_request(req).await.unwrap();
        let msg = rx.recv().await.unwrap();
        assert_eq!(msg.text, "just a question");
        assert!(!msg.metadata.contains_key("webhook_channel"));
        assert!(!msg.metadata.contains_key("webhook_target"));
        assert_eq!(
            msg.metadata.get("source").and_then(|v| v.as_str()),
            Some("ci")
        );
    }

    #[test]
    fn test_is_enabled() {
        let (handler, _rx) = make_handler();
        assert!(handler.is_enabled());
    }

    #[test]
    fn test_is_disabled() {
        let config = HooksConfig {
            enabled: false,
            token: None,
            path: None,
        };
        let (tx, _rx) = mpsc::channel(16);
        let handler = WebhookHandler::new(Arc::new(ArcSwap::new(Arc::new(config))), tx);
        assert!(!handler.is_enabled());
    }
}
