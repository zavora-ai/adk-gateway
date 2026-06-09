//! Human-in-the-loop permission handling for coding agent ACP sessions.
//!
//! Routes permission requests from coding agents to the user via Telegram
//! inline keyboard buttons. Waits indefinitely for the user's response
//! (no auto-deny timeout).

use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::oneshot;
use tracing::{info, warn};
use uuid::Uuid;

use adk_acp::permissions::{PermissionDecision, PermissionPolicy, PermissionRequest};

/// A pending permission request waiting for user response.
struct PendingPermission {
    /// Sender to deliver the user's decision.
    tx: oneshot::Sender<bool>,
}

/// Manages HITL permission requests for coding agents.
///
/// When a coding agent requests permission (e.g., "delete file", "run command"),
/// this sends a Telegram message with inline buttons and waits for the user's response.
pub struct HitlPermissionManager {
    /// Pending permission requests keyed by unique ID.
    pending: DashMap<String, PendingPermission>,
    /// Telegram bot token for sending messages.
    bot_token: String,
    /// Chat ID to send permission requests to.
    chat_id: String,
}

impl HitlPermissionManager {
    /// Create a new HITL permission manager.
    pub fn new(bot_token: String, chat_id: String) -> Self {
        Self {
            pending: DashMap::new(),
            bot_token,
            chat_id,
        }
    }

    /// Build a permission policy that routes requests through Telegram.
    ///
    /// The returned policy blocks the calling thread until the user responds.
    /// This is safe because ACP sessions run in their own tokio tasks.
    pub fn build_policy(self: &Arc<Self>) -> PermissionPolicy {
        let manager = self.clone();
        PermissionPolicy::Custom(Box::new(move |request: &PermissionRequest| {
            // Create a oneshot channel for the response
            let (tx, rx) = oneshot::channel();
            let perm_id = Uuid::new_v4().to_string();

            // Store the pending request
            manager.pending.insert(perm_id.clone(), PendingPermission { tx });

            // Send the Telegram message with inline keyboard (fire-and-forget)
            let bot_token = manager.bot_token.clone();
            let chat_id = manager.chat_id.clone();
            let title = request.title.clone();
            let perm_id_clone = perm_id.clone();

            // Spawn the Telegram send in a background task
            tokio::spawn(async move {
                let message = format!(
                    "🔐 *Coding Agent Permission Request*\n\n{}\n\n_Waiting for your decision..._",
                    title
                );

                let keyboard = serde_json::json!({
                    "inline_keyboard": [[
                        {
                            "text": "✅ Allow",
                            "callback_data": format!("perm_allow:{}", perm_id_clone)
                        },
                        {
                            "text": "❌ Deny",
                            "callback_data": format!("perm_deny:{}", perm_id_clone)
                        }
                    ]]
                });

                let body = serde_json::json!({
                    "chat_id": chat_id,
                    "text": message,
                    "parse_mode": "Markdown",
                    "reply_markup": keyboard,
                });

                let client = reqwest::Client::new();
                let url = format!("https://api.telegram.org/bot{}/sendMessage", bot_token);
                if let Err(e) = client.post(&url).json(&body).send().await {
                    warn!(error = %e, "failed to send permission request to Telegram");
                }
            });

            // Block waiting for user response (no timeout — wait indefinitely)
            match rx.blocking_recv() {
                Ok(true) => {
                    info!(perm_id = %perm_id, title = %request.title, "permission GRANTED by user");
                    // Select the first option (allow_once)
                    request
                        .options
                        .first()
                        .map(|opt| PermissionDecision::Allow(opt.id.clone()))
                        .unwrap_or_else(PermissionDecision::allow_once)
                }
                Ok(false) => {
                    info!(perm_id = %perm_id, title = %request.title, "permission DENIED by user");
                    PermissionDecision::Deny
                }
                Err(_) => {
                    warn!(perm_id = %perm_id, "permission channel closed — denying");
                    PermissionDecision::Deny
                }
            }
        }))
    }

    /// Handle a callback from a Telegram inline button press.
    ///
    /// Called by the Telegram callback handler when the user taps Allow/Deny.
    pub fn handle_callback(&self, callback_data: &str) -> bool {
        if let Some((perm_id, approved)) = parse_perm_callback(callback_data) {
            if let Some((_, pending)) = self.pending.remove(perm_id) {
                let _ = pending.tx.send(approved);
                info!(perm_id = %perm_id, approved = approved, "permission callback handled");
                return true;
            }
        }
        false
    }
}

/// Parse a permission callback data string.
///
/// Format: `"perm_allow:<id>"` or `"perm_deny:<id>"`.
fn parse_perm_callback(data: &str) -> Option<(&str, bool)> {
    if let Some(id) = data.strip_prefix("perm_allow:") {
        Some((id, true))
    } else if let Some(id) = data.strip_prefix("perm_deny:") {
        Some((id, false))
    } else {
        None
    }
}

/// Check if a callback data string is a permission callback.
pub fn is_perm_callback(data: &str) -> bool {
    data.starts_with("perm_allow:") || data.starts_with("perm_deny:")
}
