//! Property-based tests for delegation circular chain prevention.
//!
//! Feature: full-stack-completion, Property 12: Delegation circular chain prevention
//!
//! For any existing set of delegation permissions forming a DAG, attempting to add
//! a delegation permission that would create a cycle (direct or transitive) SHALL
//! be rejected.
//!
//! **Validates: Requirements 9.5**

use adk_gateway::control_panel::delegation::{would_create_cycle, DelegationRule};
use proptest::prelude::*;
use std::collections::{HashSet, VecDeque};

// ── Strategies ─────────────────────────────────────────────────────

/// Generate an agent ID from a small set to increase chance of cycles.
fn arb_agent_id() -> impl Strategy<Value = String> {
    prop::sample::select(vec![
        "agent-a".to_string(),
        "agent-b".to_string(),
        "agent-c".to_string(),
        "agent-d".to_string(),
        "agent-e".to_string(),
        "agent-f".to_string(),
        "agent-g".to_string(),
        "agent-h".to_string(),
    ])
}

/// Generate a valid DAG of delegation rules (no cycles).
/// We build edges one at a time, only adding if they don't create a cycle.
fn arb_dag() -> impl Strategy<Value = Vec<DelegationRule>> {
    prop::collection::vec((arb_agent_id(), arb_agent_id()), 0..15).prop_map(|edges| {
        let mut dag: Vec<DelegationRule> = Vec::new();
        for (caller, target) in edges {
            if caller == target {
                continue;
            }
            // Only add if it doesn't create a cycle
            if !would_create_cycle(&dag, &caller, &target) {
                // Also skip duplicates
                if !dag
                    .iter()
                    .any(|r| r.caller_id == caller && r.target_id == target)
                {
                    dag.push(DelegationRule {
                        caller_id: caller,
                        target_id: target,
                        created_at: String::new(),
                    });
                }
            }
        }
        dag
    })
}

/// Reference implementation of reachability check using BFS.
/// Checks if `from` can reach `to` by following delegation edges.
/// Note: if from == to, this returns true immediately (self-reachability).
/// For cycle detection in a graph, use `has_cycle_through` instead.
fn can_reach(permissions: &[DelegationRule], from: &str, to: &str) -> bool {
    if from == to {
        return true;
    }
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(from.to_string());

    while let Some(node) = queue.pop_front() {
        if !visited.insert(node.clone()) {
            continue;
        }
        for rule in permissions {
            if rule.caller_id == node {
                if rule.target_id == to {
                    return true;
                }
                queue.push_back(rule.target_id.clone());
            }
        }
    }

    false
}

/// Check if there's a cycle through a specific node (i.e., the node can reach
/// itself by following at least one edge).
fn has_cycle_through(permissions: &[DelegationRule], node: &str) -> bool {
    // BFS from node's successors to see if any path leads back to node
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    // Start from node's direct successors
    for rule in permissions {
        if rule.caller_id == node {
            queue.push_back(rule.target_id.clone());
        }
    }

    while let Some(current) = queue.pop_front() {
        if current == node {
            return true;
        }
        if !visited.insert(current.clone()) {
            continue;
        }
        for rule in permissions {
            if rule.caller_id == current {
                queue.push_back(rule.target_id.clone());
            }
        }
    }

    false
}

// ── Property Tests ─────────────────────────────────────────────────

proptest! {
    /// **Validates: Requirements 9.5**
    ///
    /// Property 12: For any existing DAG of permissions, adding a permission
    /// that would create a cycle SHALL be detected by would_create_cycle.
    ///
    /// We verify: if target can reach caller in the existing graph, then
    /// would_create_cycle returns true.
    #[test]
    fn cycle_detection_rejects_all_cycles(
        dag in arb_dag(),
        caller in arb_agent_id(),
        target in arb_agent_id(),
    ) {
        // Self-delegation is always a cycle
        if caller == target {
            prop_assert!(
                would_create_cycle(&dag, &caller, &target),
                "Self-delegation should always be detected as a cycle"
            );
            return Ok(());
        }

        // Check if adding caller→target would create a cycle:
        // A cycle exists iff target can reach caller in the existing graph
        // (following edges, not counting self-reachability).
        let creates_cycle = can_reach(&dag, &target, &caller);
        let detected = would_create_cycle(&dag, &caller, &target);

        prop_assert_eq!(
            detected,
            creates_cycle,
            "Cycle detection mismatch for {} → {} with DAG {:?}",
            caller,
            target,
            dag.iter().map(|r| format!("{}→{}", r.caller_id, r.target_id)).collect::<Vec<_>>()
        );
    }

    /// **Validates: Requirements 9.5**
    ///
    /// Property 12 (invariant): A valid DAG never has cycles. After constructing
    /// a DAG using our cycle detection, no node should be able to reach itself
    /// through edges.
    #[test]
    fn dag_construction_produces_no_cycles(
        dag in arb_dag(),
    ) {
        // Collect all unique agent IDs
        let mut agents: HashSet<String> = HashSet::new();
        for rule in &dag {
            agents.insert(rule.caller_id.clone());
            agents.insert(rule.target_id.clone());
        }

        // For every agent, verify it cannot reach itself through edges
        for agent in &agents {
            prop_assert!(
                !has_cycle_through(&dag, agent),
                "DAG should not contain cycles, but {} can reach itself. DAG: {:?}",
                agent,
                dag.iter().map(|r| format!("{}→{}", r.caller_id, r.target_id)).collect::<Vec<_>>()
            );
        }
    }

    /// **Validates: Requirements 9.5**
    ///
    /// Property 12 (completeness): If would_create_cycle returns false,
    /// then adding the edge should not create any cycle in the resulting graph.
    #[test]
    fn no_false_negatives_in_cycle_detection(
        dag in arb_dag(),
        caller in arb_agent_id(),
        target in arb_agent_id(),
    ) {
        if caller == target {
            return Ok(());
        }

        if !would_create_cycle(&dag, &caller, &target) {
            // Adding this edge should not create a cycle
            let mut extended = dag.clone();
            extended.push(DelegationRule {
                caller_id: caller.clone(),
                target_id: target.clone(),
                created_at: String::new(),
            });

            // Verify no node can reach itself through edges in the extended graph
            let mut agents: HashSet<String> = HashSet::new();
            for rule in &extended {
                agents.insert(rule.caller_id.clone());
                agents.insert(rule.target_id.clone());
            }

            for agent in &agents {
                prop_assert!(
                    !has_cycle_through(&extended, agent),
                    "After adding {} → {} (which passed cycle check), \
                     agent {} can reach itself. Extended graph: {:?}",
                    caller,
                    target,
                    agent,
                    extended.iter().map(|r| format!("{}→{}", r.caller_id, r.target_id)).collect::<Vec<_>>()
                );
            }
        }
    }
}
