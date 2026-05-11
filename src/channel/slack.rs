//! Slack channel implementation using Socket Mode + Web API.
//!
//! Slack requires two tokens:
//! - `botToken` (xoxb-...) for sending messages via Web API
//! - `appToken` (xapp-...) for receiving events via Socket Mode
//!
//! Socket Mode flow:
//! 1. POST apps.connections.open with app token → get wss:// URL
//! 2. Connect WebSocket, receive hello
//! 3. Receive event envelopes, ACK them immediately
//! 4. Extract message events, normalize to InboundMessage

use super::{Channel, ChannelType, EditMessage, InboundMessage, MessageSource, OutboundMessage};
use crate::config::SlackConfig;
use crate::reconnect::{ReconnectPolicy, ReconnectState};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use reqwest::Client;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message as WsMessage;

pub struct SlackChannel {
    config: SlackConfig,
    http: Client,
    /// Bot user ID (resolved on start via auth.test)
    bot_user_id: Mutex<Option<String>>,
    shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Reconnection state for automatic recovery on connection drop
    reconnect_state: Mutex<ReconnectState>,
}

impl SlackChannel {
    pub fn new(config: SlackConfig) -> Self {
        Self {
            config,
            http: Client::new(),
            bot_user_id: Mutex::new(None),
            shutdown_tx: Mutex::new(None),
            reconnect_state: Mutex::new(ReconnectState::new(ReconnectPolicy::default())),
        }
    }

    /// Create a new `SlackChannel` with a custom reconnect policy.
    #[allow(dead_code)] // Available for programmatic channel construction with custom policies
    pub fn with_reconnect_policy(config: SlackConfig, policy: ReconnectPolicy) -> Self {
        Self {
            config,
            http: Client::new(),
            bot_user_id: Mutex::new(None),
            shutdown_tx: Mutex::new(None),
            reconnect_state: Mutex::new(ReconnectState::new(policy)),
        }
    }

    /// Resolve the bot's own user ID via auth.test.
    async fn resolve_bot_id(&self) -> anyhow::Result<String> {
        let resp = self
            .http
            .post("https://slack.com/api/auth.test")
            .bearer_auth(&self.config.bot_token)
            .send()
            .await?;
        let body: serde_json::Value = resp.json().await?;
        if body["ok"].as_bool() != Some(true) {
            anyhow::bail!(
                "slack auth.test failed: {}",
                body["error"].as_str().unwrap_or("unknown")
            );
        }
        let user_id = body["user_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no user_id in auth.test response"))?
            .to_string();
        Ok(user_id)
    }

    /// Post a message via Slack Web API.
    async fn post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut body = serde_json::json!({
            "channel": channel,
            "text": text,
        });
        if let Some(ts) = thread_ts {
            body["thread_ts"] = serde_json::Value::String(ts.to_string());
        }
        let resp = self
            .http
            .post("https://slack.com/api/chat.postMessage")
            .bearer_auth(&self.config.bot_token)
            .json(&body)
            .send()
            .await?;
        let result: serde_json::Value = resp.json().await?;
        if result["ok"].as_bool() != Some(true) {
            let err = result["error"].as_str().unwrap_or("unknown error");
            anyhow::bail!("slack chat.postMessage error: {err}");
        }
        Ok(())
    }

    /// Update an existing message via Slack Web API (chat.update).
    async fn update_message(&self, channel: &str, ts: &str, text: &str) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "channel": channel,
            "ts": ts,
            "text": text,
        });
        let resp = self
            .http
            .post("https://slack.com/api/chat.update")
            .bearer_auth(&self.config.bot_token)
            .json(&body)
            .send()
            .await?;
        let result: serde_json::Value = resp.json().await?;
        if result["ok"].as_bool() != Some(true) {
            let err = result["error"].as_str().unwrap_or("unknown error");
            anyhow::bail!("slack chat.update error: {err}");
        }
        Ok(())
    }
}

#[async_trait]
impl Channel for SlackChannel {
    fn channel_type(&self) -> ChannelType {
        ChannelType::Slack
    }

    async fn start(&self, tx: mpsc::Sender<InboundMessage>) -> anyhow::Result<()> {
        if self.config.app_token.is_empty() {
            anyhow::bail!("slack: appToken required for Socket Mode");
        }

        // Resolve bot user ID so we can ignore our own messages
        let bot_id = self.resolve_bot_id().await?;
        tracing::info!(bot_user_id = %bot_id, "slack bot identified");
        *self.bot_user_id.lock().await = Some(bot_id.clone());

        // Reset reconnect state on successful start
        self.reconnect_state.lock().await.reset();

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        let config = self.config.clone();
        let http = self.http.clone();

        // Build the reconnect policy from current state for the spawned task
        let reconnect_policy = self.reconnect_state.lock().await.policy().clone();

        tokio::spawn(async move {
            let mut reconnect_state = ReconnectState::new(reconnect_policy);

            'reconnect: loop {
                // Open Socket Mode connection (get a fresh WS URL each time)
                let ws_url = match open_socket_mode_static(&http, &config.app_token).await {
                    Ok(url) => {
                        tracing::info!("slack socket mode URL obtained");
                        url
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "failed to open slack socket mode");

                        if reconnect_state.should_mark_failed() {
                            tracing::error!(
                                attempts = reconnect_state.attempts,
                                "slack reconnection failed after max attempts"
                            );
                            break;
                        }

                        let delay = reconnect_state.next_delay();
                        tracing::info!(
                            attempt = reconnect_state.attempts,
                            delay_secs = delay.as_secs(),
                            status = ?reconnect_state.channel_status(),
                            "slack reconnecting after backoff"
                        );

                        tokio::select! {
                            _ = tokio::time::sleep(delay) => continue 'reconnect,
                            _ = &mut shutdown_rx => {
                                tracing::info!("slack channel shutting down during reconnect");
                                break;
                            }
                        }
                    }
                };

                // Connect WebSocket
                let connect_result = tokio_tungstenite::connect_async(&ws_url).await;
                let (ws_stream, _) = match connect_result {
                    Ok(pair) => {
                        // Successful connection — reset reconnect state
                        reconnect_state.reset();
                        tracing::info!("slack socket mode connected");
                        pair
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "failed to connect slack websocket");

                        if reconnect_state.should_mark_failed() {
                            tracing::error!(
                                attempts = reconnect_state.attempts,
                                "slack reconnection failed after max attempts"
                            );
                            break;
                        }

                        let delay = reconnect_state.next_delay();
                        tracing::info!(
                            attempt = reconnect_state.attempts,
                            delay_secs = delay.as_secs(),
                            status = ?reconnect_state.channel_status(),
                            "slack reconnecting after backoff"
                        );

                        tokio::select! {
                            _ = tokio::time::sleep(delay) => continue 'reconnect,
                            _ = &mut shutdown_rx => {
                                tracing::info!("slack channel shutting down during reconnect");
                                break;
                            }
                        }
                    }
                };

                let (mut ws_sink, mut ws_stream_rx) = ws_stream.split();

                loop {
                    tokio::select! {
                        Some(msg_result) = ws_stream_rx.next() => {
                            match msg_result {
                                Ok(WsMessage::Text(text)) => {
                                    handle_slack_envelope(
                                        &text,
                                        &bot_id,
                                        &config,
                                        &tx,
                                        &mut ws_sink,
                                    ).await;
                                }
                                Ok(WsMessage::Ping(data)) => {
                                    let _ = ws_sink.send(WsMessage::Pong(data)).await;
                                }
                                Ok(WsMessage::Close(_)) => {
                                    tracing::warn!("slack websocket closed by server, will reconnect");
                                    continue 'reconnect;
                                }
                                Err(e) => {
                                    tracing::error!(error = %e, "slack websocket error, will reconnect");
                                    continue 'reconnect;
                                }
                                _ => {}
                            }
                        }
                        _ = &mut shutdown_rx => {
                            tracing::info!("slack channel shutting down");
                            let _ = ws_sink.send(WsMessage::Close(None)).await;
                            break 'reconnect;
                        }
                    }
                }
            }
        });

        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> anyhow::Result<Option<String>> {
        self.post_message(&msg.recipient_id, &msg.text, msg.reply_to.as_deref())
            .await?;
        // Slack post_message doesn't return the ts here; would need refactoring
        // to capture it. For now, return None (batch delivery works fine).
        Ok(None)
    }

    async fn edit(&self, msg: EditMessage) -> anyhow::Result<()> {
        self.update_message(&msg.recipient_id, &msg.message_id, &msg.text)
            .await
    }

    fn supports_editing(&self) -> bool {
        true
    }

    async fn health_check(&self) -> anyhow::Result<super::ChannelHealth> {
        use super::{ChannelHealth, ChannelStatus};
        let reconnect = self.reconnect_state.lock().await;
        let resp = self
            .http
            .post("https://slack.com/api/auth.test")
            .bearer_auth(&self.config.bot_token)
            .send()
            .await?;
        let body: serde_json::Value = resp.json().await?;
        if body["ok"].as_bool() == Some(true) {
            Ok(ChannelHealth {
                status: ChannelStatus::Connected,
                last_connected: Some(chrono::Utc::now()),
                reconnect_attempts: reconnect.attempts,
                error: None,
            })
        } else {
            Ok(ChannelHealth {
                status: reconnect.channel_status(),
                last_connected: None,
                reconnect_attempts: reconnect.attempts,
                error: Some(body["error"].as_str().unwrap_or("unknown").to_string()),
            })
        }
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }
        Ok(())
    }
}

/// Open a Socket Mode connection using the given HTTP client and app token.
/// This is a static version used by the spawned reconnection task.
async fn open_socket_mode_static(http: &Client, app_token: &str) -> anyhow::Result<String> {
    let resp = http
        .post("https://slack.com/api/apps.connections.open")
        .bearer_auth(app_token)
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    if body["ok"].as_bool() != Some(true) {
        anyhow::bail!(
            "slack apps.connections.open failed: {}",
            body["error"].as_str().unwrap_or("unknown")
        );
    }
    let url = body["url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("no url in connections.open response"))?
        .to_string();
    Ok(url)
}

/// Handle a single Socket Mode envelope.
///
/// Slack Socket Mode sends JSON envelopes with a type field:
/// - `"hello"` — connection established
/// - `"disconnect"` — server wants us to reconnect
/// - `"events_api"` — contains an event payload (message, etc.)
/// - `"interactive"` — button clicks, modals, etc.
///
/// We must ACK every envelope (except hello) by sending back
/// `{"envelope_id": "..."}` on the WebSocket.
async fn handle_slack_envelope<S>(
    raw: &str,
    bot_id: &str,
    _config: &SlackConfig,
    tx: &mpsc::Sender<InboundMessage>,
    ws_sink: &mut S,
) where
    S: futures::Sink<WsMessage> + Unpin,
    S::Error: std::fmt::Display,
{
    let envelope: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse slack envelope");
            return;
        }
    };

    let envelope_type = envelope["type"].as_str().unwrap_or("");

    match envelope_type {
        "hello" => {
            tracing::info!("slack socket mode: hello received");
            return;
        }
        "disconnect" => {
            tracing::warn!("slack socket mode: disconnect requested, outer loop will reconnect");
            return;
        }
        _ => {}
    }

    // ACK the envelope
    if let Some(envelope_id) = envelope["envelope_id"].as_str() {
        let ack = serde_json::json!({"envelope_id": envelope_id});
        if let Err(e) = ws_sink.send(WsMessage::Text(ack.to_string().into())).await {
            tracing::error!(error = %e, "failed to ACK slack envelope");
        }
    }

    // Process events_api envelopes
    if envelope_type != "events_api" {
        return;
    }

    let event = &envelope["payload"]["event"];
    let event_type = event["type"].as_str().unwrap_or("");

    // We only care about messages (not message_changed, etc.)
    if event_type != "message" {
        return;
    }

    // Skip bot's own messages and subtypes (edits, joins, etc.)
    if event["subtype"].is_string() {
        return;
    }
    let sender_id = event["user"].as_str().unwrap_or("");
    if sender_id.is_empty() || sender_id == bot_id {
        return;
    }

    let text = event["text"].as_str().unwrap_or("").to_string();
    if text.is_empty() {
        return;
    }

    let channel_id = event["channel"].as_str().unwrap_or("").to_string();
    let ts = event["ts"].as_str().unwrap_or("").to_string();
    let channel_type_str = event["channel_type"].as_str().unwrap_or("channel");
    let is_dm = channel_type_str == "im";
    let is_mention = text.contains(&format!("<@{bot_id}>"));

    // In channels, only respond if mentioned (unless configured otherwise)
    if !is_dm && !is_mention {
        return;
    }
    // DM/access policy is enforced centrally by AccessControlBridge in the
    // gateway pipeline — channels just forward all messages.

    // Strip bot mention from text for cleaner input
    let clean_text = text.replace(&format!("<@{bot_id}>"), "").trim().to_string();

    let inbound = InboundMessage {
        channel_type: ChannelType::Slack,
        account_id: String::new(),
        sender_id: sender_id.to_string(),
        sender_name: None, // Slack doesn't include display name in events
        text: if clean_text.is_empty() {
            text
        } else {
            clean_text
        },
        is_group: !is_dm,
        group_id: if !is_dm {
            Some(channel_id.clone())
        } else {
            None
        },
        is_mention,
        platform_message_id: ts,
        attachments: vec![],
        metadata: std::collections::HashMap::new(),
        source: MessageSource::Channel,
        timestamp: chrono::Utc::now(),
    };

    if let Err(e) = tx.send(inbound).await {
        tracing::error!(error = %e, "failed to send slack message to processor");
    }
}
