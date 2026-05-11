//! Property-based tests for AgentRegistry.
//!
//! Feature: multi-agent-isolation
//! Properties 1 (ID uniqueness), 2 (Lifecycle validity), 3 (System agent singleton)

use adk_gateway::agent_config::*;
use adk_gateway::agent_registry::*;
use proptest::prelude::*;
use tempfile::TempDir;

// ── Helpers ────────────────────────────────────────────────────────

fn make_config(id: &str) -> AgentConfig {
    AgentConfig {
        id: id.to_string(),
        name: format!("Agent {}", id),
        description: "test".to_string(),
        agent_type: AgentType::Llm,
        model: "test/model".to_string(),
        api_key_env: "KEY".to_string(),
        instruction: "hi".to_string(),
        tools: vec![],
        action_nodes: vec![],
        workflow_edges: vec![],
        sub_agents: vec![],
        role: AgentRoleConfig {
            allow: vec![],
            deny: vec![],
        },
        channel_bindings: vec![],
        auto_start: false,
        temperature: None,
        max_output_tokens: None,
        model_override: None,
    }
}

fn arb_kebab_id() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{0,9}"
}

// ── Strategies for lifecycle states ────────────────────────────────

fn arb_lifecycle_state() -> impl Strategy<Value = LifecycleState> {
    prop_oneof![
        Just(LifecycleState::Created),
        Just(LifecycleState::Starting),
        Just(LifecycleState::Running),
        Just(LifecycleState::Stopping),
        Just(LifecycleState::Stopped),
        "[a-z]{1,10}".prop_map(|msg| LifecycleState::Error { message: msg }),
    ]
}

// ── Property 1: Agent ID uniqueness ────────────────────────────────
// **Validates: Requirements 2.2, 2.3**
//
// Generate a list of agent IDs (with potential duplicates), create them all,
// and verify no two agents share the same ID in the registry.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    #[test]
    fn agent_id_uniqueness(ids in prop::collection::vec(arb_kebab_id(), 1..20)) {
        let tmp = TempDir::new().unwrap();
        let reg = AgentRegistry::new(tmp.path().join("reg"));

        let mut created = std::collections::HashSet::new();
        for id in &ids {
            let result = reg.create_agent(make_config(id));
            if created.contains(id) {
                // Duplicate — must fail.
                prop_assert!(result.is_err(), "duplicate id '{}' should be rejected", id);
            } else {
                // First occurrence — must succeed.
                prop_assert!(result.is_ok(), "first create of '{}' should succeed: {:?}", id, result);
                created.insert(id.clone());
            }
        }

        // Verify registry has exactly the unique IDs.
        let listed: std::collections::HashSet<String> =
            reg.list().into_iter().map(|(k, _)| k).collect();
        prop_assert_eq!(created, listed);
    }
}

// ── Property 2: Lifecycle state validity ───────────────────────────
// **Validates: Requirements 6.1, 6.2, 6.5, 6.6, 6.7**
//
// Generate random (from_state, to_state) pairs, verify only valid
// transitions succeed and invalid ones are rejected.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn lifecycle_state_validity(
        from in arb_lifecycle_state(),
        to in arb_lifecycle_state(),
    ) {
        let tmp = TempDir::new().unwrap();
        let reg = AgentRegistry::new(tmp.path().join("reg"));

        // Create an agent and force it into the `from` state by walking
        // a valid path to that state.
        reg.create_agent(make_config("test")).unwrap();
        let walk_result = walk_to_state(&reg, "test", &from);

        if walk_result.is_err() {
            // Some states are unreachable from Created without going through
            // intermediate states that we can't skip. That's fine — skip.
            return Ok(());
        }

        let result = reg.transition("test", to.clone());
        let valid = is_valid_transition(&from, &to);

        if valid {
            prop_assert!(
                result.is_ok(),
                "transition {:?} → {:?} should succeed but got {:?}",
                from, to, result
            );
        } else {
            prop_assert!(
                result.is_err(),
                "transition {:?} → {:?} should fail but succeeded",
                from, to
            );
        }
    }
}

/// Walk an agent from Created to the target state via valid transitions.
fn walk_to_state(reg: &AgentRegistry, id: &str, target: &LifecycleState) -> Result<(), ()> {
    // Paths from Created to each reachable state.
    let path: Vec<LifecycleState> = match target {
        LifecycleState::Created => vec![],
        LifecycleState::Starting => vec![LifecycleState::Starting],
        LifecycleState::Running => vec![LifecycleState::Starting, LifecycleState::Running],
        LifecycleState::Stopping => vec![
            LifecycleState::Starting,
            LifecycleState::Running,
            LifecycleState::Stopping,
        ],
        LifecycleState::Stopped => vec![
            LifecycleState::Starting,
            LifecycleState::Running,
            LifecycleState::Stopping,
            LifecycleState::Stopped,
        ],
        LifecycleState::Error { .. } => vec![
            LifecycleState::Starting,
            LifecycleState::Error {
                message: "test".to_string(),
            },
        ],
    };

    for state in path {
        reg.transition(id, state).map_err(|_| ())?;
    }
    Ok(())
}

// ── Property 3: System agent singleton ─────────────────────────────
// **Validates: Requirements 1.3**
//
// Try to register multiple system agents, verify only the first succeeds.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(50))]

    #[test]
    fn system_agent_singleton(ids in prop::collection::vec(arb_kebab_id(), 2..10)) {
        let tmp = TempDir::new().unwrap();
        let reg = AgentRegistry::new(tmp.path().join("reg"));

        let mut first_registered = false;
        for id in &ids {
            let result = reg.register_system_agent(make_config(id));
            if !first_registered {
                prop_assert!(result.is_ok(), "first system agent should succeed: {:?}", result);
                first_registered = true;
            } else {
                prop_assert!(
                    result.is_err(),
                    "subsequent system agent '{}' should be rejected",
                    id
                );
            }
        }

        // Verify only one system agent exists.
        let first_id = &ids[0];
        prop_assert!(reg.is_system_agent(first_id));
        for id in &ids[1..] {
            // Other IDs should not be system agents (they may or may not be in the registry
            // depending on whether they were also created via create_agent, but they shouldn't
            // be the system agent).
            prop_assert!(!reg.is_system_agent(id) || id == first_id);
        }
    }
}
