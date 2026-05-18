//! Property-based tests for Multi-User Support.
//!
//! Feature: phase-2-complete, Property 9: Multi-User Pairing Independence
//! Feature: phase-2-complete, Property 10: Agent Routing Correctness
//! **Validates: Requirements 8.1, 8.2, 8.5, 8.6, 8.7**

use adk_gateway::channel::ChannelType;
use adk_gateway::multi_user::{
    AgentRouter, AgentRoutingRule, MultiUserManager, PairedUser, SessionMessage, ThreadContext,
};
use chrono::Utc;
use proptest::prelude::*;

// ── Strategies ─────────────────────────────────────────────────────

/// Strategy for generating a channel type.
fn arb_channel_type() -> impl Strategy<Value = ChannelType> {
    prop_oneof![
        Just(ChannelType::Telegram),
        Just(ChannelType::Slack),
        Just(ChannelType::Discord),
        Just(ChannelType::Whatsapp),
        Just(ChannelType::Matrix),
    ]
}

/// Strategy for generating a user ID.
fn arb_user_id() -> impl Strategy<Value = String> {
    "[a-z]{1}[a-z0-9]{2,8}".prop_map(|s| format!("user-{}", s))
}

/// Strategy for generating a group ID.
fn arb_group_id() -> impl Strategy<Value = String> {
    "[a-z]{1}[a-z0-9]{2,6}".prop_map(|s| format!("group-{}", s))
}

/// Strategy for generating an agent ID.
fn arb_agent_id() -> impl Strategy<Value = String> {
    "[a-z]{1}[a-z0-9]{2,6}".prop_map(|s| format!("agent-{}", s))
}

/// Strategy for generating a paired user.
fn arb_paired_user() -> impl Strategy<Value = PairedUser> {
    (arb_user_id(), arb_channel_type()).prop_map(|(user_id, channel_type)| {
        PairedUser::new(user_id, channel_type)
    })
}

/// Strategy for generating a set of paired users with unique (channel, user_id) keys.
fn arb_unique_users(min: usize, max: usize) -> impl Strategy<Value = Vec<PairedUser>> {
    proptest::collection::vec(arb_paired_user(), min..=max).prop_map(|users| {
        let mut seen = std::collections::HashSet::new();
        users
            .into_iter()
            .filter(|u| seen.insert((u.channel_type, u.user_id.clone())))
            .collect()
    })
}

/// Strategy for generating a routing rule.
fn arb_routing_rule() -> impl Strategy<Value = AgentRoutingRule> {
    (
        arb_group_id(),
        proptest::option::of("[a-z]{1}[a-z0-9]{2,4}".prop_map(|s| format!("thread-{}", s))),
        arb_agent_id(),
    )
        .prop_map(|(group_id, thread_id, agent_id)| AgentRoutingRule {
            group_id,
            thread_id,
            agent_id,
        })
}

/// Strategy for generating a set of routing rules with unique group_ids
/// (only group-level rules, no thread-specific, for simpler property verification).
fn arb_group_level_rules(min: usize, max: usize) -> impl Strategy<Value = Vec<AgentRoutingRule>> {
    proptest::collection::vec(
        (arb_group_id(), arb_agent_id()).prop_map(|(group_id, agent_id)| AgentRoutingRule {
            group_id,
            thread_id: None,
            agent_id,
        }),
        min..=max,
    )
    .prop_map(|rules| {
        let mut seen = std::collections::HashSet::new();
        rules
            .into_iter()
            .filter(|r| seen.insert(r.group_id.clone()))
            .collect()
    })
}

// ── Property Tests ─────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    // Feature: phase-2-complete, Property 9: Multi-User Pairing Independence
    // **Validates: Requirements 8.1, 8.2**
    //
    // For any set of paired users on the same channel, adding a new user SHALL NOT
    // modify any existing user's pairing state, session history, or heartbeat schedule.
    #[test]
    fn adding_user_does_not_modify_existing_state(
        existing_users in arb_unique_users(1, 5),
        new_user in arb_paired_user(),
    ) {
        let mgr = MultiUserManager::with_defaults();

        // Add existing users
        for user in &existing_users {
            let _ = mgr.add_user(user.clone());
        }

        // Add some session history to existing users
        for user in &existing_users {
            if mgr.get_user(user.channel_type, &user.user_id).is_some() {
                let _ = mgr.add_message_to_session(
                    &user.user_id,
                    SessionMessage {
                        role: "user".into(),
                        content: format!("msg from {}", user.user_id),
                        timestamp: Utc::now(),
                        thread_id: None,
                    },
                );
            }
        }

        // Snapshot existing users' session history lengths
        let snapshots_before: Vec<(String, ChannelType, usize)> = existing_users
            .iter()
            .filter_map(|u| {
                mgr.get_session_history(&u.user_id)
                    .map(|h| (u.user_id.clone(), u.channel_type, h.len()))
            })
            .collect();

        // Add the new user (may fail if duplicate — that's fine)
        let _ = mgr.add_user(new_user);

        // Verify all existing users' state is unchanged
        let snapshots_after: Vec<(String, ChannelType, usize)> = existing_users
            .iter()
            .filter_map(|u| {
                mgr.get_session_history(&u.user_id)
                    .map(|h| (u.user_id.clone(), u.channel_type, h.len()))
            })
            .collect();

        prop_assert_eq!(
            snapshots_before.len(),
            snapshots_after.len(),
            "Number of existing user snapshots changed"
        );

        for (before, after) in snapshots_before.iter().zip(snapshots_after.iter()) {
            prop_assert_eq!(
                before, after,
                "User {} state changed after adding new user",
                before.0
            );
        }
    }

    // Feature: phase-2-complete, Property 9: Multi-User Pairing Independence
    // **Validates: Requirements 8.6, 8.7**
    //
    // Removing a user SHALL NOT affect any other user's state. Each user SHALL have
    // an independent session history that contains only their own messages.
    #[test]
    fn removing_user_does_not_affect_others(
        users in arb_unique_users(2, 6),
    ) {
        let mgr = MultiUserManager::with_defaults();

        // Add all users
        let mut added_users = Vec::new();
        for user in &users {
            if mgr.add_user(user.clone()).is_ok() {
                added_users.push(user.clone());
            }
        }

        // Need at least 2 users for this test
        prop_assume!(added_users.len() >= 2);

        // Add session history to all users
        for user in &added_users {
            let _ = mgr.add_message_to_session(
                &user.user_id,
                SessionMessage {
                    role: "user".into(),
                    content: format!("msg from {}", user.user_id),
                    timestamp: Utc::now(),
                    thread_id: None,
                },
            );
        }

        // Pick the first user to remove
        let user_to_remove = &added_users[0];
        let remaining_users = &added_users[1..];

        // Snapshot remaining users' state before removal
        let snapshots_before: Vec<(String, usize)> = remaining_users
            .iter()
            .filter_map(|u| {
                mgr.get_session_history(&u.user_id)
                    .map(|h| (u.user_id.clone(), h.len()))
            })
            .collect();

        // Remove the user
        let _ = mgr.remove_user(&user_to_remove.user_id);

        // Verify remaining users' state is unchanged
        let snapshots_after: Vec<(String, usize)> = remaining_users
            .iter()
            .filter_map(|u| {
                mgr.get_session_history(&u.user_id)
                    .map(|h| (u.user_id.clone(), h.len()))
            })
            .collect();

        prop_assert_eq!(
            snapshots_before.len(),
            snapshots_after.len(),
            "Number of remaining user snapshots changed"
        );

        for (before, after) in snapshots_before.iter().zip(snapshots_after.iter()) {
            prop_assert_eq!(
                before, after,
                "User {} state changed after removing another user",
                before.0
            );
        }

        // Verify removed user is actually gone
        prop_assert!(
            mgr.get_user(user_to_remove.channel_type, &user_to_remove.user_id).is_none(),
            "Removed user should no longer be paired"
        );
    }

    // Feature: phase-2-complete, Property 9: Multi-User Pairing Independence
    // **Validates: Requirements 8.6**
    //
    // Each user SHALL have an independent session history that contains only
    // their own messages.
    #[test]
    fn session_isolation_per_user(
        users in arb_unique_users(2, 5),
    ) {
        let mgr = MultiUserManager::with_defaults();

        // Add all users
        let mut added_users = Vec::new();
        for user in &users {
            if mgr.add_user(user.clone()).is_ok() {
                added_users.push(user.clone());
            }
        }

        prop_assume!(added_users.len() >= 2);

        // Add unique messages to each user's session
        for (i, user) in added_users.iter().enumerate() {
            let msg = format!("unique-message-{}-{}", user.user_id, i);
            let _ = mgr.add_message_to_session(
                &user.user_id,
                SessionMessage {
                    role: "user".into(),
                    content: msg,
                    timestamp: Utc::now(),
                    thread_id: None,
                },
            );
        }

        // Verify each user's session contains only their own messages
        for (i, user) in added_users.iter().enumerate() {
            let history = mgr.get_session_history(&user.user_id).unwrap_or_default();
            let expected_msg = format!("unique-message-{}-{}", user.user_id, i);

            prop_assert_eq!(
                history.len(), 1,
                "User {} should have exactly 1 message, got {}",
                user.user_id, history.len()
            );

            prop_assert_eq!(
                &history[0].content, &expected_msg,
                "User {} session contains wrong message: expected '{}', got '{}'",
                user.user_id, expected_msg, history[0].content
            );
        }
    }

    // Feature: phase-2-complete, Property 10: Agent Routing Correctness
    // **Validates: Requirements 8.5**
    //
    // For any routing configuration mapping groups to agents, and any incoming
    // message with a group context, the Agent Router SHALL select the agent whose
    // routing rule matches the message's group.
    #[test]
    fn router_selects_correct_agent_for_matching_rule(
        rules in arb_group_level_rules(1, 5),
        default_agent in arb_agent_id(),
    ) {
        let router = AgentRouter::new(rules.clone(), &default_agent);

        // For each rule, create a matching context and verify routing
        for rule in &rules {
            let context = ThreadContext {
                group_id: rule.group_id.clone(),
                thread_id: None,
                sender_id: "test-sender".to_string(),
            };

            let routed_agent = router.route(&context);

            prop_assert_eq!(
                routed_agent, &rule.agent_id,
                "Message for group '{}' should route to '{}', got '{}'",
                rule.group_id, rule.agent_id, routed_agent
            );
        }
    }

    // Feature: phase-2-complete, Property 10: Agent Routing Correctness
    // **Validates: Requirements 8.5**
    //
    // Messages without a matching rule SHALL fall through to the default agent.
    #[test]
    fn router_falls_through_to_default_for_unmatched(
        rules in arb_group_level_rules(0, 5),
        default_agent in arb_agent_id(),
        unmatched_group in arb_group_id(),
    ) {
        let configured_groups: std::collections::HashSet<String> =
            rules.iter().map(|r| r.group_id.clone()).collect();

        // Only test if the generated group doesn't match any rule
        prop_assume!(!configured_groups.contains(&unmatched_group));

        let router = AgentRouter::new(rules, &default_agent);

        let context = ThreadContext {
            group_id: unmatched_group.clone(),
            thread_id: None,
            sender_id: "test-sender".to_string(),
        };

        let routed_agent = router.route(&context);

        prop_assert_eq!(
            routed_agent, default_agent.as_str(),
            "Unmatched group '{}' should route to default '{}', got '{}'",
            unmatched_group, default_agent, routed_agent
        );
    }
}
