//! Matrix channel implementation using Client-Server API.
//!
//! Matrix integration uses:
//! - Long-polling `/sync` endpoint for receiving messages
//! - HTTP PUT `/_matrix/client/v3/rooms/{roomId}/send/{eventType}/{txnId}` for sending
//!
//! The bot connects to the configured homeserver, performs an initial sync
//! to establish a `since` token, then long-polls for new events. Incoming
//! `m.room.message` events are normalized into `InboundMessage` structs.

use super::{Channel, ChannelType, EditMessage, InboundMessage, MessageSource, OutboundMessage};
use crate::config::MatrixConfig;
use crate::reconnect::{ReconnectPolicy, ReconnectState};
use async_trait::async_trait;
use reqwest::Client;
use tokio::sync::{mpsc, Mutex};

pub struct MatrixChannel {
    config: MatrixConfig,
    http: Client,
    /// Shutdown signal
    shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Reconnection state
    reconnect_state: Mutex<ReconnectState>,
}

impl MatrixChannel {
    pub fn new(config: MatrixConfig) -> Self {
        Self {
            config,
            http: Client::new(),
            shutdown_tx: Mutex::new(None),
            reconnect_state: Mutex::new(ReconnectState::new(ReconnectPolicy::default())),
        }
    }

    /// Build a Matrix API URL from the homeserver base and path.
    fn api_url(&self, path: &str) -> String {
        let base = self.config.homeserver_url.trim_end_matches('/');
        format!("{}{}", base, path)
    }

    /// Validate the access token by calling GET /_matrix/client/v3/account/whoami.
    async fn validate_token(&self) -> anyhow::Result<String> {
        let url = self.api_url("/_matrix/client/v3/account/whoami");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.config.access_token)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("matrix token validation failed (HTTP {}): {}", status, body);
        }

        let body: serde_json::Value = resp.json().await?;
        let user_id = body["user_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no user_id in whoami response"))?
            .to_string();

        tracing::info!(user_id = %user_id, "matrix bot authenticated");
        Ok(user_id)
    }

    /// Send a text message to a Matrix room.
    async fn send_room_message(&self, room_id: &str, text: &str) -> anyhow::Result<Option<String>> {
        let txn_id = uuid::Uuid::new_v4().to_string();
        let url = self.api_url(&format!(
            "/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
            room_id, txn_id
        ));

        // Truncate to max message length
        let max_len = ChannelType::Matrix.max_message_length();
        let text = if text.len() > max_len {
            let mut truncated = text[..max_len - 3].to_string();
            truncated.push_str("...");
            truncated
        } else {
            text.to_string()
        };

        let body = serde_json::json!({
            "msgtype": "m.text",
            "body": text,
        });

        let resp = self
            .http
            .put(&url)
            .bearer_auth(&self.config.access_token)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            anyhow::bail!("matrix send failed (HTTP {}): {}", status, err_body);
        }

        let result: serde_json::Value = resp.json().await?;
        let event_id = result["event_id"].as_str().map(|s| s.to_string());
        Ok(event_id)
    }

    /// Edit a message by sending a replacement event (m.new_content).
    async fn edit_room_message(
        &self,
        room_id: &str,
        event_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let txn_id = uuid::Uuid::new_v4().to_string();
        let url = self.api_url(&format!(
            "/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
            room_id, txn_id
        ));

        // Truncate to max message length
        let max_len = ChannelType::Matrix.max_message_length();
        let text = if text.len() > max_len {
            let mut truncated = text[..max_len - 3].to_string();
            truncated.push_str("...");
            truncated
        } else {
            text.to_string()
        };

        let body = serde_json::json!({
            "msgtype": "m.text",
            "body": format!("* {}", text),
            "m.new_content": {
                "msgtype": "m.text",
                "body": text,
            },
            "m.relates_to": {
                "rel_type": "m.replace",
                "event_id": event_id,
            }
        });

        let resp = self
            .http
            .put(&url)
            .bearer_auth(&self.config.access_token)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            anyhow::bail!("matrix edit failed (HTTP {}): {}", status, err_body);
        }

        Ok(())
    }

    /// Perform a /sync request with the given since token and timeout.
    #[allow(dead_code)] // Available for direct sync calls outside the long-polling loop
    async fn sync(
        &self,
        since: Option<&str>,
        timeout_ms: u64,
    ) -> anyhow::Result<serde_json::Value> {
        let mut url = self.api_url("/_matrix/client/v3/sync");
        url.push_str(&format!("?timeout={}", timeout_ms));

        if let Some(since_token) = since {
            url.push_str(&format!("&since={}", since_token));
        }

        // Filter to only get m.room.message events
        let filter = serde_json::json!({
            "room": {
                "timeline": {
                    "types": ["m.room.message"],
                    "limit": 50
                },
                "state": {
                    "types": []
                }
            },
            "presence": {
                "types": []
            }
        });
        url.push_str(&format!(
            "&filter={}",
            urlencoding::encode(&filter.to_string())
        ));

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.config.access_token)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("matrix sync failed (HTTP {}): {}", status, body);
        }

        let body: serde_json::Value = resp.json().await?;
        Ok(body)
    }
}

#[async_trait]
impl Channel for MatrixChannel {
    fn channel_type(&self) -> ChannelType {
        ChannelType::Matrix
    }

    fn account_id(&self) -> &str {
        &self.config.account_id
    }

    async fn start(&self, tx: mpsc::Sender<InboundMessage>) -> anyhow::Result<()> {
        // Validate the access token
        let bot_user_id = self.validate_token().await?;

        // Reset reconnect state on successful start
        self.reconnect_state.lock().await.reset();

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        let config = self.config.clone();
        let http = self.http.clone();
        let reconnect_policy = self.reconnect_state.lock().await.policy().clone();

        let log_user_id = config.user_id.clone();
        let log_homeserver = config.homeserver_url.clone();

        tokio::spawn(async move {
            let mut reconnect_state = ReconnectState::new(reconnect_policy);
            let mut since_token: Option<String> = None;

            // Initial sync to get the since token (don't process old messages)
            let base_url = config.homeserver_url.trim_end_matches('/').to_string();
            let initial_url = format!(
                "{}/_matrix/client/v3/sync?timeout=0&filter={}",
                base_url,
                urlencoding::encode(
                    &serde_json::json!({
                        "room": { "timeline": { "limit": 0 } },
                        "presence": { "types": [] }
                    })
                    .to_string()
                )
            );

            match http
                .get(&initial_url)
                .bearer_auth(&config.access_token)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(body) = resp.json::<serde_json::Value>().await {
                        since_token = body["next_batch"].as_str().map(|s| s.to_string());
                        tracing::info!(since = ?since_token, "matrix initial sync complete");
                    }
                }
                Ok(resp) => {
                    tracing::error!(status = %resp.status(), "matrix initial sync failed");
                }
                Err(e) => {
                    tracing::error!(error = %e, "matrix initial sync request failed");
                }
            }

            // Long-polling sync loop
            loop {
                // Build sync URL
                let mut sync_url = format!("{}/_matrix/client/v3/sync?timeout=30000", base_url);
                if let Some(ref token) = since_token {
                    sync_url.push_str(&format!("&since={}", token));
                }

                // Filter for room messages only
                let filter = serde_json::json!({
                    "room": {
                        "timeline": {
                            "types": ["m.room.message"],
                            "limit": 50
                        },
                        "state": { "types": [] }
                    },
                    "presence": { "types": [] }
                });
                sync_url.push_str(&format!(
                    "&filter={}",
                    urlencoding::encode(&filter.to_string())
                ));

                let sync_future = http.get(&sync_url).bearer_auth(&config.access_token).send();

                tokio::select! {
                    result = sync_future => {
                        match result {
                            Ok(resp) if resp.status().is_success() => {
                                reconnect_state.reset();

                                let body: serde_json::Value = match resp.json().await {
                                    Ok(b) => b,
                                    Err(e) => {
                                        tracing::warn!(error = %e, "failed to parse matrix sync response");
                                        continue;
                                    }
                                };

                                // Update since token
                                if let Some(token) = body["next_batch"].as_str() {
                                    since_token = Some(token.to_string());
                                }

                                // Process room events
                                if let Some(rooms) = body["rooms"]["join"].as_object() {
                                    for (room_id, room_data) in rooms {
                                        // Filter by configured room_ids if specified
                                        if !config.room_ids.is_empty()
                                            && !config.room_ids.contains(room_id)
                                        {
                                            continue;
                                        }

                                        let events = match room_data["timeline"]["events"].as_array() {
                                            Some(e) => e,
                                            None => continue,
                                        };

                                        for event in events {
                                            let sender = event["sender"].as_str().unwrap_or("");

                                            // Skip bot's own messages
                                            if sender == bot_user_id {
                                                continue;
                                            }

                                            let event_type = event["type"].as_str().unwrap_or("");
                                            if event_type != "m.room.message" {
                                                continue;
                                            }

                                            let content = &event["content"];
                                            let msgtype = content["msgtype"].as_str().unwrap_or("");

                                            // Only handle text messages
                                            if msgtype != "m.text" {
                                                continue;
                                            }

                                            // Skip edits (m.new_content)
                                            if content["m.relates_to"]["rel_type"].as_str()
                                                == Some("m.replace")
                                            {
                                                continue;
                                            }

                                            let text = content["body"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string();
                                            if text.is_empty() {
                                                continue;
                                            }

                                            let event_id = event["event_id"]
                                                .as_str()
                                                .unwrap_or("")
                                                .to_string();

                                            let inbound = InboundMessage {
                                                channel_type: ChannelType::Matrix,
                                                account_id: config.account_id.clone(),
                                                sender_id: sender.to_string(),
                                                sender_name: None, // Matrix sender is the user_id
                                                text,
                                                is_group: true, // Matrix rooms are always "group-like"
                                                group_id: Some(room_id.clone()),
                                                is_mention: false, // Could parse for mentions
                                                platform_message_id: event_id,
                                                attachments: vec![],
                                                metadata: std::collections::HashMap::new(),
                                                source: MessageSource::Channel,
                                                timestamp: chrono::Utc::now(),
                                            };

                                            if let Err(e) = tx.send(inbound).await {
                                                tracing::error!(
                                                    error = %e,
                                                    "failed to send matrix message to processor"
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            Ok(resp) => {
                                let status = resp.status();
                                tracing::warn!(status = %status, "matrix sync returned error");

                                if reconnect_state.should_mark_failed() {
                                    tracing::error!("matrix sync failed after max attempts");
                                    break;
                                }

                                let delay = reconnect_state.next_delay();
                                tokio::time::sleep(delay).await;
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "matrix sync request failed");

                                if reconnect_state.should_mark_failed() {
                                    tracing::error!("matrix reconnection failed after max attempts");
                                    break;
                                }

                                let delay = reconnect_state.next_delay();
                                tokio::time::sleep(delay).await;
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        tracing::info!("matrix channel shutting down");
                        break;
                    }
                }
            }
        });

        tracing::info!(
            user_id = %log_user_id,
            homeserver = %log_homeserver,
            "matrix channel started (long-polling sync)"
        );

        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> anyhow::Result<Option<String>> {
        self.send_room_message(&msg.recipient_id, &msg.text).await
    }

    async fn edit(&self, msg: EditMessage) -> anyhow::Result<()> {
        self.edit_room_message(&msg.recipient_id, &msg.message_id, &msg.text)
            .await
    }

    fn supports_editing(&self) -> bool {
        true
    }

    async fn health_check(&self) -> anyhow::Result<super::ChannelHealth> {
        use super::{ChannelHealth, ChannelStatus};
        let reconnect = self.reconnect_state.lock().await;

        match self.validate_token().await {
            Ok(_) => Ok(ChannelHealth {
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
        tracing::info!("matrix channel shut down");
        Ok(())
    }
}
