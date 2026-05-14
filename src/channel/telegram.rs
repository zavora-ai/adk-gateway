//! Telegram channel implementation using the Bot API via teloxide.

use super::{Channel, ChannelType, EditMessage, InboundMessage, OutboundMessage};
use crate::config::TelegramConfig;
use crate::reconnect::{ReconnectPolicy, ReconnectState};
use async_trait::async_trait;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};

/// Result of probing the Telegram Bot API for connectivity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResult {
    /// Successfully connected; includes the bot's username.
    Connected { bot_username: String },
    /// The bot token is invalid or revoked (HTTP 401).
    InvalidToken,
    /// The API was unreachable within the timeout period.
    Unreachable { timeout_ms: u64 },
    /// An unexpected error occurred.
    Error { message: String },
}

pub struct TelegramChannel {
    config: TelegramConfig,
    /// Bot instance, lazily initialized on start()
    bot: Mutex<Option<teloxide::Bot>>,
    /// Shutdown signal
    shutdown_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    /// Reconnection state for automatic recovery on connection drop
    reconnect_state: Mutex<ReconnectState>,
}

impl TelegramChannel {
    pub fn new(config: TelegramConfig) -> Self {
        Self {
            config,
            bot: Mutex::new(None),
            shutdown_tx: Mutex::new(None),
            reconnect_state: Mutex::new(ReconnectState::new(ReconnectPolicy::default())),
        }
    }

    /// Create a new `TelegramChannel` with a custom reconnect policy.
    #[allow(dead_code)] // Available for programmatic channel construction with custom policies
    pub fn with_reconnect_policy(config: TelegramConfig, policy: ReconnectPolicy) -> Self {
        Self {
            config,
            bot: Mutex::new(None),
            shutdown_tx: Mutex::new(None),
            reconnect_state: Mutex::new(ReconnectState::new(policy)),
        }
    }

    /// Probe the Telegram Bot API to verify connectivity and token validity.
    ///
    /// Calls the `getMe` endpoint and returns a [`ProbeResult`] indicating
    /// whether the bot is reachable, the token is valid, or an error occurred.
    pub async fn probe(&self) -> ProbeResult {
        self.probe_with_base_url("https://api.telegram.org").await
    }

    /// Internal probe implementation that accepts a configurable base URL (for testing).
    pub async fn probe_with_base_url(&self, base_url: &str) -> ProbeResult {
        let url = format!("{}/bot{}/getMe", base_url, self.config.bot_token);
        let client = reqwest::Client::new();
        let timeout = Duration::from_secs(10);
        let timeout_ms = timeout.as_millis() as u64;

        match tokio::time::timeout(timeout, client.get(&url).send()).await {
            Ok(Ok(resp)) => {
                if resp.status().is_success() {
                    match resp.json::<serde_json::Value>().await {
                        Ok(body) => {
                            let username = body["result"]["username"]
                                .as_str()
                                .unwrap_or("unknown")
                                .to_string();
                            ProbeResult::Connected {
                                bot_username: username,
                            }
                        }
                        Err(e) => ProbeResult::Error {
                            message: format!("failed to parse response: {e}"),
                        },
                    }
                } else if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
                    ProbeResult::InvalidToken
                } else {
                    ProbeResult::Error {
                        message: format!("HTTP {}", resp.status()),
                    }
                }
            }
            Ok(Err(_e)) => ProbeResult::Unreachable { timeout_ms },
            Err(_) => ProbeResult::Unreachable { timeout_ms },
        }
    }
}

#[async_trait]
impl Channel for TelegramChannel {
    fn channel_type(&self) -> ChannelType {
        ChannelType::Telegram
    }

    async fn start(&self, tx: mpsc::Sender<InboundMessage>) -> anyhow::Result<()> {
        use teloxide::prelude::*;

        let bot = teloxide::Bot::new(&self.config.bot_token);

        // Validate the token before starting the dispatcher.
        // teloxide panics on invalid tokens inside dispatch(), so we
        // catch it here with a getMe call first.
        let me = bot
            .get_me()
            .await
            .map_err(|e| anyhow::anyhow!("telegram bot token is invalid or revoked: {e}"))?;
        let bot_username = me.username.clone().unwrap_or_default();
        tracing::info!(username = %bot_username, "telegram bot authenticated");

        *self.bot.lock().await = Some(bot.clone());

        // Reset reconnect state on successful start
        self.reconnect_state.lock().await.reset();

        let config = self.config.clone();
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        *self.shutdown_tx.lock().await = Some(shutdown_tx);

        // Build the reconnect policy from current state for the spawned task
        let reconnect_policy = self.reconnect_state.lock().await.policy().clone();

        // Spawn the polling loop with reconnection support
        let tx = tx.clone();
        tokio::spawn(async move {
            let mut reconnect_state = ReconnectState::new(reconnect_policy);

            loop {
                let bot_for_dispatch = bot.clone();
                let tx_for_handler = tx.clone();
                let config_for_handler = config.clone();
                let bot_username_for_handler = bot_username.clone();

                let handler =
                    Update::filter_message().endpoint(move |msg: Message, _bot: teloxide::Bot| {
                        let tx = tx_for_handler.clone();
                        let config = config_for_handler.clone();
                        let bot_username = bot_username_for_handler.clone();
                        async move {
                            // Extract text
                            let text = match msg.text() {
                                Some(t) => t.to_string(),
                                None => return respond(()),
                            };

                            let sender_id = msg
                                .from
                                .as_ref()
                                .map(|u| u.id.0.to_string())
                                .unwrap_or_default();
                            let sender_name = msg
                                .from
                                .as_ref()
                                .map(|u| {
                                    // Prefer first_name + last_name, fall back to @username
                                    let full_name = match (&u.first_name, &u.last_name) {
                                        (first, Some(last)) if !first.is_empty() => {
                                            format!("{first} {last}")
                                        }
                                        (first, _) if !first.is_empty() => first.clone(),
                                        _ => String::new(),
                                    };
                                    if full_name.is_empty() {
                                        u.username.clone()
                                    } else {
                                        Some(full_name)
                                    }
                                })
                                .unwrap_or(None);
                            let is_group = msg.chat.is_group() || msg.chat.is_supergroup();

                            // Check bot mention in groups using actual bot username
                            let is_mention = if is_group {
                                text.contains(&format!("@{bot_username}"))
                            } else {
                                false
                            };

                            // For groups with requireMention, skip if not mentioned
                            if is_group {
                                let require_mention = config
                                    .groups
                                    .rules
                                    .get("*")
                                    .and_then(|r| r.require_mention)
                                    .unwrap_or(true);
                                if require_mention && !is_mention {
                                    return respond(());
                                }
                            }

                            let inbound = InboundMessage {
                                channel_type: ChannelType::Telegram,
                                account_id: config.account_id.clone(),
                                sender_id,
                                sender_name,
                                text,
                                is_group,
                                group_id: if is_group {
                                    Some(msg.chat.id.0.to_string())
                                } else {
                                    None
                                },
                                is_mention,
                                platform_message_id: msg.id.0.to_string(),
                                attachments: vec![],
                                metadata: std::collections::HashMap::new(),
                                source: super::MessageSource::Channel,
                                timestamp: chrono::Utc::now(),
                            };

                            let _ = tx.send(inbound).await;
                            respond(())
                        }
                    });

                let mut dispatcher = Dispatcher::builder(bot_for_dispatch, handler)
                    .enable_ctrlc_handler()
                    .build();

                tracing::info!("telegram polling started");
                dispatcher.dispatch().await;

                // dispatch() returned — connection dropped
                tracing::warn!("telegram polling stopped, attempting reconnection");

                // Check if we've been asked to shut down
                if shutdown_rx.try_recv().is_ok() {
                    tracing::info!("telegram channel shutting down, not reconnecting");
                    break;
                }

                if reconnect_state.should_mark_failed() {
                    tracing::error!(
                        attempts = reconnect_state.attempts,
                        "telegram reconnection failed after max attempts, marking as failed"
                    );
                    break;
                }

                let delay = reconnect_state.next_delay();
                tracing::info!(
                    attempt = reconnect_state.attempts,
                    delay_secs = delay.as_secs(),
                    status = ?reconnect_state.channel_status(),
                    "telegram reconnecting after backoff"
                );

                tokio::select! {
                    _ = tokio::time::sleep(delay) => {
                        // Validate the bot token is still valid before reconnecting
                        match bot.get_me().await {
                            Ok(_) => {
                                tracing::info!("telegram bot re-authenticated, resuming polling");
                                reconnect_state.reset();
                            }
                            Err(e) => {
                                tracing::warn!(error = %e, "telegram re-authentication failed");
                                // Continue the loop to try again with next backoff
                                continue;
                            }
                        }
                    }
                    _ = &mut shutdown_rx => {
                        tracing::info!("telegram channel shutting down during reconnect backoff");
                        break;
                    }
                }
            }
        });

        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> anyhow::Result<Option<String>> {
        use teloxide::prelude::*;
        use teloxide::types::ChatId;

        let bot = self.bot.lock().await;
        let bot = bot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("telegram bot not initialized"))?;

        let chat_id: i64 = msg.recipient_id.parse()?;

        let mut request = bot.send_message(ChatId(chat_id), &msg.text);

        // Reply to the original message if specified
        if let Some(ref reply_to) = msg.reply_to {
            if let Ok(msg_id) = reply_to.parse::<i32>() {
                request = request.reply_parameters(teloxide::types::ReplyParameters::new(
                    teloxide::types::MessageId(msg_id),
                ));
            }
        }

        let sent = request.await?;
        Ok(Some(sent.id.0.to_string()))
    }

    async fn send_typing(&self, chat_id: &str) -> anyhow::Result<()> {
        use teloxide::prelude::*;
        use teloxide::types::{ChatAction, ChatId};

        let bot = self.bot.lock().await;
        if let Some(bot) = bot.as_ref() {
            let id: i64 = chat_id.parse()?;
            let _ = bot.send_chat_action(ChatId(id), ChatAction::Typing).await;
        }
        Ok(())
    }

    async fn edit(&self, msg: EditMessage) -> anyhow::Result<()> {
        use teloxide::prelude::*;
        use teloxide::types::{ChatId, MessageId};

        let bot = self.bot.lock().await;
        let bot = bot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("telegram bot not initialized"))?;

        let chat_id: i64 = msg.recipient_id.parse()?;
        let message_id: i32 = msg.message_id.parse()?;

        bot.edit_message_text(ChatId(chat_id), MessageId(message_id), &msg.text)
            .await?;
        Ok(())
    }

    fn supports_editing(&self) -> bool {
        true
    }

    async fn health_check(&self) -> anyhow::Result<super::ChannelHealth> {
        use super::{ChannelHealth, ChannelStatus};
        let reconnect = self.reconnect_state.lock().await;
        let bot = self.bot.lock().await;
        if let Some(ref bot) = *bot {
            use teloxide::prelude::*;
            match bot.get_me().await {
                Ok(me) => {
                    tracing::debug!(username = ?me.username, "telegram bot healthy");
                    Ok(ChannelHealth {
                        status: ChannelStatus::Connected,
                        last_connected: Some(chrono::Utc::now()),
                        reconnect_attempts: reconnect.attempts,
                        error: None,
                    })
                }
                Err(e) => Ok(ChannelHealth {
                    status: reconnect.channel_status(),
                    last_connected: None,
                    reconnect_attempts: reconnect.attempts,
                    error: Some(format!("{e}")),
                }),
            }
        } else {
            Ok(ChannelHealth {
                status: ChannelStatus::Disconnected,
                last_connected: None,
                reconnect_attempts: reconnect.attempts,
                error: Some("bot not initialized".to_string()),
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
