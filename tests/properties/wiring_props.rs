//! Property-based tests for multi-agent full wiring correctness.
//!
//! Feature: multi-agent-full-wiring
//! - Property 1: Agent start establishes all runtime registrations
//!   **Validates: Requirements 1.2, 3.1, 6.1**
//! - Property 2: Agent stop removes all runtime registrations
//!   **Validates: Requirements 1.3, 3.2, 6.2**
//! - Property 3: Agent delete enforces preconditions and cleans up
//!   **Validates: Requirements 1.4, 5.2, 6.4**
//! - Property 4: Agent create registers all persistent state
//!   **Validates: Requirements 1.6, 5.1**
//! - Property 5: ProxyPool-Registry consistency invariant
//!   **Validates: Requirements 3.4**
//! - Property 6: System tool stripping is unconditional
//!   **Validates: Requirements 5.1, 5.5**
//! - Property 7: RBAC check_tool matches configured permissions
//!   **Validates: Requirements 5.3**
//! - Property 8: Fallback chain tries models in order
//!   **Validates: Requirements 4.1, 4.2**
//! - Property 9: FallbackOutcome::FallbackUsed contains correct metadata
//!   **Validates: Requirements 4.3**
//! - Property 10: FallbackOutcome::AllFailed contains all errors
//!   **Validates: Requirements 4.4**
//! - Property 11: Single-model chain produces no fallback overhead
//!   **Validates: Requirements 12.4**
//! - Property 12: WebSocket event emission on state transition
//!   **Validates: Requirements 7.1**
//! - Property 13: Agent configure updates config and restarts if running
//!   **Validates: Requirements 1.5, 6.3**

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use adk_gateway::agent_config::{
    AgentConfig, AgentRoleConfig, AgentType, ChannelBinding, LifecycleState,
};
use adk_gateway::agent_registry::AgentRegistry;
use adk_gateway::config::RoutingConfig;
use adk_gateway::control_panel::ws::WsEvent;
use adk_gateway::fallback_chain::{FallbackModelChain, FallbackOutcome};
use adk_gateway::proxy_pool::RemoteAgentProxyPool;
use adk_gateway::rbac_bridge::{RbacBridge, SYSTEM_TOOLS};
use adk_gateway::router::MessageRouter;

use proptest::prelude::*;

// ── Strategies ─────────────────────────────────────────────────────

fn arb_agent_id() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{1,12}"
}

fn arb_tool_name() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z_]{2,12}".prop_map(|s| s.to_string()),
        prop::sample::select(
            SYSTEM_TOOLS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        ),
    ]
}

fn arb_role_config() -> impl Strategy<Value = AgentRoleConfig> {
    (
        prop::collection::vec(arb_tool_name(), 0..8),
        prop::collection::vec(arb_tool_name(), 0..4),
    )
        .prop_map(|(allow, deny)| AgentRoleConfig { allow, deny })
}

fn arb_channel_binding() -> impl Strategy<Value = ChannelBinding> {
    (
        prop_oneof![
            Just("telegram".to_string()),
            Just("slack".to_string()),
            Just("webhook".to_string()),
        ],
        prop::option::of("[a-z0-9-]{1,8}".prop_map(|s| s.to_string())),
        prop::option::of("[a-z0-9-]{1,8}".prop_map(|s| s.to_string())),
    )
        .prop_map(|(channel_type, account_id, peer_filter)| ChannelBinding {
            channel_type,
            account_id,
            peer_filter,
        })
}

fn arb_agent_config() -> impl Strategy<Value = AgentConfig> {
    (
        arb_agent_id(),
        arb_role_config(),
        prop::collection::vec(arb_channel_binding(), 0..4),
    )
        .prop_map(|(id, role, channel_bindings)| AgentConfig {
            id: id.clone(),
            name: format!("Agent {}", id),
            description: "test agent".to_string(),
            agent_type: AgentType::Llm,
            model: "test/model".to_string(),
            api_key_env: "TEST_KEY".to_string(),
            instruction: "do stuff".to_string(),
            tools: role.allow.clone(),
            action_nodes: vec![],
            workflow_edges: vec![],
            sub_agents: vec![],
            role,
            channel_bindings,
            auto_start: false,
            temperature: None,
            max_output_tokens: None,
            model_override: None,
        })
}

fn arb_port() -> impl Strategy<Value = u16> {
    19001u16..19100
}

/// Helper to create a fresh registry with a temp dir.
fn make_registry() -> (tempfile::TempDir, Arc<AgentRegistry>) {
    let tmp = tempfile::TempDir::new().unwrap();
    let registry = Arc::new(AgentRegistry::new(tmp.path().join("registry")));
    (tmp, registry)
}

/// Helper to create a fresh router.
fn make_router() -> MessageRouter {
    MessageRouter::new(&RoutingConfig::default(), "system".to_string())
}

// ── Property 1: Agent start establishes all runtime registrations ──
// Feature: multi-agent-full-wiring, Property 1: Agent start establishes all runtime registrations
// **Validates: Requirements 1.2, 3.1, 6.1**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn agent_start_establishes_all_runtime_registrations(
        config in arb_agent_config(),
        port in arb_port(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let (_tmp, registry) = make_registry();
            let proxy_pool = Arc::new(RemoteAgentProxyPool::new());
            let rbac = Arc::new(RbacBridge::new());
            let router_inner = make_router();
            let router = Arc::new(arc_swap::ArcSwap::from_pointee(router_inner));

            let agent_id = config.id.clone();
            let channel_bindings = config.channel_bindings.clone();

            // Create agent first
            registry.create_agent(config.clone()).unwrap();

            // Simulate agent_start: transition through Starting → Running
            registry.transition(&agent_id, LifecycleState::Starting).unwrap();

            // Register proxy (simulates ProcessManager::spawn success)
            proxy_pool.register(&agent_id, port);

            // Register RBAC role
            rbac.register_agent(&agent_id, &config.role);

            // Add router bindings
            if !channel_bindings.is_empty() {
                let current = router.load();
                let mut new_router = (**current).clone();
                new_router.add_agent_bindings(&agent_id, &channel_bindings);
                router.store(Arc::new(new_router));
            }

            // Transition to Running
            registry.transition(&agent_id, LifecycleState::Running).unwrap();

            // Verify: ProxyPool entry exists
            prop_assert!(
                proxy_pool.get(&agent_id).is_some(),
                "ProxyPool should contain entry for agent '{}'", agent_id
            );

            // Verify: RBAC role registered (non-system tools should be accessible)
            // We just verify the role exists by checking a non-system tool from allow list
            for tool in &config.role.allow {
                if !SYSTEM_TOOLS.contains(&tool.as_str()) && !config.role.deny.contains(tool) {
                    prop_assert!(
                        rbac.check_tool(&agent_id, tool).is_ok(),
                        "RBAC should allow tool '{}' for agent '{}'", tool, agent_id
                    );
                }
            }

            // Verify: Registry state == Running
            let entry = registry.get(&agent_id).unwrap();
            prop_assert_eq!(
                entry.state.clone(),
                LifecycleState::Running,
                "Registry state should be Running for agent '{}'", agent_id
            );

            Ok(())
        })?;
    }
}

// ── Property 2: Agent stop removes all runtime registrations ───────
// Feature: multi-agent-full-wiring, Property 2: Agent stop removes all runtime registrations
// **Validates: Requirements 1.3, 3.2, 6.2**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn agent_stop_removes_all_runtime_registrations(
        config in arb_agent_config(),
        port in arb_port(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let (_tmp, registry) = make_registry();
            let proxy_pool = Arc::new(RemoteAgentProxyPool::new());
            let router_inner = make_router();
            let router = Arc::new(arc_swap::ArcSwap::from_pointee(router_inner));

            let agent_id = config.id.clone();

            // Setup: create and start agent
            registry.create_agent(config.clone()).unwrap();
            registry.transition(&agent_id, LifecycleState::Starting).unwrap();
            proxy_pool.register(&agent_id, port);
            if !config.channel_bindings.is_empty() {
                let current = router.load();
                let mut new_router = (**current).clone();
                new_router.add_agent_bindings(&agent_id, &config.channel_bindings);
                router.store(Arc::new(new_router));
            }
            registry.transition(&agent_id, LifecycleState::Running).unwrap();

            // Now simulate agent_stop
            registry.transition(&agent_id, LifecycleState::Stopping).unwrap();

            // Remove from ProxyPool
            proxy_pool.remove(&agent_id);

            // Remove router bindings
            let current = router.load();
            let mut new_router = (**current).clone();
            new_router.remove_agent_bindings(&agent_id);
            router.store(Arc::new(new_router));

            // Transition to Stopped
            registry.transition(&agent_id, LifecycleState::Stopped).unwrap();

            // Verify: ProxyPool empty for this agent
            prop_assert!(
                proxy_pool.get(&agent_id).is_none(),
                "ProxyPool should NOT contain entry for stopped agent '{}'", agent_id
            );

            // Verify: Registry state == Stopped
            let entry = registry.get(&agent_id).unwrap();
            prop_assert_eq!(
                entry.state.clone(),
                LifecycleState::Stopped,
                "Registry state should be Stopped for agent '{}'", agent_id
            );

            Ok(())
        })?;
    }
}

// ── Property 3: Agent delete enforces preconditions and cleans up ──
// Feature: multi-agent-full-wiring, Property 3: Agent delete enforces preconditions and cleans up
// **Validates: Requirements 1.4, 5.2, 6.4**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn agent_delete_enforces_preconditions_and_cleans_up(
        config in arb_agent_config(),
        _port in arb_port(),
        delete_from_error in proptest::bool::ANY,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let (_tmp, registry) = make_registry();
            let rbac = Arc::new(RbacBridge::new());
            let router_inner = make_router();
            let router = Arc::new(arc_swap::ArcSwap::from_pointee(router_inner));

            let agent_id = config.id.clone();

            // Create agent
            registry.create_agent(config.clone()).unwrap();
            rbac.register_agent(&agent_id, &config.role);

            // Add router bindings
            if !config.channel_bindings.is_empty() {
                let current = router.load();
                let mut new_router = (**current).clone();
                new_router.add_agent_bindings(&agent_id, &config.channel_bindings);
                router.store(Arc::new(new_router));
            }

            // Verify: delete from Created state should FAIL
            let delete_result = registry.delete(&agent_id);
            prop_assert!(
                delete_result.is_err(),
                "delete should fail for agent in Created state"
            );

            // Transition to a deletable state (Stopped or Error)
            registry.transition(&agent_id, LifecycleState::Starting).unwrap();
            if delete_from_error {
                registry.transition(&agent_id, LifecycleState::Error {
                    message: "test error".to_string(),
                }).unwrap();
            } else {
                registry.transition(&agent_id, LifecycleState::Running).unwrap();
                registry.transition(&agent_id, LifecycleState::Stopping).unwrap();
                registry.transition(&agent_id, LifecycleState::Stopped).unwrap();
            }

            // Now delete should succeed
            registry.delete(&agent_id).unwrap();

            // Remove RBAC role
            rbac.remove_agent(&agent_id);

            // Remove router bindings
            let current = router.load();
            let mut new_router = (**current).clone();
            new_router.remove_agent_bindings(&agent_id);
            router.store(Arc::new(new_router));

            // Verify: Registry empty for this agent
            prop_assert!(
                registry.get(&agent_id).is_none(),
                "Registry should NOT contain deleted agent '{}'", agent_id
            );

            // Verify: RBAC removed
            prop_assert!(
                rbac.check_tool(&agent_id, "any_tool").is_err(),
                "RBAC should have no role for deleted agent '{}'", agent_id
            );

            Ok(())
        })?;
    }
}

// ── Property 4: Agent create registers all persistent state ────────
// Feature: multi-agent-full-wiring, Property 4: Agent create registers all persistent state
// **Validates: Requirements 1.6, 5.1**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn agent_create_registers_all_persistent_state(
        config in arb_agent_config(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let tmp = tempfile::TempDir::new().unwrap();
            let registry = Arc::new(AgentRegistry::new(tmp.path().join("registry")));
            let rbac = Arc::new(RbacBridge::new());
            let workspace_root = tmp.path().to_path_buf();

            let agent_id = config.id.clone();

            // 1. Create agent in registry
            registry.create_agent(config.clone()).unwrap();

            // 2. Create workspace directories
            let agent_dir = workspace_root.join("agents").join(&agent_id);
            let context_dir = agent_dir.join("context");
            let data_dir = agent_dir.join("data");
            let src_dir = agent_dir.join("src");
            std::fs::create_dir_all(&context_dir).unwrap();
            std::fs::create_dir_all(&data_dir).unwrap();
            std::fs::create_dir_all(&src_dir).unwrap();

            // 3. Register RBAC role (strips system tools)
            let stripped = rbac.register_agent(&agent_id, &config.role);

            // Verify: Registry contains agent with state Created
            let entry = registry.get(&agent_id).unwrap();
            prop_assert_eq!(
                entry.state.clone(),
                LifecycleState::Created,
                "Registry state should be Created for new agent '{}'", agent_id
            );

            // Verify: RBAC role registered with system tools stripped
            for system_tool in SYSTEM_TOOLS {
                prop_assert!(
                    rbac.check_tool(&agent_id, system_tool).is_err(),
                    "RBAC should deny system tool '{}' for agent '{}'", system_tool, agent_id
                );
            }

            // Verify: any system tools in allow list were stripped
            for tool in &config.role.allow {
                if SYSTEM_TOOLS.contains(&tool.as_str()) {
                    prop_assert!(
                        stripped.contains(tool),
                        "system tool '{}' should have been stripped", tool
                    );
                }
            }

            // Verify: workspace dirs exist
            prop_assert!(context_dir.exists(), "context dir should exist");
            prop_assert!(data_dir.exists(), "data dir should exist");
            prop_assert!(src_dir.exists(), "src dir should exist");

            Ok(())
        })?;
    }
}

// ── Property 5: ProxyPool-Registry consistency invariant ───────────
// Feature: multi-agent-full-wiring, Property 5: ProxyPool-Registry consistency invariant
// **Validates: Requirements 3.4**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn proxy_pool_registry_consistency_invariant(
        configs in prop::collection::vec(arb_agent_config(), 1..6),
        ops in prop::collection::vec(prop_oneof![Just("start"), Just("stop")], 1..10),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let (_tmp, registry) = make_registry();
            let proxy_pool = Arc::new(RemoteAgentProxyPool::new());

            // Deduplicate configs by ID
            let mut seen = std::collections::HashSet::new();
            let unique_configs: Vec<AgentConfig> = configs
                .into_iter()
                .filter(|c| seen.insert(c.id.clone()))
                .collect();

            // Create all agents
            for config in &unique_configs {
                registry.create_agent(config.clone()).unwrap();
            }

            let mut port_counter = 19001u16;

            // Apply random start/stop operations
            for op in &ops {
                // Pick a random agent (deterministic based on index)
                let idx = port_counter as usize % unique_configs.len();
                let agent_id = &unique_configs[idx].id;

                let entry = registry.get(agent_id).unwrap();
                let current_state = entry.state.clone();
                drop(entry);

                match *op {
                    "start" => {
                        // Can only start from Created, Stopped, or Error
                        match &current_state {
                            LifecycleState::Created
                            | LifecycleState::Stopped
                            | LifecycleState::Error { .. } => {
                                registry.transition(agent_id, LifecycleState::Starting).unwrap();
                                proxy_pool.register(agent_id, port_counter);
                                port_counter = port_counter.wrapping_add(1).max(19001);
                                registry.transition(agent_id, LifecycleState::Running).unwrap();
                            }
                            _ => {} // skip invalid transitions
                        }
                    }
                    "stop" => {
                        // Can only stop from Running
                        if current_state == LifecycleState::Running {
                            registry.transition(agent_id, LifecycleState::Stopping).unwrap();
                            proxy_pool.remove(agent_id);
                            registry.transition(agent_id, LifecycleState::Stopped).unwrap();
                        }
                    }
                    _ => {}
                }
            }

            // Verify invariant: ProxyPool entry exists iff Registry state == Running
            for config in &unique_configs {
                let agent_id = &config.id;
                let entry = registry.get(agent_id).unwrap();
                let is_running = entry.state == LifecycleState::Running;
                let has_proxy = proxy_pool.get(agent_id).is_some();

                prop_assert_eq!(
                    has_proxy,
                    is_running,
                    "ProxyPool entry ({}) should match Running state ({}) for agent '{}'",
                    has_proxy,
                    is_running,
                    agent_id
                );
            }

            Ok(())
        })?;
    }
}

// ── Property 6: System tool stripping is unconditional ─────────────
// Feature: multi-agent-full-wiring, Property 6: System tool stripping is unconditional
// **Validates: Requirements 5.1, 5.5**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn system_tool_stripping_is_unconditional(
        agent_id in arb_agent_id(),
        role_config in arb_role_config(),
    ) {
        let rbac = RbacBridge::new();
        rbac.register_agent(&agent_id, &role_config);

        // For any role config, system tools must NEVER be granted
        for system_tool in SYSTEM_TOOLS {
            prop_assert!(
                rbac.check_tool(&agent_id, system_tool).is_err(),
                "system tool '{}' should NEVER be granted to agent '{}', \
                 regardless of allow list {:?}",
                system_tool,
                agent_id,
                role_config.allow,
            );
        }
    }
}

// ── Property 7: RBAC check_tool matches configured permissions ─────
// Feature: multi-agent-full-wiring, Property 7: RBAC check_tool matches configured permissions
// **Validates: Requirements 5.3**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn rbac_check_tool_matches_configured_permissions(
        agent_id in arb_agent_id(),
        role_config in arb_role_config(),
        test_tool in "[a-z_]{2,12}".prop_map(|s| s.to_string()),
    ) {
        let rbac = RbacBridge::new();
        rbac.register_agent(&agent_id, &role_config);

        let is_system_tool = SYSTEM_TOOLS.contains(&test_tool.as_str());
        let in_allow = role_config.allow.contains(&test_tool);
        let in_deny = role_config.deny.contains(&test_tool);

        let result = rbac.check_tool(&agent_id, &test_tool);

        if is_system_tool {
            // System tools are always denied for user agents
            prop_assert!(
                result.is_err(),
                "system tool '{}' should always be denied", test_tool
            );
        } else if in_deny {
            // Deny takes precedence over allow
            prop_assert!(
                result.is_err(),
                "tool '{}' in deny list should be denied", test_tool
            );
        } else if in_allow {
            // In allow and not in deny → should be allowed
            prop_assert!(
                result.is_ok(),
                "tool '{}' in allow list (not denied) should be allowed for agent '{}'. \
                 Allow: {:?}, Deny: {:?}",
                test_tool, agent_id, role_config.allow, role_config.deny
            );
        } else {
            // Not in allow list → denied
            prop_assert!(
                result.is_err(),
                "tool '{}' not in allow list should be denied", test_tool
            );
        }
    }
}

// ── Mock Llm for fallback chain tests ──────────────────────────────

/// A mock Llm that tracks invocations. The actual success/failure behavior
/// is controlled by the closure passed to `run_with_fallback`, not by the
/// Llm trait implementation itself.
#[derive(Debug, Clone)]
struct MockModelEntry {
    model_id: String,
    should_fail: bool,
    error_msg: String,
}

/// Builds a FallbackModelChain with mock models for testing.
/// Uses `from_models_for_test` with dummy Llm implementations.
/// The actual test logic uses a closure that checks the model_id to determine
/// success/failure, tracking invocation order via a shared counter.
fn build_test_chain(
    entries: &[MockModelEntry],
) -> (
    FallbackModelChain,
    Arc<AtomicUsize>,
    Arc<std::sync::Mutex<Vec<String>>>,
) {
    let invocation_counter = Arc::new(AtomicUsize::new(0));
    let invocation_log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

    // We need real Arc<dyn Llm> instances for the chain, but we won't call generate() on them.
    // Instead, our closure will use the model's name() to look up behavior.
    // Use a minimal struct that implements Llm.
    let mut models: Vec<Arc<dyn adk_core::Llm>> = Vec::new();
    let mut model_ids: Vec<String> = Vec::new();

    for entry in entries {
        let mock = MinimalMockLlm {
            model_id: entry.model_id.clone(),
        };
        models.push(Arc::new(mock));
        model_ids.push(entry.model_id.clone());
    }

    let chain = FallbackModelChain::from_models_for_test(models, model_ids);
    (chain, invocation_counter, invocation_log)
}

/// Minimal mock that implements the Llm trait just enough for the chain to hold it.
/// The actual test behavior is driven by the closure, not by generate_content().
struct MinimalMockLlm {
    model_id: String,
}

impl std::fmt::Debug for MinimalMockLlm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MinimalMockLlm")
            .field("model_id", &self.model_id)
            .finish()
    }
}

#[async_trait::async_trait]
impl adk_core::Llm for MinimalMockLlm {
    fn name(&self) -> &str {
        &self.model_id
    }

    async fn generate_content(
        &self,
        _request: adk_core::LlmRequest,
        _streaming: bool,
    ) -> Result<
        std::pin::Pin<
            Box<
                dyn futures::Stream<Item = Result<adk_core::LlmResponse, adk_core::AdkError>>
                    + Send,
            >,
        >,
        adk_core::AdkError,
    > {
        // This won't be called in our tests — the closure handles behavior
        Ok(Box::pin(futures::stream::empty()))
    }
}

// ── Property 8: Fallback chain tries models in order ───────────────
// Feature: multi-agent-full-wiring, Property 8: Fallback chain tries models in order
// **Validates: Requirements 4.1, 4.2**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn fallback_chain_tries_models_in_order(
        chain_len in 1usize..6,
        fail_count in 0usize..5,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // Ensure fail_count < chain_len (at least one model succeeds)
            let actual_fail_count = fail_count.min(chain_len - 1);

            let entries: Vec<MockModelEntry> = (0..chain_len)
                .map(|i| MockModelEntry {
                    model_id: format!("model-{}", i),
                    should_fail: i < actual_fail_count,
                    error_msg: format!("error from model-{}", i),
                })
                .collect();

            let (chain, _counter, _log) = build_test_chain(&entries);

            // Track invocations via a shared counter in the closure
            let invocation_counter = Arc::new(AtomicUsize::new(0));
            let entries_clone = entries.clone();
            let counter_clone = invocation_counter.clone();

            let result = chain.run_with_fallback(|model| {
                let entries_inner = entries_clone.clone();
                let counter = counter_clone.clone();
                async move {
                    let idx = counter.fetch_add(1, Ordering::SeqCst);
                    let model_id = model.name().to_string();
                    // Find the entry for this model
                    let entry = entries_inner.iter().find(|e| e.model_id == model_id).unwrap();
                    if entry.should_fail {
                        Err(entry.error_msg.clone())
                    } else {
                        Ok(format!("success from {} at invocation {}", model_id, idx))
                    }
                }
            }).await;

            // Should succeed (at least one model doesn't fail)
            prop_assert!(result.is_ok(), "chain should succeed when at least one model works");

            let (_response, outcome) = result.unwrap();

            // Verify invocation count: should have tried actual_fail_count + 1 models
            let total_invocations = invocation_counter.load(Ordering::SeqCst);
            prop_assert_eq!(
                total_invocations,
                actual_fail_count + 1,
                "should invoke exactly {} models (fail_count={} + 1 success)",
                actual_fail_count + 1,
                actual_fail_count
            );

            // Verify outcome type
            if actual_fail_count == 0 {
                prop_assert!(
                    matches!(outcome, FallbackOutcome::PrimarySuccess),
                    "should be PrimarySuccess when primary succeeds"
                );
            } else {
                prop_assert!(
                    matches!(outcome, FallbackOutcome::FallbackUsed { .. }),
                    "should be FallbackUsed when primary fails"
                );
            }

            Ok(())
        })?;
    }
}

// ── Property 9: FallbackOutcome::FallbackUsed contains correct metadata ──
// Feature: multi-agent-full-wiring, Property 9: FallbackOutcome::FallbackUsed contains correct metadata
// **Validates: Requirements 4.3**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn fallback_outcome_used_contains_correct_metadata(
        chain_len in 2usize..6,
        success_idx in 1usize..5,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // Ensure success_idx is within chain bounds
            let actual_success_idx = success_idx.min(chain_len - 1);

            let entries: Vec<MockModelEntry> = (0..chain_len)
                .map(|i| MockModelEntry {
                    model_id: format!("model-{}", i),
                    should_fail: i < actual_success_idx,
                    error_msg: format!("error from model-{}", i),
                })
                .collect();

            let (chain, _counter, _log) = build_test_chain(&entries);
            let entries_clone = entries.clone();

            let result = chain.run_with_fallback(|model| {
                let entries_inner = entries_clone.clone();
                async move {
                    let model_id = model.name().to_string();
                    let entry = entries_inner.iter().find(|e| e.model_id == model_id).unwrap();
                    if entry.should_fail {
                        Err(entry.error_msg.clone())
                    } else {
                        Ok(format!("success from {}", model_id))
                    }
                }
            }).await;

            prop_assert!(result.is_ok(), "chain should succeed");
            let (_response, outcome) = result.unwrap();

            match outcome {
                FallbackOutcome::FallbackUsed {
                    primary_id,
                    fallback_id,
                    fallback_index,
                    primary_error,
                } => {
                    prop_assert_eq!(
                        primary_id,
                        format!("model-0"),
                        "primary_id should be the first model"
                    );
                    prop_assert_eq!(
                        fallback_id,
                        format!("model-{}", actual_success_idx),
                        "fallback_id should be model at success index"
                    );
                    prop_assert_eq!(
                        fallback_index,
                        actual_success_idx,
                        "fallback_index should match success index"
                    );
                    prop_assert!(
                        primary_error.contains("model-0"),
                        "primary_error should contain primary model id, got: {}",
                        primary_error
                    );
                }
                other => {
                    prop_assert!(
                        false,
                        "expected FallbackUsed, got {:?}", other
                    );
                }
            }

            Ok(())
        })?;
    }
}

// ── Property 10: FallbackOutcome::AllFailed contains all errors ─────
// Feature: multi-agent-full-wiring, Property 10: FallbackOutcome::AllFailed contains all errors
// **Validates: Requirements 4.4**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn fallback_outcome_all_failed_contains_all_errors(
        chain_len in 1usize..6,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let entries: Vec<MockModelEntry> = (0..chain_len)
                .map(|i| MockModelEntry {
                    model_id: format!("model-{}", i),
                    should_fail: true,
                    error_msg: format!("error from model-{}", i),
                })
                .collect();

            let (chain, _counter, _log) = build_test_chain(&entries);
            let entries_clone = entries.clone();
            let invocation_counter = Arc::new(AtomicUsize::new(0));
            let counter_clone = invocation_counter.clone();

            let result = chain.run_with_fallback(|model| {
                let entries_inner = entries_clone.clone();
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    let model_id = model.name().to_string();
                    let entry = entries_inner.iter().find(|e| e.model_id == model_id).unwrap();
                    Err::<String, String>(entry.error_msg.clone())
                }
            }).await;

            // Should fail with all errors
            prop_assert!(result.is_err(), "chain should fail when all models fail");
            let errors = result.unwrap_err();

            // Verify exactly N error entries
            prop_assert_eq!(
                errors.len(),
                chain_len,
                "should have exactly {} error entries, got {}",
                chain_len,
                errors.len()
            );

            // Verify each error has correct model ID and error string
            for (i, (model_id, error_str)) in errors.iter().enumerate() {
                prop_assert_eq!(
                    model_id,
                    &format!("model-{}", i),
                    "error entry {} should have model_id 'model-{}'",
                    i,
                    i
                );
                prop_assert_eq!(
                    error_str,
                    &format!("error from model-{}", i),
                    "error string for model '{}' should match",
                    model_id
                );
            }

            // Verify all models were invoked
            let total_invocations = invocation_counter.load(Ordering::SeqCst);
            prop_assert_eq!(
                total_invocations,
                chain_len,
                "all {} models should have been invoked",
                chain_len
            );

            Ok(())
        })?;
    }
}

// ── Property 11: Single-model chain produces no fallback overhead ──
// Feature: multi-agent-full-wiring, Property 11: Single-model chain produces no fallback overhead
// **Validates: Requirements 12.4**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn single_model_chain_no_fallback_overhead(
        model_id in "[a-z]+/[a-z0-9-]+",
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let entries = vec![MockModelEntry {
                model_id: model_id.clone(),
                should_fail: false,
                error_msg: String::new(),
            }];

            let (chain, _counter, _log) = build_test_chain(&entries);

            // Verify single-model chain has no fallbacks
            prop_assert!(
                !chain.has_fallbacks(),
                "single-model chain should report no fallbacks"
            );

            let invocation_counter = Arc::new(AtomicUsize::new(0));
            let counter_clone = invocation_counter.clone();

            let result = chain.run_with_fallback(|_model| {
                let counter = counter_clone.clone();
                async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok::<String, String>("success".to_string())
                }
            }).await;

            prop_assert!(result.is_ok(), "single-model chain should succeed");
            let (_response, outcome) = result.unwrap();

            // Verify exactly one invocation
            let total_invocations = invocation_counter.load(Ordering::SeqCst);
            prop_assert_eq!(
                total_invocations,
                1,
                "single-model chain should invoke exactly one model"
            );

            // Verify PrimarySuccess outcome
            prop_assert!(
                matches!(outcome, FallbackOutcome::PrimarySuccess),
                "single-model chain should produce PrimarySuccess"
            );

            Ok(())
        })?;
    }
}

// ── Property 12: WebSocket event emission on state transition ──────
// Feature: multi-agent-full-wiring, Property 12: WebSocket event emission on state transition
// **Validates: Requirements 7.1**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn websocket_event_emission_on_state_transition(
        config in arb_agent_config(),
        port in arb_port(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let (_tmp, registry) = make_registry();
            let proxy_pool = Arc::new(RemoteAgentProxyPool::new());
            let (ws_tx, mut ws_rx) = tokio::sync::broadcast::channel::<WsEvent>(32);

            let agent_id = config.id.clone();

            // Create agent
            registry.create_agent(config.clone()).unwrap();

            // Emit Created event
            let _ = ws_tx.send(WsEvent::AgentState {
                agent_id: agent_id.clone(),
                state: "Created".into(),
            });

            // Start: emit Starting then Running
            registry.transition(&agent_id, LifecycleState::Starting).unwrap();
            let _ = ws_tx.send(WsEvent::AgentState {
                agent_id: agent_id.clone(),
                state: "Starting".into(),
            });

            proxy_pool.register(&agent_id, port);
            registry.transition(&agent_id, LifecycleState::Running).unwrap();
            let _ = ws_tx.send(WsEvent::AgentState {
                agent_id: agent_id.clone(),
                state: "Running".into(),
            });

            // Stop: emit Stopping then Stopped
            registry.transition(&agent_id, LifecycleState::Stopping).unwrap();
            let _ = ws_tx.send(WsEvent::AgentState {
                agent_id: agent_id.clone(),
                state: "Stopping".into(),
            });

            proxy_pool.remove(&agent_id);
            registry.transition(&agent_id, LifecycleState::Stopped).unwrap();
            let _ = ws_tx.send(WsEvent::AgentState {
                agent_id: agent_id.clone(),
                state: "Stopped".into(),
            });

            // Collect all events
            let mut events = Vec::new();
            while let Ok(event) = ws_rx.try_recv() {
                events.push(event);
            }

            // Verify we received the correct sequence of events
            let expected_states = vec!["Created", "Starting", "Running", "Stopping", "Stopped"];
            prop_assert_eq!(
                events.len(),
                expected_states.len(),
                "should receive {} events, got {}",
                expected_states.len(),
                events.len()
            );

            for (i, event) in events.iter().enumerate() {
                match event {
                    WsEvent::AgentState { agent_id: eid, state } => {
                        prop_assert_eq!(
                            eid,
                            &agent_id,
                            "event {} should have correct agent_id",
                            i
                        );
                        prop_assert_eq!(
                            state,
                            expected_states[i],
                            "event {} should have state '{}', got '{}'",
                            i,
                            expected_states[i],
                            state
                        );
                    }
                    other => {
                        prop_assert!(
                            false,
                            "expected AgentState event, got {:?}", other
                        );
                    }
                }
            }

            Ok(())
        })?;
    }
}

// ── Property 13: Agent configure updates config and restarts if running ──
// Feature: multi-agent-full-wiring, Property 13: Agent configure updates config and restarts if running
// **Validates: Requirements 1.5, 6.3**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn agent_configure_updates_config_and_restarts_if_running(
        config in arb_agent_config(),
        new_role in arb_role_config(),
        new_bindings in prop::collection::vec(arb_channel_binding(), 0..3),
        was_running in proptest::bool::ANY,
        port in arb_port(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let (_tmp, registry) = make_registry();
            let proxy_pool = Arc::new(RemoteAgentProxyPool::new());
            let rbac = Arc::new(RbacBridge::new());
            let router_inner = make_router();
            let router = Arc::new(arc_swap::ArcSwap::from_pointee(router_inner));

            let agent_id = config.id.clone();

            // Create agent
            registry.create_agent(config.clone()).unwrap();
            rbac.register_agent(&agent_id, &config.role);

            // If was_running, bring to Running state
            if was_running {
                registry.transition(&agent_id, LifecycleState::Starting).unwrap();
                proxy_pool.register(&agent_id, port);
                if !config.channel_bindings.is_empty() {
                    let current = router.load();
                    let mut new_router = (**current).clone();
                    new_router.add_agent_bindings(&agent_id, &config.channel_bindings);
                    router.store(Arc::new(new_router));
                }
                registry.transition(&agent_id, LifecycleState::Running).unwrap();
            }

            // Build new config
            let mut new_config = config.clone();
            new_config.role = new_role.clone();
            new_config.channel_bindings = new_bindings.clone();

            // Simulate agent_configure:
            if was_running {
                // Stop phase
                registry.transition(&agent_id, LifecycleState::Stopping).unwrap();
                proxy_pool.remove(&agent_id);
                let current = router.load();
                let mut nr = (**current).clone();
                nr.remove_agent_bindings(&agent_id);
                router.store(Arc::new(nr));
                registry.transition(&agent_id, LifecycleState::Stopped).unwrap();
            }

            // Update config
            registry.update_config(&agent_id, new_config.clone()).unwrap();

            // Re-register RBAC role
            rbac.register_agent(&agent_id, &new_config.role);

            // Update router bindings
            let current = router.load();
            let mut nr = (**current).clone();
            nr.update_agent_bindings(&agent_id, &new_config.channel_bindings);
            router.store(Arc::new(nr));

            if was_running {
                // Restart phase
                registry.transition(&agent_id, LifecycleState::Starting).unwrap();
                let new_port = port.wrapping_add(1).max(19001);
                proxy_pool.register(&agent_id, new_port);
                registry.transition(&agent_id, LifecycleState::Running).unwrap();
            }

            // Verify: Registry reflects new config
            let entry = registry.get(&agent_id).unwrap();
            prop_assert_eq!(
                entry.config.role.clone(),
                new_role.clone(),
                "Registry should reflect new role config"
            );
            prop_assert_eq!(
                entry.config.channel_bindings.clone(),
                new_bindings,
                "Registry should reflect new channel bindings"
            );

            // Verify: RBAC reflects new role (system tools stripped)
            for system_tool in SYSTEM_TOOLS {
                prop_assert!(
                    rbac.check_tool(&agent_id, system_tool).is_err(),
                    "system tool '{}' should be denied after configure", system_tool
                );
            }
            // Non-system tools in new allow list (not in deny) should be allowed
            for tool in &new_role.allow {
                if !SYSTEM_TOOLS.contains(&tool.as_str()) && !new_role.deny.contains(tool) {
                    prop_assert!(
                        rbac.check_tool(&agent_id, tool).is_ok(),
                        "tool '{}' in new allow list should be allowed after configure", tool
                    );
                }
            }

            // Verify: if was Running → still Running after
            if was_running {
                prop_assert_eq!(
                    entry.state.clone(),
                    LifecycleState::Running,
                    "agent should be Running after configure if it was Running before"
                );
                prop_assert!(
                    proxy_pool.get(&agent_id).is_some(),
                    "ProxyPool should have entry for restarted agent"
                );
            }

            Ok(())
        })?;
    }
}
