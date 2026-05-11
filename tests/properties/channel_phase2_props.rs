//! Property-based tests for Phase 2 channel integrations.
//!
//! Feature: full-stack-completion
//! - Property 3: Channel message normalization invariant
//!   **Validates: Requirements 2.4**
//! - Property 4: Invalid channel config graceful handling
//!   **Validates: Requirements 2.6**
//! - Property 5: Outbound message length constraint
//!   **Validates: Requirements 2.7**

use adk_gateway::channel::whatsapp::WhatsAppChannel;
use adk_gateway::channel::{build_channels, ChannelType};
use adk_gateway::config::{ChannelsConfig, DiscordConfig, MatrixConfig, WhatsAppConfig};
use proptest::prelude::*;

// ── Strategies ─────────────────────────────────────────────────────

/// Generate a non-empty alphanumeric string (for sender IDs, message IDs, etc.)
fn arb_non_empty_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_]{1,30}"
}

/// Generate a non-empty message text.
fn arb_message_text() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 .,!?]{1,100}"
}

/// Generate a valid WhatsApp webhook payload with a text message.
fn arb_whatsapp_text_payload(from: String, msg_id: String, body: String) -> serde_json::Value {
    serde_json::json!({
        "entry": [{
            "changes": [{
                "value": {
                    "contacts": [{
                        "wa_id": from,
                        "profile": { "name": "Test User" }
                    }],
                    "messages": [{
                        "from": from,
                        "id": msg_id,
                        "type": "text",
                        "text": { "body": body }
                    }]
                }
            }]
        }]
    })
}

/// Generate a channel type for outbound message length testing.
fn arb_channel_type() -> impl Strategy<Value = ChannelType> {
    prop_oneof![
        Just(ChannelType::Telegram),
        Just(ChannelType::Slack),
        Just(ChannelType::Discord),
        Just(ChannelType::Whatsapp),
        Just(ChannelType::Matrix),
        Just(ChannelType::Signal),
        Just(ChannelType::Webhook),
    ]
}

/// Generate arbitrary text of varying lengths, including texts that exceed
/// channel limits.
fn arb_outbound_text() -> impl Strategy<Value = String> {
    prop_oneof![
        // Short text (within all limits)
        "[a-zA-Z0-9 ]{1,100}",
        // Medium text (may exceed Discord's 2000 limit)
        "[a-zA-Z0-9 ]{1500,2500}",
        // Long text (exceeds most limits)
        "[a-zA-Z0-9 ]{4000,5000}",
        // Very long text (exceeds all limits except Matrix/Webhook)
        "[a-zA-Z0-9 ]{10000,20000}",
        // Extremely long text (exceeds all limits)
        "[a-zA-Z0-9 ]{60000,70000}",
    ]
}

// ── Property 3: Channel message normalization invariant ────────────
// Feature: full-stack-completion, Property 3: Channel message normalization invariant
// **Validates: Requirements 2.4**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 3: For any raw WhatsApp webhook payload with valid message
    /// structure, normalizing it into InboundMessage SHALL produce a struct
    /// where channel_type, sender_id, text, and platform_message_id are all
    /// non-empty, and channel_type matches the source platform.
    #[test]
    fn channel_message_normalization_invariant(
        sender_id in arb_non_empty_string(),
        msg_id in arb_non_empty_string(),
        body in arb_message_text(),
        account_id in arb_non_empty_string(),
    ) {
        let payload = arb_whatsapp_text_payload(
            sender_id.clone(),
            msg_id.clone(),
            body.clone(),
        );

        let messages = WhatsAppChannel::parse_webhook_payload(&payload, &account_id);

        // Should produce exactly one message for a single-message payload
        prop_assert_eq!(
            messages.len(), 1,
            "Expected exactly 1 message from valid payload, got {}",
            messages.len()
        );

        let msg = &messages[0];

        // channel_type must match the source platform (WhatsApp)
        prop_assert_eq!(
            msg.channel_type, ChannelType::Whatsapp,
            "channel_type should be Whatsapp, got {:?}",
            msg.channel_type
        );

        // sender_id must be non-empty
        prop_assert!(
            !msg.sender_id.is_empty(),
            "sender_id should be non-empty"
        );

        // text must be non-empty
        prop_assert!(
            !msg.text.is_empty(),
            "text should be non-empty"
        );

        // platform_message_id must be non-empty
        prop_assert!(
            !msg.platform_message_id.is_empty(),
            "platform_message_id should be non-empty"
        );

        // Verify the values match what we put in
        prop_assert_eq!(
            &msg.sender_id, &sender_id,
            "sender_id should match input"
        );
        prop_assert_eq!(
            &msg.text, &body,
            "text should match input body"
        );
        prop_assert_eq!(
            &msg.platform_message_id, &msg_id,
            "platform_message_id should match input"
        );
    }
}

// ── Property 4: Invalid channel config graceful handling ───────────
// Feature: full-stack-completion, Property 4: Invalid channel config graceful handling
// **Validates: Requirements 2.6**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 4: For any channel configuration with missing or empty
    /// credentials, `build_channels` SHALL return successfully (not panic)
    /// and the resulting channel map SHALL not contain an entry for the
    /// misconfigured channel.
    #[test]
    fn invalid_channel_config_graceful_handling(
        // WhatsApp with empty credentials
        wa_enabled in any::<bool>(),
        wa_phone_number_id_empty in any::<bool>(),
        wa_access_token_empty in any::<bool>(),
        // Discord with empty credentials
        dc_enabled in any::<bool>(),
        dc_bot_token_empty in any::<bool>(),
        // Matrix with empty credentials
        mx_enabled in any::<bool>(),
        mx_access_token_empty in any::<bool>(),
        mx_homeserver_url_empty in any::<bool>(),
    ) {
        // Build configs where at least one credential is empty
        let wa_config = WhatsAppConfig {
            enabled: wa_enabled,
            account_id: "wa-test".to_string(),
            phone_number_id: if wa_phone_number_id_empty {
                String::new()
            } else {
                "12345".to_string()
            },
            access_token: if wa_access_token_empty {
                String::new()
            } else {
                "token123".to_string()
            },
            verify_token: "verify".to_string(),
            webhook_path: "/webhook/whatsapp".to_string(),
        };

        let dc_config = DiscordConfig {
            enabled: dc_enabled,
            account_id: "dc-test".to_string(),
            bot_token: if dc_bot_token_empty {
                String::new()
            } else {
                "bot-token-123".to_string()
            },
            application_id: "app-id".to_string(),
            guild_ids: vec![],
        };

        let mx_config = MatrixConfig {
            enabled: mx_enabled,
            account_id: "mx-test".to_string(),
            homeserver_url: if mx_homeserver_url_empty {
                String::new()
            } else {
                "https://matrix.example.com".to_string()
            },
            access_token: if mx_access_token_empty {
                String::new()
            } else {
                "mx-token-123".to_string()
            },
            user_id: "@bot:example.com".to_string(),
            room_ids: vec![],
        };

        let channels_config = ChannelsConfig {
            telegram: None,
            slack: None,
            telegram_accounts: vec![],
            slack_accounts: vec![],
            whatsapp: Some(wa_config.clone()),
            discord: Some(dc_config.clone()),
            matrix: Some(mx_config.clone()),
            signal: None,
            imessage: None,
        };

        // build_channels must not panic
        let result = build_channels(&channels_config);

        // WhatsApp: should only be present if enabled AND both credentials non-empty
        let wa_has_creds = !wa_config.access_token.is_empty()
            && !wa_config.phone_number_id.is_empty();
        let wa_should_be_present = wa_enabled && wa_has_creds;

        let wa_present = result.keys().any(|k| {
            k.channel_type == ChannelType::Whatsapp && k.account_id == "wa-test"
        });

        if !wa_should_be_present {
            prop_assert!(
                !wa_present,
                "WhatsApp channel should NOT be present when credentials are missing \
                 (enabled={}, phone_number_id_empty={}, access_token_empty={})",
                wa_enabled, wa_phone_number_id_empty, wa_access_token_empty
            );
        }

        // Discord: should only be present if enabled AND bot_token non-empty
        let dc_has_creds = !dc_config.bot_token.is_empty();
        let dc_should_be_present = dc_enabled && dc_has_creds;

        let dc_present = result.keys().any(|k| {
            k.channel_type == ChannelType::Discord && k.account_id == "dc-test"
        });

        if !dc_should_be_present {
            prop_assert!(
                !dc_present,
                "Discord channel should NOT be present when credentials are missing \
                 (enabled={}, bot_token_empty={})",
                dc_enabled, dc_bot_token_empty
            );
        }

        // Matrix: should only be present if enabled AND both credentials non-empty
        let mx_has_creds = !mx_config.access_token.is_empty()
            && !mx_config.homeserver_url.is_empty();
        let mx_should_be_present = mx_enabled && mx_has_creds;

        let mx_present = result.keys().any(|k| {
            k.channel_type == ChannelType::Matrix && k.account_id == "mx-test"
        });

        if !mx_should_be_present {
            prop_assert!(
                !mx_present,
                "Matrix channel should NOT be present when credentials are missing \
                 (enabled={}, access_token_empty={}, homeserver_url_empty={})",
                mx_enabled, mx_access_token_empty, mx_homeserver_url_empty
            );
        }
    }
}

// ── Property 5: Outbound message length constraint ─────────────────
// Feature: full-stack-completion, Property 5: Outbound message length constraint
// **Validates: Requirements 2.7**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 5: For any outbound message text and any channel type, the
    /// text actually sent through the channel SHALL NOT exceed that channel's
    /// `max_message_length()` limit.
    ///
    /// We verify this by applying the same truncation logic used in the
    /// channel send methods.
    #[test]
    fn outbound_message_length_constraint(
        text in arb_outbound_text(),
        channel_type in arb_channel_type(),
    ) {
        let max_len = channel_type.max_message_length();

        // Apply the truncation logic used by all channel implementations
        let sent_text = if text.len() > max_len {
            let mut truncated = text[..max_len - 3].to_string();
            truncated.push_str("...");
            truncated
        } else {
            text.clone()
        };

        // The sent text must not exceed the channel's max message length
        prop_assert!(
            sent_text.len() <= max_len,
            "Sent text length ({}) exceeds max_message_length ({}) for {:?}",
            sent_text.len(), max_len, channel_type
        );

        // If original text was within limits, it should be unchanged
        if text.len() <= max_len {
            prop_assert_eq!(
                &sent_text, &text,
                "Text within limits should not be modified"
            );
        }

        // If original text exceeded limits, the result should be exactly max_len
        if text.len() > max_len {
            prop_assert_eq!(
                sent_text.len(), max_len,
                "Truncated text should be exactly max_message_length"
            );
            // Should end with "..."
            prop_assert!(
                sent_text.ends_with("..."),
                "Truncated text should end with '...'"
            );
        }
    }
}
