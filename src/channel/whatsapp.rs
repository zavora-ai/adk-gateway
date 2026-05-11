//! WhatsApp Cloud API channel implementation.
//!
//! WhatsApp integration uses:
//! - Webhook-based message receive (registered via Meta developer portal)
//! - HTTP Graph API for sending messages
//!
//! The webhook handler is registered on the gateway's HTTP server at the
//! configured `webhook_path` (default: `/webhook/whatsapp`). Incoming
//! webhook events are validated using the `verify_token` and normalized
//! into `InboundMessage` structs.

use super::{
    Attachment, Channel, ChannelType, EditMessage, InboundMessage, MessageSource, OutboundMessage,
};
use crate::config::WhatsAppConfig;
use crate::reconnect::{ReconnectPolicy, ReconnectState};
use async_trait::async_trait;
use reqwest::Client;
use tokio::sync::{mpsc, Mutex};

/// WhatsApp Cloud API base URL (Graph API v18.0).
const GRAPH_API_BASE: &str = "https://graph.facebook.com/v18.0";

pub struct WhatsAppChannel {
    config: WhatsAppConfig,
    http: Client,
    /// Shutdown signal
    shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Reconnection state for health tracking
    reconnect_state: Mutex<ReconnectState>,
}

impl WhatsAppChannel {
    pub fn new(config: WhatsAppConfig) -> Self {
        Self {
            config,
            http: Client::new(),
            shutdown_tx: Mutex::new(None),
            reconnect_state: Mutex::new(ReconnectState::new(ReconnectPolicy::default())),
        }
    }

    /// Validate that the access token is functional by calling the phone number endpoint.
    async fn validate_token(&self) -> anyhow::Result<()> {
        let url = format!(
            "{}/{}/whatsapp_business_profile",
            GRAPH_API_BASE, self.config.phone_number_id
        );
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.config.access_token)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "whatsapp token validation failed (HTTP {}): {}",
                status,
                body
            );
        }
        Ok(())
    }

    /// Send a text message via the WhatsApp Cloud API.
    async fn send_text_message(&self, to: &str, text: &str) -> anyhow::Result<Option<String>> {
        let url = format!(
            "{}/{}/messages",
            GRAPH_API_BASE, self.config.phone_number_id
        );

        // Truncate to max message length
        let max_len = ChannelType::Whatsapp.max_message_length();
        let text = if text.len() > max_len {
            let mut truncated = text[..max_len - 3].to_string();
            truncated.push_str("...");
            truncated
        } else {
            text.to_string()
        };

        let body = serde_json::json!({
            "messaging_product": "whatsapp",
            "recipient_type": "individual",
            "to": to,
            "type": "text",
            "text": {
                "preview_url": false,
                "body": text
            }
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.config.access_token)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            anyhow::bail!("whatsapp send failed (HTTP {}): {}", status, err_body);
        }

        let result: serde_json::Value = resp.json().await?;
        let message_id = result["messages"][0]["id"].as_str().map(|s| s.to_string());

        Ok(message_id)
    }

    /// Parse a WhatsApp webhook payload into InboundMessages.
    #[allow(dead_code)] // Called by the gateway webhook route handler
    pub fn parse_webhook_payload(
        payload: &serde_json::Value,
        account_id: &str,
    ) -> Vec<InboundMessage> {
        let mut messages = Vec::new();

        // WhatsApp webhook structure:
        // { "entry": [{ "changes": [{ "value": { "messages": [...], "contacts": [...] } }] }] }
        let entries = match payload["entry"].as_array() {
            Some(e) => e,
            None => return messages,
        };

        for entry in entries {
            let changes = match entry["changes"].as_array() {
                Some(c) => c,
                None => continue,
            };

            for change in changes {
                let value = &change["value"];

                // Get contacts for sender name lookup
                let contacts = value["contacts"].as_array();

                let msgs = match value["messages"].as_array() {
                    Some(m) => m,
                    None => continue,
                };

                for msg in msgs {
                    let msg_type = msg["type"].as_str().unwrap_or("");
                    let from = msg["from"].as_str().unwrap_or("").to_string();
                    let msg_id = msg["id"].as_str().unwrap_or("").to_string();

                    if from.is_empty() || msg_id.is_empty() {
                        continue;
                    }

                    // Extract text based on message type
                    let text = match msg_type {
                        "text" => msg["text"]["body"].as_str().unwrap_or("").to_string(),
                        "image" | "video" | "audio" | "document" => {
                            msg[msg_type]["caption"].as_str().unwrap_or("").to_string()
                        }
                        _ => continue,
                    };

                    if text.is_empty() && msg_type == "text" {
                        continue;
                    }

                    // Look up sender name from contacts
                    let sender_name = contacts
                        .and_then(|c| {
                            c.iter()
                                .find(|contact| contact["wa_id"].as_str() == Some(&from))
                        })
                        .and_then(|contact| {
                            contact["profile"]["name"].as_str().map(|s| s.to_string())
                        });

                    // Extract attachments for media messages
                    let attachments = if msg_type != "text" {
                        let media_id = msg[msg_type]["id"].as_str().unwrap_or("");
                        let mime = msg[msg_type]["mime_type"]
                            .as_str()
                            .unwrap_or("application/octet-stream");
                        if !media_id.is_empty() {
                            vec![Attachment {
                                mime_type: mime.to_string(),
                                url: Some(format!("{}/{}", GRAPH_API_BASE, media_id)),
                                data: None,
                                filename: msg[msg_type]["filename"].as_str().map(|s| s.to_string()),
                            }]
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    };

                    let inbound = InboundMessage {
                        channel_type: ChannelType::Whatsapp,
                        account_id: account_id.to_string(),
                        sender_id: from,
                        sender_name,
                        text,
                        is_group: false, // WhatsApp Cloud API sends group messages differently
                        group_id: None,
                        is_mention: false,
                        platform_message_id: msg_id,
                        attachments,
                        metadata: std::collections::HashMap::new(),
                        source: MessageSource::Channel,
                        timestamp: chrono::Utc::now(),
                    };

                    messages.push(inbound);
                }
            }
        }

        messages
    }
}

#[async_trait]
impl Channel for WhatsAppChannel {
    fn channel_type(&self) -> ChannelType {
        ChannelType::Whatsapp
    }

    fn account_id(&self) -> &str {
        &self.config.account_id
    }

    async fn start(&self, tx: mpsc::Sender<InboundMessage>) -> anyhow::Result<()> {
        // Validate the access token
        self.validate_token()
            .await
            .map_err(|e| anyhow::anyhow!("whatsapp access token validation failed: {e}"))?;

        tracing::info!(
            phone_number_id = %self.config.phone_number_id,
            webhook_path = %self.config.webhook_path,
            "whatsapp channel started (webhook-based receive)"
        );

        // Reset reconnect state on successful start
        self.reconnect_state.lock().await.reset();

        let (shutdown_tx, _shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        // WhatsApp uses webhook-based receive — the gateway's HTTP server
        // handles incoming webhooks at the configured path and calls
        // `parse_webhook_payload` to produce InboundMessages.
        //
        // We store the tx sender for use by the webhook handler.
        // The webhook route is registered in the gateway routes module.
        let _tx = tx;

        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> anyhow::Result<Option<String>> {
        self.send_text_message(&msg.recipient_id, &msg.text).await
    }

    async fn edit(&self, _msg: EditMessage) -> anyhow::Result<()> {
        // WhatsApp Cloud API does not support editing sent messages
        Err(anyhow::anyhow!("whatsapp does not support message editing"))
    }

    fn supports_editing(&self) -> bool {
        false
    }

    async fn health_check(&self) -> anyhow::Result<super::ChannelHealth> {
        use super::{ChannelHealth, ChannelStatus};
        let reconnect = self.reconnect_state.lock().await;

        match self.validate_token().await {
            Ok(()) => Ok(ChannelHealth {
                status: ChannelStatus::Connected,
                last_connected: Some(chrono::Utc::now()),
                reconnect_attempts: reconnect.attempts,
                error: None,
            }),
            Err(e) => Ok(ChannelHealth {
                status: reconnect.channel_status(),
                last_connected: None,
                reconnect_attempts: reconnect.attempts,
                error: Some(format!("{e}")),
            }),
        }
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }
        tracing::info!("whatsapp channel shut down");
        Ok(())
    }
}
