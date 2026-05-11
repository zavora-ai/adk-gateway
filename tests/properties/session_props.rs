//! Property-based tests for session persistence round trip.
//!
//! Feature: gateway-production-maturity
//! - Property 13: Session persistence round trip
//!   **Validates: Requirements R6.1, R6.3**

use adk_gateway::channel::{ChannelType, InboundMessage, MessageSource};
use adk_gateway::config::{SessionConfig, SessionResetConfig};
use adk_gateway::session_bridge::SessionBridge;
use adk_session::InMemorySessionService;
use proptest::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;

// ── Strategies ─────────────────────────────────────────────────────

/// Arbitrary sender ID (non-empty alphanumeric string).
fn arb_sender_id() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_]{1,30}"
}

/// Arbitrary channel type.
fn arb_channel_type() -> impl Strategy<Value = ChannelType> {
    prop_oneof![
        Just(ChannelType::Telegram),
        Just(ChannelType::Slack),
        Just(ChannelType::Discord),
        Just(ChannelType::Whatsapp),
        Just(ChannelType::Signal),
        Just(ChannelType::Webhook),
    ]
}

/// Build an InboundMessage with the given sender_id and channel_type.
fn make_inbound_message(sender_id: &str, channel_type: ChannelType) -> InboundMessage {
    InboundMessage {
        channel_type,
        account_id: "default".to_string(),
        sender_id: sender_id.to_string(),
        sender_name: None,
        text: "hello".to_string(),
        is_group: false,
        group_id: None,
        is_mention: false,
        platform_message_id: "msg-1".to_string(),
        attachments: vec![],
        metadata: HashMap::new(),
        source: MessageSource::Channel,
        timestamp: chrono::Utc::now(),
    }
}

// ── Property tests ─────────────────────────────────────────────────

// Feature: gateway-production-maturity, Property 13: Session persistence round trip
// **Validates: Requirements R6.1, R6.3**
proptest! {
    /// Property 13: Session persistence round trip.
    ///
    /// For any sender_id and channel_type:
    /// 1. Create a SessionBridge with InMemorySessionService (R6.1)
    /// 2. Resolve a session for an InboundMessage
    /// 3. Resolve the same session again (same sender/channel)
    /// 4. Assert the same (user_id, session_id) is returned both times (R6.3)
    /// 5. Assert the session appears in active_sessions()
    #[test]
    fn session_persistence_round_trip(
        sender_id in arb_sender_id(),
        channel_type in arb_channel_type(),
    ) {
        let session_config = SessionConfig {
            dm_scope: "per-channel-peer".to_string(),
            reset: SessionResetConfig {
                mode: "idle".to_string(),
                at_hour: None,
                idle_minutes: Some(120),
            },
            backend: adk_gateway::config::SessionBackendType::InMemory,
            connection_string: None,
        };

        let session_service: Arc<dyn adk_session::SessionService> =
            Arc::new(InMemorySessionService::new());

        let bridge = SessionBridge::new(
            session_config,
            "test-app".to_string(),
            session_service,
        );

        let msg = make_inbound_message(&sender_id, channel_type);

        // First resolution — creates the session
        let (user_id_1, session_id_1) = bridge.resolve_session(&msg);

        // Second resolution — should return the same session
        let (user_id_2, session_id_2) = bridge.resolve_session(&msg);

        // Assert same (user_id, session_id) both times
        prop_assert_eq!(
            &user_id_1, &user_id_2,
            "user_id must be stable across resolve calls: {} vs {}",
            user_id_1, user_id_2
        );
        prop_assert_eq!(
            &session_id_1, &session_id_2,
            "session_id must be stable across resolve calls: {} vs {}",
            session_id_1, session_id_2
        );

        // Assert the session appears in active_sessions()
        let active = bridge.active_sessions();
        prop_assert!(
            !active.is_empty(),
            "active_sessions() must not be empty after resolving a session"
        );

        let found = active.iter().any(|info| {
            info.session_id == session_id_1
                && info.user_id == user_id_1
                && info.channel_type == channel_type
        });
        prop_assert!(
            found,
            "session (user_id={}, session_id={}, channel_type={}) must appear in active_sessions()",
            user_id_1, session_id_1, channel_type
        );
    }
}
