//! Property-based tests for AgentConfig types.
//!
//! Feature: multi-agent-isolation, Property 6: Config round-trip
//! **Validates: Requirements 9.6**

use adk_gateway::agent_config::*;
use proptest::prelude::*;

// ── Leaf strategies ────────────────────────────────────────────────

fn arb_non_empty_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,20}"
}

fn arb_opt_string() -> impl Strategy<Value = Option<String>> {
    prop::option::of(arb_non_empty_string())
}

fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        any::<i32>().prop_map(|n| serde_json::Value::Number(n.into())),
        arb_non_empty_string().prop_map(serde_json::Value::String),
    ]
}

// ── Enum strategies ────────────────────────────────────────────────

fn arb_agent_type() -> impl Strategy<Value = AgentType> {
    prop_oneof![
        Just(AgentType::Llm),
        Just(AgentType::Sequential),
        Just(AgentType::Parallel),
        Just(AgentType::Loop),
        Just(AgentType::Router),
        Just(AgentType::Graph),
    ]
}

// ── Struct strategies ──────────────────────────────────────────────

fn arb_agent_role_config() -> impl Strategy<Value = AgentRoleConfig> {
    (
        prop::collection::vec(arb_non_empty_string(), 0..3),
        prop::collection::vec(arb_non_empty_string(), 0..3),
    )
        .prop_map(|(allow, deny)| AgentRoleConfig { allow, deny })
}

fn arb_channel_binding() -> impl Strategy<Value = ChannelBinding> {
    (arb_non_empty_string(), arb_opt_string(), arb_opt_string()).prop_map(
        |(channel_type, account_id, peer_filter)| ChannelBinding {
            channel_type,
            account_id,
            peer_filter,
        },
    )
}

fn arb_action_node_entry() -> impl Strategy<Value = ActionNodeEntry> {
    (arb_non_empty_string(), arb_json_value())
        .prop_map(|(id, config)| ActionNodeEntry { id, config })
}

fn arb_workflow_edge() -> impl Strategy<Value = WorkflowEdge> {
    (
        arb_non_empty_string(),
        arb_non_empty_string(),
        arb_opt_string(),
    )
        .prop_map(|(from, to, condition)| WorkflowEdge {
            from,
            to,
            condition,
        })
}

/// Use finite f32 values only to avoid NaN != NaN issues in round-trip.
fn arb_finite_f32() -> impl Strategy<Value = f32> {
    -1e6f32..1e6f32
}

fn arb_agent_config() -> impl Strategy<Value = AgentConfig> {
    let group1 = (
        arb_non_empty_string(),
        arb_non_empty_string(),
        arb_non_empty_string(),
        arb_agent_type(),
        arb_non_empty_string(),
        arb_non_empty_string(),
        arb_non_empty_string(),
        prop::collection::vec(arb_non_empty_string(), 0..3),
        prop::collection::vec(arb_action_node_entry(), 0..3),
        prop::collection::vec(arb_workflow_edge(), 0..3),
    );
    let group2 = (
        prop::collection::vec(arb_non_empty_string(), 0..3),
        arb_agent_role_config(),
        prop::collection::vec(arb_channel_binding(), 0..3),
        any::<bool>(),
        prop::option::of(arb_finite_f32()),
        prop::option::of(any::<i32>()),
    );
    (group1, group2).prop_map(
        |(
            (
                id,
                name,
                description,
                agent_type,
                model,
                api_key_env,
                instruction,
                tools,
                action_nodes,
                workflow_edges,
            ),
            (sub_agents, role, channel_bindings, auto_start, temperature, max_output_tokens),
        )| {
            AgentConfig {
                id,
                name,
                description,
                agent_type,
                model,
                api_key_env,
                instruction,
                tools,
                action_nodes,
                workflow_edges,
                sub_agents,
                role,
                channel_bindings,
                auto_start,
                temperature,
                max_output_tokens,
                model_override: None,
            }
        },
    )
}

// ── Property test ──────────────────────────────────────────────────

// Feature: multi-agent-isolation, Property 6: Config round-trip
// **Validates: Requirements 9.6**
proptest! {
    #[test]
    fn agent_config_json_round_trip(config in arb_agent_config()) {
        let json = serde_json::to_string(&config).expect("serialization should succeed");
        let parsed: AgentConfig = serde_json::from_str(&json).expect("deserialization should succeed");
        prop_assert_eq!(config, parsed);
    }
}
