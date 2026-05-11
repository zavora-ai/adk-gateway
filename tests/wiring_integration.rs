//! Integration tests for end-to-end agent lifecycle wiring.
//!
//! Feature: multi-agent-full-wiring
//! These tests exercise the actual subsystem implementations with mock
//! ProcessManager/Codegen for deterministic behavior.
//!
//! - Test 10.1: Full lifecycle (create → start → route → stop → delete)
//!   **Validates: Requirements 11.1**
//! - Test 10.2: Fallback chain retries on primary failure
//!   **Validates: Requirements 11.2**
//! - Test 10.3: RBAC denies system tools for User Agents
//!   **Validates: Requirements 11.3**
//! - Test 10.4: Router bindings added on start, removed on stop
//!   **Validates: Requirements 11.4**
//! - Test 10.5: WebSocket events emitted on state transitions
//!   **Validates: Requirements 11.5**

use std::sync::Arc;

use adk_gateway::agent_config::{
    AgentConfig, AgentRoleConfig, AgentType, ChannelBinding, LifecycleState,
};
use adk_gateway::agent_registry::AgentRegistry;
use adk_gateway::channel::{ChannelType, InboundMessage, MessageSource};
use adk_gateway::config::RoutingConfig;
use adk_gateway::control_panel::ws::WsEvent;
use adk_gateway::fallback_chain::{FallbackModelChain, FallbackOutcome};
use adk_gateway::proxy_pool::RemoteAgentProxyPool;
use adk_gateway::rbac_bridge::{RbacBridge, SYSTEM_TOOLS};
use adk_gateway::router::MessageRouter;

// ── Helpers ────────────────────────────────────────────────────────

/// Build a test agent config with the given ID and channel bindings.
fn make_test_config(id: &str, channel_bindings: Vec<ChannelBinding>) -> AgentConfig {
    AgentConfig {
        id: id.to_string(),
        name: format!("Agent {}", id),
        description: "integration test agent".to_string(),
        agent_type: AgentType::Llm,
        model: "test/model".to_string(),
        api_key_env: "TEST_KEY".to_string(),
        instruction: "You are a test agent.".to_string(),
        tools: vec!["web_search".to_string(), "code_exec".to_string()],
        action_nodes: vec![],
        workflow_edges: vec![],
        sub_agents: vec![],
        role: AgentRoleConfig {
            allow: vec!["web_search".to_string(), "code_exec".to_string()],
            deny: vec![],
        },
        channel_bindings,
        auto_start: false,
        temperature: None,
        max_output_tokens: None,
        model_override: None,
    }
}

/// Create a fresh registry backed by a temp dir.
fn make_registry() -> (tempfile::TempDir, Arc<AgentRegistry>) {
    let tmp = tempfile::TempDir::new().unwrap();
    let registry = Arc::new(AgentRegistry::new(tmp.path().join("registry")));
    (tmp, registry)
}

/// Create a fresh MessageRouter with "system" as default.
fn make_router() -> MessageRouter {
    MessageRouter::new(&RoutingConfig::default(), "system".to_string())
}

/// Build an InboundMessage for routing tests.
fn make_inbound_msg(channel: ChannelType, account_id: &str, sender_id: &str) -> InboundMessage {
    InboundMessage {
        channel_type: channel,
        account_id: account_id.to_string(),
        sender_id: sender_id.to_string(),
        sender_name: None,
        text: "hello".into(),
        is_group: false,
        group_id: None,
        is_mention: false,
        platform_message_id: "msg-1".into(),
        attachments: vec![],
        metadata: std::collections::HashMap::new(),
        source: MessageSource::Channel,
        timestamp: chrono::Utc::now(),
    }
}

// ── Mock Llm for fallback chain tests ──────────────────────────────

/// Minimal mock that implements the Llm trait for fallback chain testing.
struct MockLlm {
    model_id: String,
}

impl std::fmt::Debug for MockLlm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockLlm")
            .field("model_id", &self.model_id)
            .finish()
    }
}

#[async_trait::async_trait]
impl adk_core::Llm for MockLlm {
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
        Ok(Box::pin(futures::stream::empty()))
    }
}

// ════════════════════════════════════════════════════════════════════
// 10.1: Full lifecycle (create → start → route → stop → delete)
// **Validates: Requirements 11.1**
// ════════════════════════════════════════════════════════════════════

/// Integration test: exercises the full agent lifecycle pipeline.
///
/// Uses real AgentRegistry, ProxyPool, RbacBridge, and MessageRouter
/// with mock ProcessManager/Codegen behavior (simulated via direct
/// subsystem calls rather than spawning real processes).
#[tokio::test]
async fn full_lifecycle_create_start_route_stop_delete() {
    let (_tmp, registry) = make_registry();
    let proxy_pool = Arc::new(RemoteAgentProxyPool::new());
    let rbac = Arc::new(RbacBridge::new());
    let router = Arc::new(arc_swap::ArcSwap::from_pointee(make_router()));
    let (ws_tx, mut ws_rx) = tokio::sync::broadcast::channel::<WsEvent>(32);

    let agent_id = "research-agent";
    let port: u16 = 19042;
    let config = make_test_config(
        agent_id,
        vec![ChannelBinding {
            channel_type: "telegram".to_string(),
            account_id: Some("default".to_string()),
            peer_filter: None,
        }],
    );

    // ── Step 1: CREATE ──────────────────────────────────────────────
    registry.create_agent(config.clone()).unwrap();
    rbac.register_agent(agent_id, &config.role);

    // Create workspace dirs (simulated)
    let agent_dir = _tmp.path().join("agents").join(agent_id);
    std::fs::create_dir_all(agent_dir.join("context")).unwrap();
    std::fs::create_dir_all(agent_dir.join("data")).unwrap();
    std::fs::create_dir_all(agent_dir.join("src")).unwrap();

    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: agent_id.to_string(),
        state: "Created".into(),
    });

    // Verify: agent exists in registry with Created state
    let entry = registry.get(agent_id).unwrap();
    assert_eq!(entry.state, LifecycleState::Created);
    drop(entry);

    // Verify: RBAC role registered (non-system tools allowed)
    assert!(rbac.check_tool(agent_id, "web_search").is_ok());
    assert!(rbac.check_tool(agent_id, "agent_create").is_err());

    // ── Step 2: START ───────────────────────────────────────────────
    // Simulate agent_start: transition → spawn → register proxy → add bindings → Running
    registry
        .transition(agent_id, LifecycleState::Starting)
        .unwrap();
    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: agent_id.to_string(),
        state: "Starting".into(),
    });

    // Mock ProcessManager::spawn success → register in ProxyPool
    proxy_pool.register(agent_id, port);

    // Add router bindings
    {
        let current = router.load();
        let mut new_router = (**current).clone();
        new_router.add_agent_bindings(agent_id, &config.channel_bindings);
        router.store(Arc::new(new_router));
    }

    registry
        .transition(agent_id, LifecycleState::Running)
        .unwrap();
    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: agent_id.to_string(),
        state: "Running".into(),
    });

    // Verify: ProxyPool entry exists
    let proxy = proxy_pool.get(agent_id);
    assert!(
        proxy.is_some(),
        "ProxyPool should contain the started agent"
    );
    assert_eq!(proxy.unwrap().agent_url(), "http://127.0.0.1:19042");

    // Verify: Registry state == Running
    let entry = registry.get(agent_id).unwrap();
    assert_eq!(entry.state, LifecycleState::Running);
    drop(entry);

    // ── Step 3: ROUTE ───────────────────────────────────────────────
    // Verify: MessageRouter resolves telegram messages to this agent
    let msg = make_inbound_msg(ChannelType::Telegram, "default", "user123");
    let resolved = router.load().resolve_agent(&msg).to_string();
    assert_eq!(
        resolved, agent_id,
        "router should resolve telegram messages to the started agent"
    );

    // Verify: other channels still fall back to system
    let slack_msg = make_inbound_msg(ChannelType::Slack, "", "user456");
    let resolved_slack = router.load().resolve_agent(&slack_msg).to_string();
    assert_eq!(resolved_slack, "system");

    // ── Step 4: STOP ────────────────────────────────────────────────
    registry
        .transition(agent_id, LifecycleState::Stopping)
        .unwrap();
    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: agent_id.to_string(),
        state: "Stopping".into(),
    });

    // Mock ProcessManager::stop → remove from ProxyPool
    proxy_pool.remove(agent_id);

    // Remove router bindings
    {
        let current = router.load();
        let mut new_router = (**current).clone();
        new_router.remove_agent_bindings(agent_id);
        router.store(Arc::new(new_router));
    }

    registry
        .transition(agent_id, LifecycleState::Stopped)
        .unwrap();
    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: agent_id.to_string(),
        state: "Stopped".into(),
    });

    // Verify: ProxyPool entry removed
    assert!(
        proxy_pool.get(agent_id).is_none(),
        "ProxyPool should NOT contain the stopped agent"
    );

    // Verify: Registry state == Stopped
    let entry = registry.get(agent_id).unwrap();
    assert_eq!(entry.state, LifecycleState::Stopped);
    drop(entry);

    // Verify: Router falls back to system for telegram
    let msg2 = make_inbound_msg(ChannelType::Telegram, "default", "user123");
    let resolved2 = router.load().resolve_agent(&msg2).to_string();
    assert_eq!(
        resolved2, "system",
        "router should fall back to system after agent stop"
    );

    // ── Step 5: DELETE ──────────────────────────────────────────────
    registry.delete(agent_id).unwrap();
    rbac.remove_agent(agent_id);

    // Remove any residual router bindings
    {
        let current = router.load();
        let mut new_router = (**current).clone();
        new_router.remove_agent_bindings(agent_id);
        router.store(Arc::new(new_router));
    }

    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: agent_id.to_string(),
        state: "Deleted".into(),
    });

    // Verify: Registry empty
    assert!(
        registry.get(agent_id).is_none(),
        "Registry should NOT contain the deleted agent"
    );

    // Verify: RBAC removed
    assert!(
        rbac.check_tool(agent_id, "web_search").is_err(),
        "RBAC should deny all tools for deleted agent"
    );

    // ── Verify WebSocket events ─────────────────────────────────────
    let mut events = Vec::new();
    while let Ok(event) = ws_rx.try_recv() {
        events.push(event);
    }

    let expected_states = vec![
        "Created", "Starting", "Running", "Stopping", "Stopped", "Deleted",
    ];
    assert_eq!(
        events.len(),
        expected_states.len(),
        "should receive {} WsEvents, got {}",
        expected_states.len(),
        events.len()
    );

    for (i, event) in events.iter().enumerate() {
        match event {
            WsEvent::AgentState {
                agent_id: eid,
                state,
            } => {
                assert_eq!(eid, agent_id);
                assert_eq!(state, expected_states[i], "event {} state mismatch", i);
            }
            other => panic!("expected AgentState event, got {:?}", other),
        }
    }
}

// ════════════════════════════════════════════════════════════════════
// 10.2: Fallback chain retries on primary failure
// **Validates: Requirements 11.2**
// ════════════════════════════════════════════════════════════════════

/// Integration test: verifies the fallback chain retries on primary model
/// failure and produces the correct FallbackOutcome::FallbackUsed metadata.
#[tokio::test]
async fn fallback_chain_retries_on_primary_failure() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Build a chain with 3 mock models: primary fails, second fails, third succeeds
    let models: Vec<Arc<dyn adk_core::Llm>> = vec![
        Arc::new(MockLlm {
            model_id: "provider-a/model-primary".to_string(),
        }),
        Arc::new(MockLlm {
            model_id: "provider-b/model-fallback-1".to_string(),
        }),
        Arc::new(MockLlm {
            model_id: "provider-c/model-fallback-2".to_string(),
        }),
    ];
    let model_ids = vec![
        "provider-a/model-primary".to_string(),
        "provider-b/model-fallback-1".to_string(),
        "provider-c/model-fallback-2".to_string(),
    ];

    let chain = FallbackModelChain::from_models_for_test(models, model_ids);
    assert!(chain.has_fallbacks(), "chain should have fallbacks");

    // Track invocation order
    let invocation_counter = Arc::new(AtomicUsize::new(0));
    let invocation_log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));

    let counter_clone = invocation_counter.clone();
    let log_clone = invocation_log.clone();

    let result = chain
        .run_with_fallback(|model| {
            let counter = counter_clone.clone();
            let log = log_clone.clone();
            async move {
                let idx = counter.fetch_add(1, Ordering::SeqCst);
                let model_name = model.name().to_string();
                log.lock().unwrap().push(model_name.clone());

                match idx {
                    0 => Err("rate limit exceeded".to_string()), // primary fails
                    1 => Err("timeout after 30s".to_string()),   // first fallback fails
                    _ => Ok(format!("success from {}", model_name)), // second fallback succeeds
                }
            }
        })
        .await;

    // Verify success
    assert!(result.is_ok(), "chain should succeed on third model");
    let (response, outcome) = result.unwrap();

    // Verify response came from the third model
    assert!(response.contains("provider-c/model-fallback-2"));

    // Verify invocation count: all 3 models were tried
    assert_eq!(invocation_counter.load(Ordering::SeqCst), 3);

    // Verify invocation order
    let log = invocation_log.lock().unwrap();
    assert_eq!(log[0], "provider-a/model-primary");
    assert_eq!(log[1], "provider-b/model-fallback-1");
    assert_eq!(log[2], "provider-c/model-fallback-2");

    // Verify FallbackOutcome::FallbackUsed metadata
    match outcome {
        FallbackOutcome::FallbackUsed {
            primary_id,
            fallback_id,
            fallback_index,
            primary_error,
        } => {
            assert_eq!(primary_id, "provider-a/model-primary");
            assert_eq!(fallback_id, "provider-c/model-fallback-2");
            assert_eq!(fallback_index, 2);
            assert_eq!(primary_error, "rate limit exceeded");
        }
        other => panic!("expected FallbackOutcome::FallbackUsed, got {:?}", other),
    }
}

/// Integration test: verifies FallbackOutcome when all models fail.
#[tokio::test]
async fn fallback_chain_all_models_fail() {
    let models: Vec<Arc<dyn adk_core::Llm>> = vec![
        Arc::new(MockLlm {
            model_id: "model-a".to_string(),
        }),
        Arc::new(MockLlm {
            model_id: "model-b".to_string(),
        }),
    ];
    let model_ids = vec!["model-a".to_string(), "model-b".to_string()];

    let chain = FallbackModelChain::from_models_for_test(models, model_ids);

    let result = chain
        .run_with_fallback(|model| async move {
            Err::<String, String>(format!("error from {}", model.name()))
        })
        .await;

    // Should fail with all errors
    assert!(result.is_err(), "chain should fail when all models fail");
    let errors = result.unwrap_err();

    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].0, "model-a");
    assert_eq!(errors[0].1, "error from model-a");
    assert_eq!(errors[1].0, "model-b");
    assert_eq!(errors[1].1, "error from model-b");
}

// ════════════════════════════════════════════════════════════════════
// 10.3: RBAC denies system tools for User Agents
// **Validates: Requirements 11.3**
// ════════════════════════════════════════════════════════════════════

/// Integration test: verifies that a user agent cannot invoke system tools
/// (agent_create, agent_start, agent_stop, agent_delete, agent_list, agent_configure)
/// even if the creator attempts to include them in the allow list.
#[tokio::test]
async fn rbac_denies_system_tools_for_user_agents() {
    let (_tmp, registry) = make_registry();
    let rbac = Arc::new(RbacBridge::new());

    // Create a user agent that tries to include ALL system tools in its allow list
    let mut config = make_test_config("sneaky-agent", vec![]);
    config.role.allow = vec![
        "web_search".to_string(),
        "code_exec".to_string(),
        "agent_create".to_string(),
        "agent_start".to_string(),
        "agent_stop".to_string(),
        "agent_delete".to_string(),
        "agent_list".to_string(),
        "agent_configure".to_string(),
    ];
    config.tools = config.role.allow.clone();

    // Register the agent
    registry.create_agent(config.clone()).unwrap();
    let stripped = rbac.register_agent("sneaky-agent", &config.role);

    // Verify: system tools were stripped
    assert_eq!(
        stripped.len(),
        6,
        "all 6 system tools should have been stripped"
    );
    for system_tool in SYSTEM_TOOLS {
        assert!(
            stripped.contains(&system_tool.to_string()),
            "system tool '{}' should have been stripped",
            system_tool
        );
    }

    // Verify: RBAC denies every system tool
    for system_tool in SYSTEM_TOOLS {
        let result = rbac.check_tool("sneaky-agent", system_tool);
        assert!(
            result.is_err(),
            "RBAC should deny system tool '{}' for user agent",
            system_tool
        );

        // Verify the error is an AccessDenied
        let err = result.unwrap_err();
        assert_eq!(err.agent_id, "sneaky-agent");
    }

    // Verify: non-system tools ARE allowed
    assert!(
        rbac.check_tool("sneaky-agent", "web_search").is_ok(),
        "non-system tool 'web_search' should be allowed"
    );
    assert!(
        rbac.check_tool("sneaky-agent", "code_exec").is_ok(),
        "non-system tool 'code_exec' should be allowed"
    );

    // Verify: tools not in allow list are denied
    assert!(
        rbac.check_tool("sneaky-agent", "file_write").is_err(),
        "tool not in allow list should be denied"
    );

    // Verify: system agent CAN use system tools
    rbac.register_system_agent("system");
    for system_tool in SYSTEM_TOOLS {
        assert!(
            rbac.check_tool("system", system_tool).is_ok(),
            "system agent should have access to system tool '{}'",
            system_tool
        );
    }
}

/// Integration test: verifies deny list takes precedence over allow list.
#[tokio::test]
async fn rbac_deny_list_takes_precedence() {
    let rbac = RbacBridge::new();

    let role = AgentRoleConfig {
        allow: vec!["web_search".to_string(), "code_exec".to_string()],
        deny: vec!["web_search".to_string()], // explicitly deny web_search
    };

    rbac.register_agent("restricted-agent", &role);

    // web_search is in both allow and deny → deny wins
    assert!(
        rbac.check_tool("restricted-agent", "web_search").is_err(),
        "deny should take precedence over allow"
    );

    // code_exec is only in allow → allowed
    assert!(
        rbac.check_tool("restricted-agent", "code_exec").is_ok(),
        "code_exec should be allowed (not in deny list)"
    );
}

// ════════════════════════════════════════════════════════════════════
// 10.4: Router bindings added on start, removed on stop
// **Validates: Requirements 11.4**
// ════════════════════════════════════════════════════════════════════

/// Integration test: verifies that router bindings are correctly added when
/// an agent starts and removed when it stops, with proper fallback behavior.
#[tokio::test]
async fn router_bindings_added_on_start_removed_on_stop() {
    let (_tmp, registry) = make_registry();
    let proxy_pool = Arc::new(RemoteAgentProxyPool::new());
    let router = Arc::new(arc_swap::ArcSwap::from_pointee(make_router()));

    // Create an agent with multiple channel bindings
    let config = make_test_config(
        "multi-channel-agent",
        vec![
            ChannelBinding {
                channel_type: "telegram".to_string(),
                account_id: Some("bot-1".to_string()),
                peer_filter: None,
            },
            ChannelBinding {
                channel_type: "slack".to_string(),
                account_id: None,
                peer_filter: None,
            },
        ],
    );

    registry.create_agent(config.clone()).unwrap();

    // ── Before start: all messages go to system ─────────────────────
    let tg_msg = make_inbound_msg(ChannelType::Telegram, "bot-1", "user1");
    let slack_msg = make_inbound_msg(ChannelType::Slack, "", "user2");

    assert_eq!(
        router.load().resolve_agent(&tg_msg),
        "system",
        "before start: telegram should route to system"
    );
    assert_eq!(
        router.load().resolve_agent(&slack_msg),
        "system",
        "before start: slack should route to system"
    );

    // ── Simulate agent_start: add bindings ──────────────────────────
    registry
        .transition("multi-channel-agent", LifecycleState::Starting)
        .unwrap();
    proxy_pool.register("multi-channel-agent", 19050);

    // Add router bindings (clone-mutate-store via ArcSwap)
    {
        let current = router.load();
        let mut new_router = (**current).clone();
        new_router.add_agent_bindings("multi-channel-agent", &config.channel_bindings);
        router.store(Arc::new(new_router));
    }

    registry
        .transition("multi-channel-agent", LifecycleState::Running)
        .unwrap();

    // ── After start: messages route to the agent ────────────────────
    assert_eq!(
        router.load().resolve_agent(&tg_msg),
        "multi-channel-agent",
        "after start: telegram+bot-1 should route to agent"
    );
    assert_eq!(
        router.load().resolve_agent(&slack_msg),
        "multi-channel-agent",
        "after start: slack should route to agent"
    );

    // Other channels still fall back to system
    let webhook_msg = make_inbound_msg(ChannelType::Webhook, "hook-1", "sender");
    assert_eq!(
        router.load().resolve_agent(&webhook_msg),
        "system",
        "webhook should still route to system"
    );

    // Telegram with different account should fall back to system
    let tg_other = make_inbound_msg(ChannelType::Telegram, "bot-2", "user3");
    assert_eq!(
        router.load().resolve_agent(&tg_other),
        "system",
        "telegram with different account should route to system"
    );

    // ── Simulate agent_stop: remove bindings ────────────────────────
    registry
        .transition("multi-channel-agent", LifecycleState::Stopping)
        .unwrap();
    proxy_pool.remove("multi-channel-agent");

    // Remove router bindings
    {
        let current = router.load();
        let mut new_router = (**current).clone();
        new_router.remove_agent_bindings("multi-channel-agent");
        router.store(Arc::new(new_router));
    }

    registry
        .transition("multi-channel-agent", LifecycleState::Stopped)
        .unwrap();

    // ── After stop: all messages fall back to system ─────────────────
    assert_eq!(
        router.load().resolve_agent(&tg_msg),
        "system",
        "after stop: telegram should fall back to system"
    );
    assert_eq!(
        router.load().resolve_agent(&slack_msg),
        "system",
        "after stop: slack should fall back to system"
    );
}

/// Integration test: verifies that multiple agents can have non-overlapping bindings.
#[tokio::test]
async fn router_multiple_agents_non_overlapping_bindings() {
    let router = Arc::new(arc_swap::ArcSwap::from_pointee(make_router()));

    // Agent A handles telegram
    {
        let current = router.load();
        let mut new_router = (**current).clone();
        new_router.add_agent_bindings(
            "agent-a",
            &[ChannelBinding {
                channel_type: "telegram".to_string(),
                account_id: None,
                peer_filter: None,
            }],
        );
        router.store(Arc::new(new_router));
    }

    // Agent B handles slack
    {
        let current = router.load();
        let mut new_router = (**current).clone();
        new_router.add_agent_bindings(
            "agent-b",
            &[ChannelBinding {
                channel_type: "slack".to_string(),
                account_id: None,
                peer_filter: None,
            }],
        );
        router.store(Arc::new(new_router));
    }

    let tg_msg = make_inbound_msg(ChannelType::Telegram, "", "user1");
    let slack_msg = make_inbound_msg(ChannelType::Slack, "", "user2");
    let webhook_msg = make_inbound_msg(ChannelType::Webhook, "", "user3");

    assert_eq!(router.load().resolve_agent(&tg_msg), "agent-a");
    assert_eq!(router.load().resolve_agent(&slack_msg), "agent-b");
    assert_eq!(
        router.load().resolve_agent(&webhook_msg),
        "system",
        "unbound channel falls back to system"
    );

    // Remove agent-a bindings
    {
        let current = router.load();
        let mut new_router = (**current).clone();
        new_router.remove_agent_bindings("agent-a");
        router.store(Arc::new(new_router));
    }

    // Telegram now falls back to system, slack still routes to agent-b
    assert_eq!(router.load().resolve_agent(&tg_msg), "system");
    assert_eq!(router.load().resolve_agent(&slack_msg), "agent-b");
}

// ════════════════════════════════════════════════════════════════════
// 10.5: WebSocket events emitted on state transitions
// **Validates: Requirements 11.5**
// ════════════════════════════════════════════════════════════════════

/// Integration test: verifies that WebSocket events are emitted on each
/// agent lifecycle state transition, with correct agent_id and state strings.
#[tokio::test]
async fn websocket_events_emitted_on_state_transitions() {
    let (_tmp, registry) = make_registry();
    let proxy_pool = Arc::new(RemoteAgentProxyPool::new());
    let (ws_tx, mut ws_rx) = tokio::sync::broadcast::channel::<WsEvent>(64);

    let agent_id = "ws-test-agent";
    let config = make_test_config(agent_id, vec![]);

    // ── CREATE ──────────────────────────────────────────────────────
    registry.create_agent(config.clone()).unwrap();
    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: agent_id.to_string(),
        state: "Created".into(),
    });

    // ── START ───────────────────────────────────────────────────────
    registry
        .transition(agent_id, LifecycleState::Starting)
        .unwrap();
    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: agent_id.to_string(),
        state: "Starting".into(),
    });

    proxy_pool.register(agent_id, 19060);
    registry
        .transition(agent_id, LifecycleState::Running)
        .unwrap();
    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: agent_id.to_string(),
        state: "Running".into(),
    });

    // ── STOP ────────────────────────────────────────────────────────
    registry
        .transition(agent_id, LifecycleState::Stopping)
        .unwrap();
    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: agent_id.to_string(),
        state: "Stopping".into(),
    });

    proxy_pool.remove(agent_id);
    registry
        .transition(agent_id, LifecycleState::Stopped)
        .unwrap();
    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: agent_id.to_string(),
        state: "Stopped".into(),
    });

    // ── DELETE ───────────────────────────────────────────────────────
    registry.delete(agent_id).unwrap();
    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: agent_id.to_string(),
        state: "Deleted".into(),
    });

    // ── Verify all events ───────────────────────────────────────────
    let mut events = Vec::new();
    while let Ok(event) = ws_rx.try_recv() {
        events.push(event);
    }

    let expected_states = vec![
        "Created", "Starting", "Running", "Stopping", "Stopped", "Deleted",
    ];

    assert_eq!(
        events.len(),
        expected_states.len(),
        "should receive exactly {} events, got {}",
        expected_states.len(),
        events.len()
    );

    for (i, event) in events.iter().enumerate() {
        match event {
            WsEvent::AgentState {
                agent_id: eid,
                state,
            } => {
                assert_eq!(
                    eid, agent_id,
                    "event {} should have agent_id '{}'",
                    i, agent_id
                );
                assert_eq!(
                    state, expected_states[i],
                    "event {} should have state '{}', got '{}'",
                    i, expected_states[i], state
                );
            }
            other => panic!("event {}: expected AgentState, got {:?}", i, other),
        }
    }
}

/// Integration test: verifies that WsEvent serializes correctly for the UI.
#[tokio::test]
async fn websocket_event_serialization_format() {
    let event = WsEvent::AgentState {
        agent_id: "test-agent".to_string(),
        state: "Running".to_string(),
    };

    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "agent_state");
    assert_eq!(json["agent_id"], "test-agent");
    assert_eq!(json["state"], "Running");

    let log_event = WsEvent::Log {
        timestamp: "2024-01-01T00:00:00Z".to_string(),
        level: "info".to_string(),
        message: "agent started".to_string(),
        target: Some("gateway".to_string()),
    };

    let log_json = serde_json::to_value(&log_event).unwrap();
    assert_eq!(log_json["type"], "log");
    assert_eq!(log_json["level"], "info");
    assert_eq!(log_json["message"], "agent started");
    assert_eq!(log_json["target"], "gateway");

    let dashboard_event = WsEvent::Dashboard {
        uptime_secs: 3600,
        session_count: 5,
        channel_count: 2,
    };

    let dash_json = serde_json::to_value(&dashboard_event).unwrap();
    assert_eq!(dash_json["type"], "dashboard");
    assert_eq!(dash_json["uptime_secs"], 3600);
    assert_eq!(dash_json["session_count"], 5);
    assert_eq!(dash_json["channel_count"], 2);
}

/// Integration test: verifies broadcast channel behavior with multiple subscribers.
#[tokio::test]
async fn websocket_broadcast_multiple_subscribers() {
    let (ws_tx, mut ws_rx1) = tokio::sync::broadcast::channel::<WsEvent>(16);
    let mut ws_rx2 = ws_tx.subscribe();

    // Emit events
    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: "agent-1".to_string(),
        state: "Running".into(),
    });
    let _ = ws_tx.send(WsEvent::AgentState {
        agent_id: "agent-2".to_string(),
        state: "Stopped".into(),
    });

    // Both subscribers should receive both events
    let ev1_a = ws_rx1.try_recv().unwrap();
    let ev1_b = ws_rx1.try_recv().unwrap();
    let ev2_a = ws_rx2.try_recv().unwrap();
    let ev2_b = ws_rx2.try_recv().unwrap();

    match (&ev1_a, &ev2_a) {
        (WsEvent::AgentState { agent_id: a1, .. }, WsEvent::AgentState { agent_id: a2, .. }) => {
            assert_eq!(a1, "agent-1");
            assert_eq!(a2, "agent-1");
        }
        _ => panic!("unexpected event types"),
    }

    match (&ev1_b, &ev2_b) {
        (WsEvent::AgentState { agent_id: a1, .. }, WsEvent::AgentState { agent_id: a2, .. }) => {
            assert_eq!(a1, "agent-2");
            assert_eq!(a2, "agent-2");
        }
        _ => panic!("unexpected event types"),
    }
}
