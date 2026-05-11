//! Channel abstraction — trait + implementations for each messaging platform.
//!
//! Each channel normalizes platform-specific messages into a unified
//! `InboundMessage` / `OutboundMessage` format that the gateway routes
//! to adk-rust agents.

pub mod discord;
pub mod matrix;
pub mod slack;
pub mod telegram;
pub mod whatsapp;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Unified inbound message from any channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMessage {
    /// Which channel this came from
    pub channel_type: ChannelType,
    /// Account identifier for multi-account support (R12)
    #[serde(default)]
    pub account_id: String,
    /// Unique sender ID (platform-specific format)
    pub sender_id: String,
    /// Human-readable sender name
    pub sender_name: Option<String>,
    /// The message text
    pub text: String,
    /// Whether this is a group/channel message vs DM
    pub is_group: bool,
    /// Group/channel ID if applicable
    pub group_id: Option<String>,
    /// Whether the bot was explicitly mentioned (for group messages)
    pub is_mention: bool,
    /// Platform-specific message ID for replies
    pub platform_message_id: String,
    /// Optional media attachments (URLs or base64)
    pub attachments: Vec<Attachment>,
    /// Arbitrary metadata (webhook/cron payloads, etc.)
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    /// Origin of this message
    #[serde(default)]
    pub source: MessageSource,
    /// When the message was received
    #[serde(default = "Utc::now")]
    pub timestamp: DateTime<Utc>,
}

/// Where an inbound message originated from.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MessageSource {
    #[default]
    Channel,
    Webhook {
        request_id: String,
    },
    Cron {
        job_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub mime_type: String,
    pub url: Option<String>,
    pub data: Option<String>,
    pub filename: Option<String>,
}

/// Outbound message to send back through a channel.
#[derive(Debug, Clone)]
pub struct OutboundMessage {
    /// Target channel
    #[allow(dead_code)]
    pub channel_type: ChannelType,
    /// Account identifier for multi-account support
    #[allow(dead_code)]
    pub account_id: String,
    /// Recipient ID
    pub recipient_id: String,
    /// The response text (markdown)
    pub text: String,
    /// Platform message ID to reply to
    pub reply_to: Option<String>,
    /// Whether this is a partial/streaming update
    #[allow(dead_code)]
    pub is_partial: bool,
}

/// Message edit request for streaming delivery (R2).
#[derive(Debug, Clone)]
pub struct EditMessage {
    /// Target channel
    #[allow(dead_code)] // Read by channel implementations during edit dispatch
    pub channel_type: ChannelType,
    /// Account identifier for multi-account support
    #[allow(dead_code)] // Read by channel implementations during edit dispatch
    pub account_id: String,
    /// Platform message ID to edit
    pub message_id: String,
    /// Recipient / chat ID
    pub recipient_id: String,
    /// Updated text content
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelType {
    Telegram,
    Slack,
    Discord,
    Whatsapp,
    Matrix,
    Signal,
    Imessage,
    Webhook,
}

impl ChannelType {
    /// Returns the maximum message length for this channel type.
    /// Based on well-known platform limits.
    pub fn max_message_length(&self) -> usize {
        match self {
            ChannelType::Telegram => 4096,
            ChannelType::Slack => 4000,
            ChannelType::Discord => 2000,
            ChannelType::Whatsapp => 4096,
            ChannelType::Matrix => 65536,
            ChannelType::Signal => 6000,
            ChannelType::Imessage => 20000,
            ChannelType::Webhook => 65536,
        }
    }
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelType::Telegram => write!(f, "telegram"),
            ChannelType::Slack => write!(f, "slack"),
            ChannelType::Discord => write!(f, "discord"),
            ChannelType::Whatsapp => write!(f, "whatsapp"),
            ChannelType::Matrix => write!(f, "matrix"),
            ChannelType::Signal => write!(f, "signal"),
            ChannelType::Imessage => write!(f, "imessage"),
            ChannelType::Webhook => write!(f, "webhook"),
        }
    }
}

/// Composite key for identifying a channel instance (channel_type + account_id).
/// Supports multi-account setups where the same platform has multiple bot accounts (R12).
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChannelKey {
    pub channel_type: ChannelType,
    pub account_id: String,
}

impl std::fmt::Display for ChannelKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.channel_type, self.account_id)
    }
}

/// Health status of a channel connection.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelHealth {
    pub status: ChannelStatus,
    pub last_connected: Option<DateTime<Utc>>,
    pub reconnect_attempts: u32,
    pub error: Option<String>,
}

/// Connection state of a channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChannelStatus {
    Connected,
    Reconnecting,
    Failed,
    Disconnected,
}

/// The core channel trait. Each platform implements this.
#[async_trait]
pub trait Channel: Send + Sync + 'static {
    /// Channel type identifier
    fn channel_type(&self) -> ChannelType;

    /// Account identifier for multi-account support (R12)
    fn account_id(&self) -> &str {
        "default"
    }

    /// Start listening for messages. Sends inbound messages to the provided sender.
    async fn start(&self, tx: mpsc::Sender<InboundMessage>) -> anyhow::Result<()>;

    /// Send a message back through this channel.
    /// Returns the platform message ID if available (used for streaming edits).
    async fn send(&self, msg: OutboundMessage) -> anyhow::Result<Option<String>>;

    /// Edit an existing message in-place for streaming delivery (R2).
    async fn edit(&self, _msg: EditMessage) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("editing not supported by this channel"))
    }

    /// Whether this channel supports in-place message editing.
    fn supports_editing(&self) -> bool {
        false
    }

    /// Check channel health with detailed status.
    async fn health_check(&self) -> anyhow::Result<ChannelHealth> {
        Ok(ChannelHealth {
            status: ChannelStatus::Connected,
            last_connected: Some(Utc::now()),
            reconnect_attempts: 0,
            error: None,
        })
    }

    /// Graceful shutdown
    async fn shutdown(&self) -> anyhow::Result<()>;
}

/// Print status for all configured channels.
pub async fn print_status(channels: &crate::config::ChannelsConfig, probe: bool) {
    println!("Channel Status:");
    println!("───────────────────────────────────");

    if let Some(ref tg) = channels.telegram {
        let status = if tg.bot_token.is_empty() {
            "⚠ no token".to_string()
        } else if tg.enabled {
            if probe {
                let tg_channel = telegram::TelegramChannel::new(tg.clone());
                match tg_channel.probe().await {
                    telegram::ProbeResult::Connected { bot_username } => {
                        format!("✓ connected (@{bot_username})")
                    }
                    telegram::ProbeResult::InvalidToken => "✗ invalid token".to_string(),
                    telegram::ProbeResult::Unreachable { timeout_ms } => {
                        format!("✗ unreachable ({timeout_ms}ms)")
                    }
                    telegram::ProbeResult::Error { message } => {
                        format!("✗ error: {message}")
                    }
                }
            } else {
                "✓ configured".to_string()
            }
        } else {
            "○ disabled".to_string()
        };
        println!("  telegram: {status}");
    } else {
        println!("  telegram: ○ not configured");
    }

    if let Some(ref sl) = channels.slack {
        let status = if sl.bot_token.is_empty() {
            "⚠ no bot token"
        } else if sl.enabled {
            "✓ configured"
        } else {
            "○ disabled"
        };
        println!("  slack:    {status}");
    } else {
        println!("  slack:    ○ not configured");
    }

    // Phase 2 channels — WhatsApp
    if let Some(ref wa) = channels.whatsapp {
        let status = if wa.access_token.is_empty() || wa.phone_number_id.is_empty() {
            "⚠ missing credentials"
        } else if wa.enabled {
            "✓ configured"
        } else {
            "○ disabled"
        };
        println!("  whatsapp: {status}");
    } else {
        println!("  whatsapp: ○ not configured");
    }

    // Phase 2 channels — Discord
    if let Some(ref dc) = channels.discord {
        let status = if dc.bot_token.is_empty() {
            "⚠ no bot token"
        } else if dc.enabled {
            "✓ configured"
        } else {
            "○ disabled"
        };
        println!("  discord:  {status}");
    } else {
        println!("  discord:  ○ not configured");
    }

    // Phase 2 channels — Matrix
    if let Some(ref mx) = channels.matrix {
        let status = if mx.access_token.is_empty() || mx.homeserver_url.is_empty() {
            "⚠ missing credentials"
        } else if mx.enabled {
            "✓ configured"
        } else {
            "○ disabled"
        };
        println!("  matrix:   {status}");
    } else {
        println!("  matrix:   ○ not configured");
    }

    // Remaining Phase 2 channels (not yet implemented)
    for (name, val) in [
        ("signal", &channels.signal),
        ("imessage", &channels.imessage),
    ] {
        if val.is_some() {
            println!("  {name:10} ○ phase 2 (not yet supported)");
        }
    }
}

/// Build all enabled channels from config.
///
/// Returns a map of `ChannelKey` → `Arc<dyn Channel>` supporting
/// multi-account setups (R12.1).
pub fn build_channels(
    config: &crate::config::ChannelsConfig,
) -> std::collections::HashMap<ChannelKey, Arc<dyn Channel>> {
    let mut channels: std::collections::HashMap<ChannelKey, Arc<dyn Channel>> =
        std::collections::HashMap::new();

    // Primary Telegram account
    if let Some(ref tg) = config.telegram {
        if tg.enabled && !tg.bot_token.is_empty() {
            let key = ChannelKey {
                channel_type: ChannelType::Telegram,
                account_id: tg.account_id.clone(),
            };
            channels.insert(key, Arc::new(telegram::TelegramChannel::new(tg.clone())));
            tracing::info!(account_id = %tg.account_id, "telegram channel enabled");
        }
    }

    // Additional Telegram accounts (R12)
    for tg in &config.telegram_accounts {
        if tg.enabled && !tg.bot_token.is_empty() {
            let key = ChannelKey {
                channel_type: ChannelType::Telegram,
                account_id: tg.account_id.clone(),
            };
            channels.insert(key, Arc::new(telegram::TelegramChannel::new(tg.clone())));
            tracing::info!(account_id = %tg.account_id, "telegram channel (multi-account) enabled");
        }
    }

    // Primary Slack account
    if let Some(ref sl) = config.slack {
        if sl.enabled && !sl.bot_token.is_empty() {
            let key = ChannelKey {
                channel_type: ChannelType::Slack,
                account_id: sl.account_id.clone(),
            };
            channels.insert(key, Arc::new(slack::SlackChannel::new(sl.clone())));
            tracing::info!(account_id = %sl.account_id, "slack channel enabled");
        }
    }

    // Additional Slack accounts (R12)
    for sl in &config.slack_accounts {
        if sl.enabled && !sl.bot_token.is_empty() {
            let key = ChannelKey {
                channel_type: ChannelType::Slack,
                account_id: sl.account_id.clone(),
            };
            channels.insert(key, Arc::new(slack::SlackChannel::new(sl.clone())));
            tracing::info!(account_id = %sl.account_id, "slack channel (multi-account) enabled");
        }
    }

    // WhatsApp channel
    if let Some(ref wa) = config.whatsapp {
        if wa.enabled && !wa.access_token.is_empty() && !wa.phone_number_id.is_empty() {
            let key = ChannelKey {
                channel_type: ChannelType::Whatsapp,
                account_id: wa.account_id.clone(),
            };
            channels.insert(key, Arc::new(whatsapp::WhatsAppChannel::new(wa.clone())));
            tracing::info!(account_id = %wa.account_id, "whatsapp channel enabled");
        } else if wa.enabled {
            tracing::warn!("whatsapp channel enabled but missing credentials (access_token or phone_number_id), skipping");
        }
    }

    // Discord channel
    if let Some(ref dc) = config.discord {
        if dc.enabled && !dc.bot_token.is_empty() {
            let key = ChannelKey {
                channel_type: ChannelType::Discord,
                account_id: dc.account_id.clone(),
            };
            channels.insert(key, Arc::new(discord::DiscordChannel::new(dc.clone())));
            tracing::info!(account_id = %dc.account_id, "discord channel enabled");
        } else if dc.enabled {
            tracing::warn!("discord channel enabled but missing bot_token, skipping");
        }
    }

    // Matrix channel
    if let Some(ref mx) = config.matrix {
        if mx.enabled && !mx.access_token.is_empty() && !mx.homeserver_url.is_empty() {
            let key = ChannelKey {
                channel_type: ChannelType::Matrix,
                account_id: mx.account_id.clone(),
            };
            channels.insert(key, Arc::new(matrix::MatrixChannel::new(mx.clone())));
            tracing::info!(account_id = %mx.account_id, "matrix channel enabled");
        } else if mx.enabled {
            tracing::warn!("matrix channel enabled but missing credentials (access_token or homeserver_url), skipping");
        }
    }

    channels
}
