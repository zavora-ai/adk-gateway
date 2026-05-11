//! Property tests for MessageRouter routing completeness.
//!
//! **Validates: Requirements 11**
//! Correctness Property 8: Every inbound message resolves to exactly one agent
//! (system agent as fallback).

use adk_gateway::agent_config::ChannelBinding;
use adk_gateway::channel::{ChannelType, InboundMessage, MessageSource};
use adk_gateway::config::RoutingConfig;
use adk_gateway::router::MessageRouter;
use proptest::prelude::*;

/// Strategy to generate an arbitrary ChannelType.
fn arb_channel_type() -> impl Strategy<Value = ChannelType> {
    prop_oneof![
        Just(ChannelType::Telegram),
        Just(ChannelType::Slack),
        Just(ChannelType::Discord),
        Just(ChannelType::Whatsapp),
        Just(ChannelType::Signal),
        Just(ChannelType::Imessage),
        Just(ChannelType::Webhook),
    ]
}

/// Strategy to generate a channel type name string matching ChannelType variants.
fn arb_channel_name() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("telegram".to_string()),
        Just("slack".to_string()),
        Just("discord".to_string()),
        Just("whatsapp".to_string()),
        Just("signal".to_string()),
        Just("imessage".to_string()),
        Just("webhook".to_string()),
    ]
}

/// Strategy to generate an arbitrary ChannelBinding.
fn arb_channel_binding() -> impl Strategy<Value = (String, ChannelBinding)> {
    (
        "[a-z]{3,8}",                              // agent_id
        arb_channel_name(),                        // channel_type
        proptest::option::of("[a-z0-9\\-]{1,10}"), // account_id
        proptest::option::of("[a-z0-9]{1,10}"),    // peer_filter
    )
        .prop_map(|(agent_id, channel_type, account_id, peer_filter)| {
            (
                agent_id,
                ChannelBinding {
                    channel_type,
                    account_id,
                    peer_filter,
                },
            )
        })
}

/// Strategy to generate an arbitrary InboundMessage.
fn arb_inbound_message() -> impl Strategy<Value = InboundMessage> {
    (
        arb_channel_type(),
        "[a-z0-9\\-]{0,10}", // account_id
        "[a-z0-9]{1,10}",    // sender_id
        ".*",                // text
    )
        .prop_map(
            |(channel_type, account_id, sender_id, text)| InboundMessage {
                channel_type,
                account_id,
                sender_id,
                sender_name: None,
                text,
                is_group: false,
                group_id: None,
                is_mention: false,
                platform_message_id: "1".to_string(),
                attachments: vec![],
                metadata: std::collections::HashMap::new(),
                source: MessageSource::Channel,
                timestamp: chrono::Utc::now(),
            },
        )
}

proptest! {
    /// **Validates: Requirements 11**
    ///
    /// Correctness Property 8: Routing completeness — every message
    /// resolves to exactly one agent. The system agent is always the
    /// fallback, so resolve_agent() never returns an empty string.
    #[test]
    fn routing_always_resolves_to_exactly_one_agent(
        bindings in proptest::collection::vec(arb_channel_binding(), 0..10),
        msg in arb_inbound_message(),
    ) {
        let mut router = MessageRouter::new(
            &RoutingConfig { bindings: vec![] },
            "system".to_string(),
        );

        for (agent_id, binding) in &bindings {
            router.add_agent_bindings(agent_id, &[binding.clone()]);
        }

        let resolved = router.resolve_agent(&msg);

        // Property: resolve_agent always returns a non-empty agent ID
        prop_assert!(!resolved.is_empty(), "resolved agent ID must not be empty");

        // Property: the resolved agent is either one of the bound agents or "system"
        let bound_agent_ids: Vec<&str> = bindings.iter().map(|(id, _)| id.as_str()).collect();
        prop_assert!(
            resolved == "system" || bound_agent_ids.contains(&resolved),
            "resolved agent '{}' must be 'system' or one of the bound agents {:?}",
            resolved,
            bound_agent_ids,
        );
    }

    /// **Validates: Requirements 11**
    ///
    /// Additional property: when no bindings are configured, every message
    /// resolves to the default (system) agent.
    #[test]
    fn empty_bindings_always_resolve_to_default(
        msg in arb_inbound_message(),
    ) {
        let router = MessageRouter::new(
            &RoutingConfig { bindings: vec![] },
            "system".to_string(),
        );

        let resolved = router.resolve_agent(&msg);
        prop_assert_eq!(resolved, "system");
    }
}
