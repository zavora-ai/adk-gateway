//! Property-based tests for graph workflows and action nodes.
//!
//! Feature: gateway-production-maturity
//! - Property 29: Graph workflow builds from valid config
//!   **Validates: Requirements R16.1, R16.2, R16.3, R16.6, R16.8**
//! - Property 30: Graph workflow checkpointing persists state
//!   **Validates: Requirements R16.4**
//! - Property 31: Action nodes execute without LLM
//!   **Validates: Requirements R18.1, R18.8**
//! - Property 32: HTTP action node includes auth credentials
//!   **Validates: Requirements R18.3**
//! - Property 33: Switch action routes to correct branch
//!   **Validates: Requirements R18.6**

use adk_core::Agent;
use adk_gateway::action_executor::ActionExecutor;
use adk_gateway::config::*;
use adk_gateway::graph_workflow::{GraphWorkflowBuilder, WorkflowExecutionContext, WorkflowState};
use adk_session::SessionService;
use dashmap::DashMap;
use proptest::prelude::*;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Helper to create a WorkflowExecutionContext for property tests.
fn make_test_ctx(executor: &ActionExecutor) -> WorkflowExecutionContext<'_> {
    let agents: &DashMap<String, Arc<dyn Agent>> = Box::leak(Box::new(DashMap::new()));
    let session_service: &Arc<dyn SessionService> = Box::leak(Box::new(Arc::new(
        adk_session::InMemorySessionService::new(),
    )
        as Arc<dyn SessionService>));
    WorkflowExecutionContext {
        action_executor: executor,
        agents,
        session_service,
    }
}

/// Strategy for generating valid node IDs (unique, non-empty).
fn node_id_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{1,15}".prop_map(|s| s)
}

/// Strategy for generating a valid GraphWorkflowConfig with 2-5 nodes in a linear chain.
fn valid_workflow_config_strategy() -> impl Strategy<Value = GraphWorkflowConfig> {
    (2..6usize).prop_flat_map(|node_count| {
        proptest::collection::vec(node_id_strategy(), node_count)
            .prop_filter("unique node ids", |ids| {
                let set: std::collections::HashSet<_> = ids.iter().collect();
                set.len() == ids.len()
            })
            .prop_flat_map(move |ids| {
                let node_count = ids.len();
                let ids_clone = ids.clone();
                // Generate optional max_iterations
                let max_iter = proptest::option::of(2..200u32);
                // Generate optional stream mode
                let stream_mode = proptest::option::of(prop_oneof![
                    Just(GraphStreamMode::Values),
                    Just(GraphStreamMode::Updates),
                    Just(GraphStreamMode::Messages),
                    Just(GraphStreamMode::Debug),
                ]);
                // Generate optional reducer keys
                let reducer_count = 0..3usize;

                (max_iter, stream_mode, reducer_count).prop_map(
                    move |(max_iterations, stream_mode, reducer_count)| {
                        let nodes: Vec<GraphNodeConfig> = ids_clone
                            .iter()
                            .map(|id| GraphNodeConfig {
                                id: id.clone(),
                                node_type: GraphNodeType::Agent,
                                config: serde_json::json!({"agent_id": id}),
                            })
                            .collect();

                        // Linear chain edges
                        let edges: Vec<GraphEdgeConfig> = (0..node_count - 1)
                            .map(|i| GraphEdgeConfig {
                                from: ids_clone[i].clone(),
                                to: ids_clone[i + 1].clone(),
                                condition: None,
                            })
                            .collect();

                        let mut state_reducers = HashMap::new();
                        for i in 0..reducer_count {
                            let key = format!("key_{}", i);
                            let reducer = match i % 3 {
                                0 => ReducerType::Overwrite,
                                1 => ReducerType::Append,
                                _ => ReducerType::Sum,
                            };
                            state_reducers.insert(key, reducer);
                        }

                        GraphWorkflowConfig {
                            nodes,
                            edges,
                            state_reducers,
                            checkpoint: None,
                            stream_mode,
                            max_iterations,
                            interrupts: None,
                        }
                    },
                )
            })
    })
}

// ── Property 29: Graph workflow builds from valid config ────────────
// **Validates: Requirements 16.1, 16.2, 16.3, 16.6, 16.8**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn graph_workflow_builds_from_valid_config(
        config in valid_workflow_config_strategy()
    ) {
        let result = GraphWorkflowBuilder::build(&config);
        prop_assert!(result.is_ok(), "valid config should build successfully");

        let wf = result.unwrap();

        // Node count matches
        prop_assert_eq!(wf.nodes.len(), config.nodes.len());

        // Edge count matches
        prop_assert_eq!(wf.edges.len(), config.edges.len());

        // State reducers match
        prop_assert_eq!(wf.state_reducers.len(), config.state_reducers.len());
        for (key, _reducer) in &config.state_reducers {
            prop_assert!(
                wf.state_reducers.contains_key(key),
                "reducer key '{}' should be present", key
            );
        }

        // Max iterations: uses config value or default 100
        let expected_max = config.max_iterations.unwrap_or(100);
        prop_assert_eq!(wf.max_iterations, expected_max);

        // Stream mode matches
        match (&config.stream_mode, &wf.stream_mode) {
            (Some(_), Some(_)) => {}
            (None, None) => {}
            _ => prop_assert!(false, "stream mode mismatch"),
        }

        // All node IDs are preserved
        for node in &config.nodes {
            prop_assert!(
                wf.find_node(&node.id).is_some(),
                "node '{}' should be findable", node.id
            );
        }
    }
}

// ── Property 30: Graph workflow checkpointing persists state ───────
// **Validates: Requirements 16.4**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(80))]

    #[test]
    fn graph_workflow_checkpointing_persists_state(
        config in valid_workflow_config_strategy()
    ) {
        // Enable checkpointing and ensure max_iterations is large enough
        let mut config = config;
        config.checkpoint = Some(CheckpointConfig {
            backend: "memory".into(),
            path: None,
        });
        // Ensure max_iterations >= node count so linear chain completes
        let min_iters = config.nodes.len() as u32;
        config.max_iterations = Some(config.max_iterations.unwrap_or(100).max(min_iters));

        let wf = GraphWorkflowBuilder::build(&config).unwrap();
        let executor = ActionExecutor::new();
        let ctx = make_test_ctx(&executor);
        let result = wf.execute(WorkflowState::new(), &ctx).unwrap();

        // Number of checkpoints should equal number of executed nodes
        let snapshot_count = result.checkpoint_store.snapshots().len();
        let executed_count = result.executed_nodes.len();
        prop_assert_eq!(
            snapshot_count, executed_count,
            "checkpoint count should match executed node count"
        );

        // Last checkpoint should have the final state
        if let Some(last_snapshot) = result.checkpoint_store.restore() {
            prop_assert_eq!(
                &last_snapshot.state, &result.state,
                "last checkpoint state should match final state"
            );
        }

        // Each checkpoint should reference a valid node
        for snapshot in result.checkpoint_store.snapshots() {
            prop_assert!(
                wf.find_node(&snapshot.node_id).is_some(),
                "checkpoint node '{}' should exist in workflow", snapshot.node_id
            );
        }
    }
}

/// Strategy for generating action types.
fn action_type_strategy() -> impl Strategy<Value = ActionType> {
    prop_oneof![
        Just(ActionType::Http),
        Just(ActionType::Database),
        Just(ActionType::File),
        Just(ActionType::Transform),
        Just(ActionType::Set),
        Just(ActionType::Switch),
        Just(ActionType::Loop),
        Just(ActionType::Merge),
        Just(ActionType::Wait),
        Just(ActionType::Code),
        Just(ActionType::Email),
        Just(ActionType::Notification),
        Just(ActionType::Rss),
        Just(ActionType::Trigger),
    ]
}

// ── Property 31: Action nodes execute without LLM ──────────────────
// **Validates: Requirements 18.1, 18.8**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn action_nodes_execute_without_llm(
        action_type in action_type_strategy()
    ) {
        let executor = ActionExecutor::new();
        let config = ActionNodeConfig {
            action_type,
            params: serde_json::json!({}),
        };

        let result = executor.execute(&config);
        prop_assert!(result.is_ok(), "action should execute successfully");

        let output = result.unwrap();
        prop_assert!(
            output.get("action").is_some(),
            "output should contain 'action' field"
        );
        prop_assert!(
            !output.is_null(),
            "output should not be null"
        );
    }
}

/// Strategy for generating HTTP auth types.
fn http_auth_strategy() -> impl Strategy<Value = (String, Value)> {
    prop_oneof![
        Just((
            "bearer".to_string(),
            serde_json::json!({"type": "bearer", "token": "tok123"})
        )),
        Just((
            "basic".to_string(),
            serde_json::json!({"type": "basic", "username": "user", "password": "pass"})
        )),
        Just((
            "api_key".to_string(),
            serde_json::json!({"type": "api_key", "key": "X-API-Key", "value": "key123"})
        )),
    ]
}

// ── Property 32: HTTP action node includes auth credentials ────────
// **Validates: Requirements 18.3**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn http_action_includes_auth_credentials(
        (auth_type, auth_config) in http_auth_strategy(),
        method in prop_oneof![
            Just("GET"), Just("POST"), Just("PUT"), Just("PATCH"), Just("DELETE")
        ],
        url in "[a-z]{3,10}\\.[a-z]{2,5}/[a-z]{1,10}"
    ) {
        let executor = ActionExecutor::new();
        let config = ActionNodeConfig {
            action_type: ActionType::Http,
            params: serde_json::json!({
                "method": method,
                "url": format!("https://{}", url),
                "auth": auth_config,
            }),
        };

        let result = executor.execute(&config).unwrap();

        // Auth should be included in the result
        prop_assert_eq!(
            result["auth_included"].as_bool(),
            Some(true),
            "auth_included should be true"
        );
        prop_assert_eq!(
            result["auth_type"].as_str(),
            Some(auth_type.as_str()),
            "auth_type should match configured type"
        );
    }

    #[test]
    fn http_action_without_auth_has_no_auth_metadata(
        method in prop_oneof![Just("GET"), Just("POST")],
    ) {
        let executor = ActionExecutor::new();
        let config = ActionNodeConfig {
            action_type: ActionType::Http,
            params: serde_json::json!({
                "method": method,
                "url": "https://example.com",
            }),
        };

        let result = executor.execute(&config).unwrap();
        prop_assert!(
            result.get("auth_included").is_none(),
            "should not have auth_included when no auth configured"
        );
    }
}

// ── Property 33: Switch action routes to correct branch ────────────
// **Validates: Requirements 18.6**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn switch_action_routes_to_matching_branch(
        status_value in "[a-z]{3,10}",
        branch_name in "[a-z_]{3,15}",
    ) {
        let executor = ActionExecutor::new();
        let config = ActionNodeConfig {
            action_type: ActionType::Switch,
            params: serde_json::json!({
                "conditions": [
                    {
                        "expression": format!("status == \"{}\"", status_value),
                        "branch": branch_name,
                    }
                ],
                "default": "default_branch",
                "state": {"status": status_value},
            }),
        };

        let result = executor.execute(&config).unwrap();
        prop_assert_eq!(
            result["matched_branch"].as_str(),
            Some(branch_name.as_str()),
            "should route to the matching branch"
        );
    }

    #[test]
    fn switch_action_routes_to_default_when_no_match(
        status_value in "[a-z]{3,10}",
        condition_value in "[a-z]{3,10}",
        default_branch in "[a-z_]{3,15}",
    ) {
        prop_assume!(status_value != condition_value);

        let executor = ActionExecutor::new();
        let config = ActionNodeConfig {
            action_type: ActionType::Switch,
            params: serde_json::json!({
                "conditions": [
                    {
                        "expression": format!("status == \"{}\"", condition_value),
                        "branch": "wrong_branch",
                    }
                ],
                "default": default_branch,
                "state": {"status": status_value},
            }),
        };

        let result = executor.execute(&config).unwrap();
        prop_assert_eq!(
            result["matched_branch"].as_str(),
            Some(default_branch.as_str()),
            "should route to default branch when no condition matches"
        );
    }
}
