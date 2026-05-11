//! Discord channel implementation using Gateway WebSocket + REST API.
//!
//! Discord integration uses:
//! - Gateway WebSocket (wss://gateway.discord.gg) for receiving messages
//! - REST API (https://discord.com/api/v10) for sending/editing messages
//!
//! The bot connects to the Discord Gateway, identifies with the bot token,
//! maintains a heartbeat, and receives MESSAGE_CREATE events which are
//! normalized into `InboundMessage` structs.

use super::{Channel, ChannelType, EditMessage, InboundMessage, MessageSource, OutboundMessage};
use crate::config::DiscordConfig;
use crate::reconnect::{ReconnectPolicy, ReconnectState};
use async_trait::async_trait;
use futures::{SinkExt, StreamExt};
use reqwest::Client;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Discord REST API base URL.
const DISCORD_API_BASE: &str = "https://discord.com/api/v10";

/// Discord Gateway URL.
const DISCORD_GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";

/// Discord Gateway opcodes.
mod opcode {
    pub const DISPATCH: u64 = 0;
    pub const HEARTBEAT: u64 = 1;
    pub const IDENTIFY: u64 = 2;
    pub const RESUME: u64 = 6;
    pub const RECONNECT: u64 = 7;
    pub const INVALID_SESSION: u64 = 9;
    pub const HELLO: u64 = 10;
    pub const HEARTBEAT_ACK: u64 = 11;
}

/// Gateway intents — we need GUILD_MESSAGES + MESSAGE_CONTENT + DIRECT_MESSAGES.
const GATEWAY_INTENTS: u64 = (1 << 9) | (1 << 15) | (1 << 12);

pub struct DiscordChannel {
    config: DiscordConfig,
    http: Client,
    /// Bot user ID (resolved on start)
    bot_user_id: Mutex<Option<String>>,
    /// Shutdown signal
    shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Reconnection state
    reconnect_state: Mutex<ReconnectState>,
}

impl DiscordChannel {
    pub fn new(config: DiscordConfig) -> Self {
        Self {
            config,
            http: Client::new(),
            bot_user_id: Mutex::new(None),
            shutdown_tx: Mutex::new(None),
            reconnect_state: Mutex::new(ReconnectState::new(ReconnectPolicy::default())),
        }
    }

    /// Validate the bot token by calling GET /users/@me.
    async fn validate_token(&self) -> anyhow::Result<String> {
        let resp = self
            .http
            .get(format!("{}/users/@me", DISCORD_API_BASE))
            .header("Authorization", format!("Bot {}", self.config.bot_token))
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "discord token validation failed (HTTP {}): {}",
                status,
                body
            );
        }

        let body: serde_json::Value = resp.json().await?;
        let bot_id = body["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("no id in /users/@me response"))?
            .to_string();

        let username = body["username"].as_str().unwrap_or("unknown");
        tracing::info!(bot_id = %bot_id, username = %username, "discord bot authenticated");

        Ok(bot_id)
    }

    /// Send a message via Discord REST API.
    async fn send_message(
        &self,
        channel_id: &str,
        text: &str,
        reply_to: Option<&str>,
    ) -> anyhow::Result<Option<String>> {
        let url = format!("{}/channels/{}/messages", DISCORD_API_BASE, channel_id);

        // Truncate to max message length
        let max_len = ChannelType::Discord.max_message_length();
        let text = if text.len() > max_len {
            let mut truncated = text[..max_len - 3].to_string();
            truncated.push_str("...");
            truncated
        } else {
            text.to_string()
        };

        let mut body = serde_json::json!({
            "content": text,
        });

        if let Some(msg_id) = reply_to {
            body["message_reference"] = serde_json::json!({
                "message_id": msg_id,
            });
        }

        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bot {}", self.config.bot_token))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            anyhow::bail!("discord send failed (HTTP {}): {}", status, err_body);
        }

        let result: serde_json::Value = resp.json().await?;
        let message_id = result["id"].as_str().map(|s| s.to_string());
        Ok(message_id)
    }

    /// Edit a message via Discord REST API.
    async fn edit_message(
        &self,
        channel_id: &str,
        message_id: &str,
        text: &str,
    ) -> anyhow::Result<()> {
        let url = format!(
            "{}/channels/{}/messages/{}",
            DISCORD_API_BASE, channel_id, message_id
        );

        // Truncate to max message length
        let max_len = ChannelType::Discord.max_message_length();
        let text = if text.len() > max_len {
            let mut truncated = text[..max_len - 3].to_string();
            truncated.push_str("...");
            truncated
        } else {
            text.to_string()
        };

        let body = serde_json::json!({
            "content": text,
        });

        let resp = self
            .http
            .patch(&url)
            .header("Authorization", format!("Bot {}", self.config.bot_token))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            anyhow::bail!("discord edit failed (HTTP {}): {}", status, err_body);
        }

        Ok(())
    }
}

#[async_trait]
impl Channel for DiscordChannel {
    fn channel_type(&self) -> ChannelType {
        ChannelType::Discord
    }

    fn account_id(&self) -> &str {
        &self.config.account_id
    }

    async fn start(&self, tx: mpsc::Sender<InboundMessage>) -> anyhow::Result<()> {
        // Validate the bot token
        let bot_id = self.validate_token().await?;
        *self.bot_user_id.lock().await = Some(bot_id.clone());

        // Reset reconnect state on successful start
        self.reconnect_state.lock().await.reset();

        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        let config = self.config.clone();
        let reconnect_policy = self.reconnect_state.lock().await.policy().clone();

        tokio::spawn(async move {
            let mut reconnect_state = ReconnectState::new(reconnect_policy);
            let mut session_id: Option<String> = None;
            let mut sequence: Option<u64> = None;
            let mut resume_gateway_url: Option<String> = None;

            'reconnect: loop {
                let gateway_url = resume_gateway_url.as_deref().unwrap_or(DISCORD_GATEWAY_URL);

                let connect_result = tokio_tungstenite::connect_async(gateway_url).await;
                let (ws_stream, _) = match connect_result {
                    Ok(pair) => {
                        reconnect_state.reset();
                        tracing::info!("discord gateway connected");
                        pair
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "failed to connect discord gateway");

                        if reconnect_state.should_mark_failed() {
                            tracing::error!(
                                attempts = reconnect_state.attempts,
                                "discord reconnection failed after max attempts"
                            );
                            break;
                        }

                        let delay = reconnect_state.next_delay();
                        tracing::info!(
                            attempt = reconnect_state.attempts,
                            delay_secs = delay.as_secs(),
                            "discord reconnecting after backoff"
                        );

                        tokio::select! {
                            _ = tokio::time::sleep(delay) => continue 'reconnect,
                            _ = &mut shutdown_rx => {
                                tracing::info!("discord channel shutting down during reconnect");
                                break;
                            }
                        }
                    }
                };

                let (mut ws_sink, mut ws_stream_rx) = ws_stream.split();
                let mut heartbeat_interval: Option<tokio::time::Interval> = None;

                loop {
                    tokio::select! {
                        Some(msg_result) = ws_stream_rx.next() => {
                            match msg_result {
                                Ok(WsMessage::Text(text)) => {
                                    let payload: serde_json::Value = match serde_json::from_str(&text) {
                                        Ok(v) => v,
                                        Err(e) => {
                                            tracing::warn!(error = %e, "failed to parse discord gateway message");
                                            continue;
                                        }
                                    };

                                    let op = payload["op"].as_u64().unwrap_or(0);

                                    // Update sequence number
                                    if let Some(s) = payload["s"].as_u64() {
                                        sequence = Some(s);
                                    }

                                    match op {
                                        opcode::HELLO => {
                                            // Start heartbeat
                                            let interval_ms = payload["d"]["heartbeat_interval"]
                                                .as_u64()
                                                .unwrap_or(41250);
                                            let mut interval = tokio::time::interval(
                                                std::time::Duration::from_millis(interval_ms),
                                            );
                                            interval.tick().await; // consume first immediate tick
                                            heartbeat_interval = Some(interval);

                                            // Send IDENTIFY or RESUME
                                            if let Some(ref sid) = session_id {
                                                // Resume
                                                let resume = serde_json::json!({
                                                    "op": opcode::RESUME,
                                                    "d": {
                                                        "token": config.bot_token,
                                                        "session_id": sid,
                                                        "seq": sequence,
                                                    }
                                                });
                                                let _ = ws_sink.send(WsMessage::Text(resume.to_string().into())).await;
                                                tracing::info!("discord gateway: sent RESUME");
                                            } else {
                                                // Identify
                                                let identify = serde_json::json!({
                                                    "op": opcode::IDENTIFY,
                                                    "d": {
                                                        "token": config.bot_token,
                                                        "intents": GATEWAY_INTENTS,
                                                        "properties": {
                                                            "os": "linux",
                                                            "browser": "adk-gateway",
                                                            "device": "adk-gateway",
                                                        }
                                                    }
                                                });
                                                let _ = ws_sink.send(WsMessage::Text(identify.to_string().into())).await;
                                                tracing::info!("discord gateway: sent IDENTIFY");
                                            }
                                        }
                                        opcode::HEARTBEAT_ACK => {
                                            // Heartbeat acknowledged, all good
                                        }
                                        opcode::HEARTBEAT => {
                                            // Server requests immediate heartbeat
                                            let hb = serde_json::json!({
                                                "op": opcode::HEARTBEAT,
                                                "d": sequence,
                                            });
                                            let _ = ws_sink.send(WsMessage::Text(hb.to_string().into())).await;
                                        }
                                        opcode::RECONNECT => {
                                            tracing::warn!("discord gateway: reconnect requested");
                                            continue 'reconnect;
                                        }
                                        opcode::INVALID_SESSION => {
                                            let resumable = payload["d"].as_bool().unwrap_or(false);
                                            if !resumable {
                                                session_id = None;
                                                sequence = None;
                                            }
                                            tracing::warn!(resumable, "discord gateway: invalid session");
                                            // Wait a bit before reconnecting (Discord recommends 1-5s)
                                            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                                            continue 'reconnect;
                                        }
                                        opcode::DISPATCH => {
                                            let event_name = payload["t"].as_str().unwrap_or("");

                                            match event_name {
                                                "READY" => {
                                                    session_id = payload["d"]["session_id"]
                                                        .as_str()
                                                        .map(|s| s.to_string());
                                                    resume_gateway_url = payload["d"]["resume_gateway_url"]
                                                        .as_str()
                                                        .map(|s| s.to_string());
                                                    tracing::info!(
                                                        session_id = ?session_id,
                                                        "discord gateway: READY"
                                                    );
                                                }
                                                "MESSAGE_CREATE" => {
                                                    let d = &payload["d"];
                                                    let author = &d["author"];
                                                    let author_id = author["id"].as_str().unwrap_or("");

                                                    // Skip bot's own messages
                                                    if author_id == bot_id {
                                                        continue;
                                                    }
                                                    // Skip other bots
                                                    if author["bot"].as_bool() == Some(true) {
                                                        continue;
                                                    }

                                                    let content = d["content"].as_str().unwrap_or("").to_string();
                                                    if content.is_empty() {
                                                        continue;
                                                    }

                                                    let channel_id = d["channel_id"].as_str().unwrap_or("").to_string();
                                                    let message_id = d["id"].as_str().unwrap_or("").to_string();
                                                    let guild_id = d["guild_id"].as_str().map(|s| s.to_string());
                                                    let is_dm = guild_id.is_none();

                                                    // Check if bot is mentioned
                                                    let is_mention = d["mentions"]
                                                        .as_array()
                                                        .map(|mentions| {
                                                            mentions.iter().any(|m| {
                                                                m["id"].as_str() == Some(&bot_id)
                                                            })
                                                        })
                                                        .unwrap_or(false);

                                                    // In guilds, only respond if mentioned (unless DM)
                                                    if !is_dm && !is_mention {
                                                        continue;
                                                    }

                                                    // Strip bot mention from text
                                                    let clean_text = content
                                                        .replace(&format!("<@{}>", bot_id), "")
                                                        .replace(&format!("<@!{}>", bot_id), "")
                                                        .trim()
                                                        .to_string();

                                                    let sender_name = author["username"]
                                                        .as_str()
                                                        .map(|s| s.to_string());

                                                    let inbound = InboundMessage {
                                                        channel_type: ChannelType::Discord,
                                                        account_id: config.account_id.clone(),
                                                        sender_id: author_id.to_string(),
                                                        sender_name,
                                                        text: if clean_text.is_empty() { content } else { clean_text },
                                                        is_group: !is_dm,
                                                        group_id: if !is_dm { Some(channel_id.clone()) } else { None },
                                                        is_mention,
                                                        platform_message_id: message_id,
                                                        attachments: vec![],
                                                        metadata: {
                                                            let mut m = std::collections::HashMap::new();
                                                            m.insert("discord_channel_id".to_string(), serde_json::Value::String(channel_id));
                                                            if let Some(ref gid) = guild_id {
                                                                m.insert("discord_guild_id".to_string(), serde_json::Value::String(gid.clone()));
                                                            }
                                                            m
                                                        },
                                                        source: MessageSource::Channel,
                                                        timestamp: chrono::Utc::now(),
                                                    };

                                                    if let Err(e) = tx.send(inbound).await {
                                                        tracing::error!(error = %e, "failed to send discord message to processor");
                                                    }
                                                }
                                                _ => {
                                                    // Ignore other dispatch events
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                Ok(WsMessage::Close(_)) => {
                                    tracing::warn!("discord gateway closed by server, will reconnect");
                                    continue 'reconnect;
                                }
                                Err(e) => {
                                    tracing::error!(error = %e, "discord gateway error, will reconnect");
                                    continue 'reconnect;
                                }
                                _ => {}
                            }
                        }
                        _ = async {
                            if let Some(ref mut interval) = heartbeat_interval {
                                interval.tick().await;
                            } else {
                                // No heartbeat interval yet, wait forever
                                std::future::pending::<()>().await;
                            }
                        } => {
                            // Send heartbeat
                            let hb = serde_json::json!({
                                "op": opcode::HEARTBEAT,
                                "d": sequence,
                            });
                            if let Err(e) = ws_sink.send(WsMessage::Text(hb.to_string().into())).await {
                                tracing::error!(error = %e, "failed to send discord heartbeat");
                                continue 'reconnect;
                            }
                        }
                        _ = &mut shutdown_rx => {
                            tracing::info!("discord channel shutting down");
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
        self.send_message(&msg.recipient_id, &msg.text, msg.reply_to.as_deref())
            .await
    }

    async fn edit(&self, msg: EditMessage) -> anyhow::Result<()> {
        self.edit_message(&msg.recipient_id, &msg.message_id, &msg.text)
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
            .get(format!("{}/users/@me", DISCORD_API_BASE))
            .header("Authorization", format!("Bot {}", self.config.bot_token))
            .send()
            .await?;

        if resp.status().is_success() {
            Ok(ChannelHealth {
                status: ChannelStatus::Connected,
                last_connected: Some(chrono::Utc::now()),
                reconnect_attempts: reconnect.attempts,
                error: None,
            })
        } else {
            let err = format!("HTTP {}", resp.status());
            Ok(ChannelHealth {
                status: reconnect.channel_status(),
                last_connected: None,
                reconnect_attempts: reconnect.attempts,
                error: Some(err),
            })
        }
    }

    async fn shutdown(&self) -> anyhow::Result<()> {
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }
        tracing::info!("discord channel shut down");
        Ok(())
    }
}
