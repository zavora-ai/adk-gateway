//! Property-based tests for channel reconnection and multi-account support.
//!
//! Feature: gateway-production-maturity
//! - Property 15: Channel reconnection exponential backoff
//!   **Validates: R8.1**
//! - Property 16: Channel health reports reconnecting status
//!   **Validates: R8.2**
//! - Property 23: Multi-account session key includes account_id
//!   **Validates: R12.1, R12.2, R12.3**

use adk_gateway::channel::InboundMessage;
use adk_gateway::channel::{ChannelStatus, ChannelType};
use adk_gateway::config::SessionConfig;
use adk_gateway::reconnect::{ReconnectPolicy, ReconnectState};
use adk_gateway::session_bridge::SessionBridge;
use adk_session::InMemorySessionService;
use proptest::prelude::*;
use std::sync::Arc;
use std::time::Duration;

// ── Strategies ─────────────────────────────────────────────────────

fn arb_reconnect_policy() -> impl Strategy<Value = ReconnectPolicy> {
    (1u64..10, 10u64..600, 1u32..20).prop_map(|(initial_secs, max_secs, max_attempts)| {
        ReconnectPolicy {
            initial_delay: Duration::from_secs(initial_secs),
            max_delay: Duration::from_secs(max_secs),
            max_attempts,
        }
    })
}

fn arb_channel_type() -> impl Strategy<Value = ChannelType> {
    prop_oneof![Just(ChannelType::Telegram), Just(ChannelType::Slack),]
}

fn arb_non_empty_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_]{1,20}".prop_map(|s| s)
}

fn make_inbound(channel_type: ChannelType, sender_id: &str, account_id: &str) -> InboundMessage {
    InboundMessage {
        channel_type,
        account_id: account_id.to_string(),
        sender_id: sender_id.to_string(),
        sender_name: None,
        text: "hello".to_string(),
        is_group: false,
        group_id: None,
        is_mention: false,
        platform_message_id: "1".to_string(),
        attachments: vec![],
        metadata: std::collections::HashMap::new(),
        source: Default::default(),
        timestamp: chrono::Utc::now(),
    }
}

// ── Property 15: Channel reconnection exponential backoff ──────────
// **Validates: R8.1**

proptest! {
    /// Property 15: For any ReconnectPolicy, calling next_delay() N times
    /// produces delays that:
    /// 1. Start at initial_delay
    /// 2. Double each time (exponential backoff)
    /// 3. Never exceed max_delay
    #[test]
    fn reconnect_exponential_backoff(
        policy in arb_reconnect_policy(),
        n in 1u32..15,
    ) {
        let mut state = ReconnectState::new(policy.clone());
        let mut prev_delay = None;

        for i in 0..n {
            let delay = state.next_delay();

            // First delay must equal initial_delay
            if i == 0 {
                prop_assert_eq!(
                    delay, policy.initial_delay,
                    "first delay should be initial_delay"
                );
            }

            // Delay must never exceed max_delay
            prop_assert!(
                delay <= policy.max_delay,
                "delay {:?} exceeded max_delay {:?} at attempt {}",
                delay, policy.max_delay, i
            );

            // Each delay should be >= previous (monotonically non-decreasing)
            if let Some(prev) = prev_delay {
                prop_assert!(
                    delay >= prev,
                    "delay {:?} decreased from {:?} at attempt {}",
                    delay, prev, i
                );
            }

            prev_delay = Some(delay);
        }

        // Attempt count should match
        prop_assert_eq!(state.attempts, n);
    }
}

// ── Property 16: Channel health reports reconnecting status ────────
// **Validates: R8.2**

proptest! {
    /// Property 16: For any ReconnectPolicy, the channel_status() method
    /// correctly transitions through Connected → Reconnecting → Failed
    /// based on the number of attempts.
    #[test]
    fn channel_health_reports_reconnecting_status(
        policy in arb_reconnect_policy(),
    ) {
        let mut state = ReconnectState::new(policy.clone());

        // Before any attempts: Connected
        prop_assert_eq!(
            state.channel_status(), ChannelStatus::Connected,
            "status should be Connected before any attempts"
        );

        // After 1..max_attempts-1 attempts: Reconnecting
        for i in 1..policy.max_attempts {
            state.next_delay();
            prop_assert_eq!(
                state.channel_status(), ChannelStatus::Reconnecting,
                "status should be Reconnecting at attempt {}", i
            );
        }

        // At max_attempts: Failed
        state.next_delay();
        prop_assert_eq!(
            state.channel_status(), ChannelStatus::Failed,
            "status should be Failed at max_attempts"
        );

        // After reset: back to Connected
        state.reset();
        prop_assert_eq!(
            state.channel_status(), ChannelStatus::Connected,
            "status should be Connected after reset"
        );
    }
}

// ── Property 23: Multi-account session key includes account_id ─────
// **Validates: R12.1, R12.2, R12.3**

proptest! {
    /// Property 23: When dmScope is "per-account-channel-peer", the
    /// session key includes the account_id, so two messages from the
    /// same sender on different accounts produce different session keys.
    #[test]
    fn multi_account_session_key_includes_account_id(
        channel_type in arb_channel_type(),
        sender_id in arb_non_empty_string(),
        account_a in arb_non_empty_string(),
        account_b in arb_non_empty_string(),
    ) {
        let config = SessionConfig {
            dm_scope: "per-account-channel-peer".to_string(),
            ..SessionConfig::default()
        };
        let service = Arc::new(InMemorySessionService::new());
        let bridge = SessionBridge::new(config, "test".into(), service);

        let msg_a = make_inbound(channel_type, &sender_id, &account_a);
        let msg_b = make_inbound(channel_type, &sender_id, &account_b);

        let (_, session_a) = bridge.resolve_session(&msg_a);
        let (_, session_b) = bridge.resolve_session(&msg_b);

        if account_a == account_b {
            // Same account → same session
            prop_assert_eq!(
                session_a, session_b,
                "same account_id should produce same session"
            );
        } else {
            // Different accounts → different sessions
            prop_assert_ne!(
                session_a, session_b,
                "different account_ids should produce different sessions"
            );
        }
    }

    /// Property 23 (supplementary): When dmScope is NOT
    /// "per-account-channel-peer", account_id does not affect the
    /// session key.
    #[test]
    fn non_account_scope_ignores_account_id(
        channel_type in arb_channel_type(),
        sender_id in arb_non_empty_string(),
        account_a in arb_non_empty_string(),
        account_b in arb_non_empty_string(),
    ) {
        let config = SessionConfig {
            dm_scope: "per-channel-peer".to_string(),
            ..SessionConfig::default()
        };
        let service = Arc::new(InMemorySessionService::new());
        let bridge = SessionBridge::new(config, "test".into(), service);

        let msg_a = make_inbound(channel_type, &sender_id, &account_a);
        let msg_b = make_inbound(channel_type, &sender_id, &account_b);

        let (_, session_a) = bridge.resolve_session(&msg_a);
        let (_, session_b) = bridge.resolve_session(&msg_b);

        // per-channel-peer ignores account_id, so sessions should be the same
        prop_assert_eq!(
            session_a, session_b,
            "per-channel-peer scope should ignore account_id"
        );
    }
}

// ── Property 16: Reconnect state machine lifecycle ─────────────────
// Feature: gateway-full-wiring, Property 16: Reconnect state machine lifecycle
// **Validates: Requirements 12.2, 12.3, 12.4**

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 16: For any ReconnectPolicy, ReconnectState should produce
    /// exponentially doubling delays capped at max_delay, report
    /// should_mark_failed() after max_attempts, and after reset() return
    /// to initial state.
    #[test]
    fn reconnect_state_machine_lifecycle(
        policy in arb_reconnect_policy(),
    ) {
        let mut state = ReconnectState::new(policy.clone());

        // (a) Verify exponentially doubling delays capped at max_delay
        let mut expected_delay = policy.initial_delay;
        for i in 0..policy.max_attempts {
            let delay = state.next_delay();

            // Delay should match expected (doubling, capped)
            prop_assert_eq!(
                delay, expected_delay,
                "delay at attempt {} should be {:?}, got {:?}",
                i, expected_delay, delay
            );

            // Delay must never exceed max_delay
            prop_assert!(
                delay <= policy.max_delay,
                "delay {:?} exceeded max_delay {:?} at attempt {}",
                delay, policy.max_delay, i
            );

            // Compute next expected delay (double, capped)
            expected_delay = std::cmp::min(
                expected_delay.saturating_mul(2),
                policy.max_delay,
            );
        }

        // (b) After max_attempts calls, should_mark_failed() must be true
        prop_assert!(
            state.should_mark_failed(),
            "should_mark_failed() should be true after {} attempts",
            policy.max_attempts
        );
        prop_assert_eq!(
            state.channel_status(), ChannelStatus::Failed,
            "channel_status should be Failed after max_attempts"
        );

        // (c) After reset(), return to initial state
        state.reset();
        prop_assert_eq!(state.attempts, 0, "attempts should be 0 after reset");
        prop_assert_eq!(
            state.channel_status(), ChannelStatus::Connected,
            "channel_status should be Connected after reset"
        );
        // First delay after reset should be initial_delay again
        let first_delay = state.next_delay();
        prop_assert_eq!(
            first_delay, policy.initial_delay,
            "first delay after reset should be initial_delay"
        );
    }
}
