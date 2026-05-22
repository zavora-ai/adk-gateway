//! Telegram channel implementation using the Bot API via teloxide.
//!
//! Supports inline keyboard buttons for tool approval flow (Task 14):
//! - Sending messages with inline keyboard markup (✅ Approve / ❌ Reject)
//! - Handling callback queries from inline button presses
//! - Updating messages to reflect approval/rejection/timeout status
//!
//! Supports coding agent delegation commands (Task 13):
//! - Parsing "delegate to {agent}: {task}" messages
//! - Routing tasks to the appropriate coding agent via TaskDelegator
//! - Handling /stop command to cancel in-progress coding agent tasks

use super::{Channel, ChannelType, EditMessage, InboundMessage, OutboundMessage};
use crate::coding_agent::delegator::{DelegationCommand, TaskDelegator};
use crate::coding_agent::error::CodingAgentError;
use crate::coding_agent::models::{ReplyTarget, TaskRequest, TaskTrigger};
use crate::config::TelegramConfig;
use crate::reconnect::{ReconnectPolicy, ReconnectState};
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;
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

/// Status for updating an approval message after user action or timeout.
#[derive(Debug, Clone)]
pub enum ApprovalMessageStatus {
    /// User approved the tool call.
    Approved { timestamp: String },
    /// User rejected the tool call.
    Rejected { timestamp: String },
    /// Approval timed out (auto-rejected).
    TimedOut,
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

    // ── Inline Keyboard Support (Tool Approval - Task 14) ──────────

    /// Send a message with an inline keyboard for tool approval.
    ///
    /// The message displays the tool name, arguments summary, and a
    /// "⏳ Waiting for approval..." status. Two inline buttons are attached:
    /// ✅ Approve and ❌ Reject.
    ///
    /// Returns the platform message ID of the sent message.
    pub async fn send_approval_request(
        &self,
        chat_id: &str,
        approval_id: &str,
        tool_name: &str,
        tool_args: &serde_json::Value,
    ) -> anyhow::Result<String> {
        let message_text = crate::tool_approval::build_approval_message(tool_name, tool_args);
        let keyboard = crate::tool_approval::build_approval_keyboard(approval_id);

        let bot = self.bot.lock().await;
        let bot = bot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("telegram bot not initialized"))?;

        let id: i64 = chat_id.parse()?;
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.config.bot_token
        );

        let payload = serde_json::json!({
            "chat_id": id,
            "text": message_text,
            "parse_mode": "Markdown",
            "reply_markup": keyboard,
        });

        let client = reqwest::Client::new();
        let resp = client.post(&url).json(&payload).send().await?;
        let body: serde_json::Value = resp.json().await?;

        let message_id = body["result"]["message_id"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("failed to get message_id from response"))?;

        tracing::info!(
            chat_id = %chat_id,
            approval_id = %approval_id,
            tool_name = %tool_name,
            message_id = message_id,
            "Sent tool approval request with inline keyboard"
        );

        // Suppress unused variable warning for bot reference
        let _ = bot;

        Ok(message_id.to_string())
    }

    /// Update an approval message after the user responds or timeout occurs.
    ///
    /// Replaces the inline keyboard with a status message:
    /// - "✅ Approved" with timestamp on approve
    /// - "❌ Rejected" with timestamp on reject
    /// - "⏰ Timed out (auto-rejected)" on timeout
    pub async fn update_approval_message(
        &self,
        chat_id: &str,
        message_id: &str,
        tool_name: &str,
        status: ApprovalMessageStatus,
    ) -> anyhow::Result<()> {
        let status_text = match status {
            ApprovalMessageStatus::Approved { timestamp } => {
                format!(
                    "🔒 *Tool Approval*\n\nTool: `{}`\n\n✅ Approved at {}",
                    tool_name, timestamp
                )
            }
            ApprovalMessageStatus::Rejected { timestamp } => {
                format!(
                    "🔒 *Tool Approval*\n\nTool: `{}`\n\n❌ Rejected at {}",
                    tool_name, timestamp
                )
            }
            ApprovalMessageStatus::TimedOut => {
                format!(
                    "🔒 *Tool Approval*\n\nTool: `{}`\n\n⏰ Timed out (auto-rejected)",
                    tool_name
                )
            }
        };

        let id: i64 = chat_id.parse()?;
        let msg_id: i32 = message_id.parse()?;

        let url = format!(
            "https://api.telegram.org/bot{}/editMessageText",
            self.config.bot_token
        );

        let payload = serde_json::json!({
            "chat_id": id,
            "message_id": msg_id,
            "text": status_text,
            "parse_mode": "Markdown",
            // Remove inline keyboard by setting empty markup
            "reply_markup": { "inline_keyboard": [] },
        });

        let client = reqwest::Client::new();
        let _resp = client.post(&url).json(&payload).send().await?;

        tracing::info!(
            chat_id = %chat_id,
            message_id = %message_id,
            tool_name = %tool_name,
            "Updated approval message status"
        );

        Ok(())
    }

    /// Answer a callback query from an inline button press.
    ///
    /// This acknowledges the button press to Telegram so the loading
    /// indicator disappears for the user.
    pub async fn answer_callback_query(
        &self,
        callback_query_id: &str,
        text: Option<&str>,
    ) -> anyhow::Result<()> {
        let url = format!(
            "https://api.telegram.org/bot{}/answerCallbackQuery",
            self.config.bot_token
        );

        let mut payload = serde_json::json!({
            "callback_query_id": callback_query_id,
        });

        if let Some(t) = text {
            payload["text"] = serde_json::Value::String(t.to_string());
        }

        let client = reqwest::Client::new();
        let _resp = client.post(&url).json(&payload).send().await?;

        Ok(())
    }
}

// ── Coding Agent Delegation Handler (Task 13) ──────────────────────

/// Result of attempting to handle a message as a delegation command.
#[derive(Debug)]
pub enum DelegationResult {
    /// The message was handled as a delegation command.
    Handled,
    /// The message was not a delegation command; continue normal processing.
    NotADelegationCommand,
}

/// Handles coding agent delegation commands from Telegram messages.
///
/// Integrates with the `TaskDelegator` to parse delegation commands,
/// route tasks to coding agents, and send appropriate replies.
/// Tracks the last task ID per chat to support the `/stop` command.
pub struct TelegramDelegationHandler {
    /// The task delegator for routing tasks to coding agents.
    delegator: Arc<TaskDelegator>,
    /// Tracks the last task ID per chat_id for /stop support.
    last_task_per_chat: Arc<DashMap<String, String>>,
}

impl TelegramDelegationHandler {
    /// Create a new delegation handler with the given task delegator.
    pub fn new(delegator: Arc<TaskDelegator>) -> Self {
        Self {
            delegator,
            last_task_per_chat: Arc::new(DashMap::new()),
        }
    }

    /// Attempt to handle an inbound message as a delegation command.
    ///
    /// Returns `DelegationResult::Handled` if the message was a delegation command
    /// (regardless of success/failure), or `DelegationResult::NotADelegationCommand`
    /// if the message doesn't match the delegation pattern.
    ///
    /// On success, sends an acknowledgment message to the chat.
    /// On failure, sends an appropriate error message.
    pub async fn handle_delegation(
        &self,
        text: &str,
        chat_id: &str,
        sender_id: &str,
        message_id: &str,
        bot_token: &str,
    ) -> DelegationResult {
        // Try to parse as a delegation command
        let command = match TaskDelegator::parse_delegation_command(text) {
            Some(cmd) => cmd,
            None => return DelegationResult::NotADelegationCommand,
        };

        let DelegationCommand {
            agent_name,
            task_description,
        } = command;

        // Build the task request
        let task_request = TaskRequest {
            description: task_description,
            trigger: TaskTrigger::UserCommand {
                user_id: sender_id.to_string(),
                channel: "telegram".to_string(),
            },
            workspace: None,
            file_context: None,
            reply_to: ReplyTarget {
                channel_type: "telegram".to_string(),
                channel_id: chat_id.to_string(),
                message_id: Some(message_id.to_string()),
            },
        };

        // Attempt delegation
        match self.delegator.delegate(&agent_name, task_request).await {
            Ok(task_id) => {
                // Track the task for /stop support
                self.last_task_per_chat
                    .insert(chat_id.to_string(), task_id.clone());

                // Resolve display name for the acknowledgment message
                let display_name = self.resolve_display_name(&agent_name);

                let reply = format!("✅ Task queued for {display_name}");
                let _ = send_telegram_message(bot_token, chat_id, &reply, Some(message_id)).await;

                tracing::info!(
                    chat_id = %chat_id,
                    sender_id = %sender_id,
                    agent = %agent_name,
                    task_id = %task_id,
                    "Delegation command handled successfully"
                );
            }
            Err(CodingAgentError::AgentNotFound(_)) => {
                let available = self.format_available_agents();
                let reply = format!(
                    "❌ Agent \"{}\" not found.\n\nAvailable agents:\n{}",
                    agent_name, available
                );
                let _ = send_telegram_message(bot_token, chat_id, &reply, Some(message_id)).await;

                tracing::warn!(
                    chat_id = %chat_id,
                    agent = %agent_name,
                    "Delegation failed: agent not found"
                );
            }
            Err(CodingAgentError::AgentDisconnected(ref agent_id)) => {
                let reply = format!(
                    "⚠️ Agent \"{}\" is currently unavailable. Please try again later.",
                    agent_id
                );
                let _ = send_telegram_message(bot_token, chat_id, &reply, Some(message_id)).await;

                tracing::warn!(
                    chat_id = %chat_id,
                    agent = %agent_name,
                    "Delegation failed: agent disconnected"
                );
            }
            Err(err) => {
                let reply = format!("❌ Delegation failed: {}", err);
                let _ = send_telegram_message(bot_token, chat_id, &reply, Some(message_id)).await;

                tracing::error!(
                    chat_id = %chat_id,
                    agent = %agent_name,
                    error = %err,
                    "Delegation failed with unexpected error"
                );
            }
        }

        DelegationResult::Handled
    }

    /// Handle the `/stop` command to cancel in-progress coding agent tasks.
    ///
    /// Accepts either `/stop` (cancels the last task for this chat) or
    /// `/stop {task_id}` (cancels a specific task by ID).
    ///
    /// Returns `true` if the command was handled as a coding agent stop,
    /// `false` if there's no task to cancel (caller should fall through to
    /// normal /stop handling).
    pub async fn handle_stop(
        &self,
        text: &str,
        chat_id: &str,
        message_id: &str,
        bot_token: &str,
    ) -> bool {
        let trimmed = text.trim();

        // Extract task ID: either from the command argument or from last tracked task
        let task_id = if trimmed.eq_ignore_ascii_case("/stop") {
            // Use the last task for this chat
            match self.last_task_per_chat.get(chat_id) {
                Some(entry) => entry.value().clone(),
                None => return false, // No tracked task, let normal /stop handle it
            }
        } else if trimmed.to_lowercase().starts_with("/stop ") {
            // Extract task ID from argument
            trimmed[6..].trim().to_string()
        } else {
            return false;
        };

        // Attempt cancellation
        match self.delegator.cancel_task(&task_id).await {
            Ok(()) => {
                // Remove from tracking
                self.last_task_per_chat.remove(chat_id);

                let reply = format!("⏹ Task `{}` has been cancelled.", task_id);
                let _ = send_telegram_message(bot_token, chat_id, &reply, Some(message_id)).await;

                tracing::info!(
                    chat_id = %chat_id,
                    task_id = %task_id,
                    "Coding agent task cancelled via /stop"
                );
                true
            }
            Err(err) => {
                let reply = format!("❌ Could not cancel task: {}", err);
                let _ = send_telegram_message(bot_token, chat_id, &reply, Some(message_id)).await;

                tracing::warn!(
                    chat_id = %chat_id,
                    task_id = %task_id,
                    error = %err,
                    "Failed to cancel coding agent task"
                );
                true // Still handled — don't fall through to normal /stop
            }
        }
    }

    /// Resolve the display name for an agent (by ID or alias).
    fn resolve_display_name(&self, agent_name: &str) -> String {
        // Try to get the agent from registry to find its backend display name
        let registry = self.delegator.registry();
        if let Some(agent) = registry.get_agent(agent_name) {
            // Try to get the backend definition for a nicer display name
            if let Some(backend) = registry.get_backend(&agent.backend_type) {
                return backend.display_name;
            }
            return agent.id;
        }

        // Try alias resolution
        if let Some(resolved_id) = registry.resolve_by_alias(agent_name) {
            if let Some(agent) = registry.get_agent(&resolved_id) {
                if let Some(backend) = registry.get_backend(&agent.backend_type) {
                    return backend.display_name;
                }
                return agent.id;
            }
        }

        // Fallback to the name as provided
        agent_name.to_string()
    }

    /// Format the list of available agents for error messages.
    fn format_available_agents(&self) -> String {
        let registry = self.delegator.registry();
        let agents = registry.list_agents();

        if agents.is_empty() {
            return "  (no agents registered)".to_string();
        }

        agents
            .iter()
            .map(|agent| {
                let alias_info = agent
                    .config
                    .alias
                    .as_ref()
                    .map(|a| format!(" (alias: {})", a))
                    .unwrap_or_default();
                format!("• {}{}", agent.id, alias_info)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Send a plain text message via the Telegram Bot API.
///
/// This is a standalone helper used by the delegation handler to send
/// replies without requiring a full `TelegramChannel` instance.
async fn send_telegram_message(
    bot_token: &str,
    chat_id: &str,
    text: &str,
    reply_to_message_id: Option<&str>,
) -> anyhow::Result<()> {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);

    let mut payload = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
        "parse_mode": "Markdown",
    });

    if let Some(reply_id) = reply_to_message_id {
        if let Ok(msg_id) = reply_id.parse::<i64>() {
            payload["reply_parameters"] = serde_json::json!({
                "message_id": msg_id,
            });
        }
    }

    let client = reqwest::Client::new();
    let resp = client.post(&url).json(&payload).send().await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::warn!(
            status = %status,
            body = %body,
            "Failed to send Telegram delegation reply"
        );
    }

    Ok(())
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

                let message_handler =
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

                // Callback query handler for inline button presses (tool approval + HITL permissions)
                let callback_handler = Update::filter_callback_query().endpoint(
                    move |q: teloxide::types::CallbackQuery, bot: teloxide::Bot| async move {
                        if let Some(data) = q.data {
                            // Handle coding agent HITL permission callbacks
                            if crate::coding_agent::hitl_permissions::is_perm_callback(&data) {
                                // Answer the callback to remove loading indicator
                                let _ = bot.answer_callback_query(&q.id).await;
                                // The actual handling happens via the HitlPermissionManager
                                // which is wired separately — for now just log
                                tracing::info!(callback_data = %data, "HITL permission callback received");
                            }
                            // Handle tool approval callbacks (approve:/reject:)
                            else if data.starts_with("approve:") || data.starts_with("reject:") {
                                let _ = bot.answer_callback_query(&q.id).await;
                                tracing::info!(callback_data = %data, "tool approval callback received");
                            }
                        }
                        respond(())
                    },
                );

                let handler = dptree::entry()
                    .branch(message_handler)
                    .branch(callback_handler);

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

    /// Send a photo to a chat. Used for image responses from tools like screenshot.
    async fn send_photo(&self, chat_id: &str, data: &[u8], mime_type: &str, caption: Option<&str>) -> anyhow::Result<()> {
        use teloxide::prelude::*;
        use teloxide::types::{ChatId, InputFile};

        let bot = self.bot.lock().await;
        let bot = bot.as_ref().ok_or_else(|| anyhow::anyhow!("telegram bot not initialized"))?;
        let id: i64 = chat_id.parse()?;

        let ext = match mime_type {
            "image/png" => "png",
            "image/jpeg" | "image/jpg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "png",
        };
        let file = InputFile::memory(data.to_vec()).file_name(format!("image.{ext}"));
        let mut request = bot.send_photo(ChatId(id), file);
        if let Some(cap) = caption {
            request = request.caption(cap);
        }
        request.await?;
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

#[cfg(test)]
mod delegation_tests {
    use super::*;
    use crate::coding_agent::config::CodingAgentInstanceConfig;
    use crate::coding_agent::cost::CostTracker;
    use crate::coding_agent::queue::TaskQueue;
    use crate::coding_agent::registry::CodingAgentRegistry;
    use crate::coding_agent::status::AgentConnectionStatus;
    use std::path::PathBuf;

    fn sample_agent_config(id: &str, alias: Option<&str>) -> CodingAgentInstanceConfig {
        CodingAgentInstanceConfig {
            id: id.to_string(),
            backend_type: "claude-code".to_string(),
            endpoint: format!("http://localhost:3000/{}", id),
            transport: None,
            workspaces: vec![PathBuf::from("/home/user/projects")],
            timeout_secs: Some(900),
            cost_cap_usd: Some(5.0),
            monthly_budget_usd: None,
            alias: alias.map(|a| a.to_string()),
            auth: None,
        }
    }

    fn make_handler_with_agents() -> TelegramDelegationHandler {
        let registry = Arc::new(CodingAgentRegistry::new(16));
        registry
            .register_agent(sample_agent_config("claude-code-1", Some("cc")))
            .unwrap();
        registry
            .update_status("claude-code-1", AgentConnectionStatus::Connected)
            .unwrap();
        registry
            .register_agent(sample_agent_config("kiro-cli-1", Some("kiro")))
            .unwrap();
        registry
            .update_status("kiro-cli-1", AgentConnectionStatus::Connected)
            .unwrap();

        let queue = TaskQueue::new(Some(3));
        let cost_tracker = Arc::new(CostTracker::new());
        let delegator = Arc::new(TaskDelegator::new(registry, queue, cost_tracker));

        TelegramDelegationHandler::new(delegator)
    }

    fn make_handler_empty() -> TelegramDelegationHandler {
        let registry = Arc::new(CodingAgentRegistry::new(16));
        let queue = TaskQueue::new(Some(3));
        let cost_tracker = Arc::new(CostTracker::new());
        let delegator = Arc::new(TaskDelegator::new(registry, queue, cost_tracker));

        TelegramDelegationHandler::new(delegator)
    }

    fn make_handler_with_disconnected_agent() -> TelegramDelegationHandler {
        let registry = Arc::new(CodingAgentRegistry::new(16));
        registry
            .register_agent(sample_agent_config("claude-code-1", Some("cc")))
            .unwrap();
        registry
            .update_status(
                "claude-code-1",
                AgentConnectionStatus::Disconnected {
                    since: chrono::Utc::now(),
                },
            )
            .unwrap();

        let queue = TaskQueue::new(Some(3));
        let cost_tracker = Arc::new(CostTracker::new());
        let delegator = Arc::new(TaskDelegator::new(registry, queue, cost_tracker));

        TelegramDelegationHandler::new(delegator)
    }

    #[tokio::test]
    async fn test_handle_delegation_not_a_command() {
        let handler = make_handler_with_agents();
        let result = handler
            .handle_delegation("hello world", "12345", "user1", "1", "fake_token")
            .await;
        assert!(matches!(result, DelegationResult::NotADelegationCommand));
    }

    #[tokio::test]
    async fn test_handle_delegation_success() {
        let handler = make_handler_with_agents();
        let result = handler
            .handle_delegation(
                "delegate to cc: fix the auth bug",
                "12345",
                "user1",
                "1",
                "fake_token",
            )
            .await;
        assert!(matches!(result, DelegationResult::Handled));

        // Verify task was tracked
        assert!(handler.last_task_per_chat.contains_key("12345"));
    }

    #[tokio::test]
    async fn test_handle_delegation_agent_not_found() {
        let handler = make_handler_empty();
        let result = handler
            .handle_delegation(
                "delegate to nonexistent: fix bug",
                "12345",
                "user1",
                "1",
                "fake_token",
            )
            .await;
        assert!(matches!(result, DelegationResult::Handled));

        // No task should be tracked
        assert!(!handler.last_task_per_chat.contains_key("12345"));
    }

    #[tokio::test]
    async fn test_handle_delegation_agent_disconnected() {
        let handler = make_handler_with_disconnected_agent();
        let result = handler
            .handle_delegation(
                "delegate to cc: fix bug",
                "12345",
                "user1",
                "1",
                "fake_token",
            )
            .await;
        assert!(matches!(result, DelegationResult::Handled));

        // No task should be tracked (delegation failed)
        assert!(!handler.last_task_per_chat.contains_key("12345"));
    }

    #[tokio::test]
    async fn test_handle_stop_no_tracked_task() {
        let handler = make_handler_with_agents();
        let handled = handler
            .handle_stop("/stop", "12345", "1", "fake_token")
            .await;
        // No tracked task, should return false
        assert!(!handled);
    }

    #[tokio::test]
    async fn test_handle_stop_with_tracked_task() {
        let handler = make_handler_with_agents();

        // First delegate a task to track it
        handler
            .handle_delegation(
                "delegate to cc: fix the auth bug",
                "12345",
                "user1",
                "1",
                "fake_token",
            )
            .await;

        // Give the queue time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Now /stop should find the tracked task
        let handled = handler
            .handle_stop("/stop", "12345", "2", "fake_token")
            .await;
        assert!(handled);

        // Task should be removed from tracking
        assert!(!handler.last_task_per_chat.contains_key("12345"));
    }

    #[tokio::test]
    async fn test_handle_stop_with_explicit_task_id() {
        let handler = make_handler_with_agents();

        // Delegate a task first
        handler
            .handle_delegation(
                "delegate to cc: fix bug",
                "12345",
                "user1",
                "1",
                "fake_token",
            )
            .await;

        // Give the queue time to process
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Get the tracked task ID
        let task_id = handler
            .last_task_per_chat
            .get("12345")
            .map(|e| e.value().clone())
            .unwrap();

        // Stop with explicit task ID
        let handled = handler
            .handle_stop(&format!("/stop {}", task_id), "12345", "2", "fake_token")
            .await;
        assert!(handled);
    }

    #[tokio::test]
    async fn test_handle_stop_non_stop_command() {
        let handler = make_handler_with_agents();
        let handled = handler
            .handle_stop("hello world", "12345", "1", "fake_token")
            .await;
        assert!(!handled);
    }

    #[test]
    fn test_format_available_agents_empty() {
        // Use a runtime since TaskQueue::new requires one
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let handler = make_handler_empty();
            let result = handler.format_available_agents();
            assert_eq!(result, "  (no agents registered)");
        });
    }

    #[test]
    fn test_format_available_agents_with_agents() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let handler = make_handler_with_agents();
            let result = handler.format_available_agents();
            assert!(result.contains("claude-code-1"));
            assert!(result.contains("(alias: cc)"));
            assert!(result.contains("kiro-cli-1"));
            assert!(result.contains("(alias: kiro)"));
        });
    }

    #[test]
    fn test_resolve_display_name_by_id() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let handler = make_handler_with_agents();
            // Without a backend definition loaded, it should fall back to the agent ID
            let name = handler.resolve_display_name("claude-code-1");
            assert_eq!(name, "claude-code-1");
        });
    }

    #[test]
    fn test_resolve_display_name_unknown() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let handler = make_handler_with_agents();
            let name = handler.resolve_display_name("nonexistent");
            assert_eq!(name, "nonexistent");
        });
    }
}
