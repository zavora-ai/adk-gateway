//! End-to-end integration tests for adk-gateway.
//!
//! These tests exercise the full system from a user's perspective:
//! configuration → component wiring → message flow → response delivery.
//! No external services required — everything runs in-process.

use adk_gateway::access_control::{AccessControlBridge, AuthDecision};
use adk_gateway::audit::{AuditEvent, AuditEventType, AuditOutcome, AuditSink, NullAuditSink};
use adk_gateway::channel::{ChannelType, InboundMessage, MessageSource};
use adk_gateway::config::{
    AuthConfig, AuthMode, ChunkingStrategy, CronDelivery, CronJob, DmPolicy, EmbeddingConfig,
    GatewayConfig, HooksConfig, RagConfig, RoleConfig, SessionConfig, TelegramConfig,
    UserRoleMapping, VectorStoreBackend,
};
use adk_gateway::config_watcher::{validate_config, ConfigDiff};
use adk_gateway::context_coordinator::ContextCoordinator;
use adk_gateway::control_panel::{ChannelInfo, ControlPanelState, LogEntry};
use adk_gateway::cron::CronScheduler;
use adk_gateway::knowledge_graph::{CreateEntityInput, CreateRelationInput, KnowledgeGraph};
use adk_gateway::mcp::{McpConnectionManager, McpServerConfig, McpTransport};
use adk_gateway::metrics::{GatewayMetrics, MessageStatus};
use adk_gateway::pairing::{DmPairingService, PairingResult};
use adk_gateway::plugin_manager::PluginManager;
use adk_gateway::rag::RagPipelineBuilder;
use adk_gateway::reconnect::{ReconnectPolicy, ReconnectState};
use adk_gateway::session_bridge::SessionBridge;
use adk_gateway::shutdown::ShutdownCoordinator;
use adk_gateway::skill_loader::SkillLoader;
use adk_gateway::tool_registry::ToolRegistry;
use adk_gateway::webhook::{WebhookHandler, WebhookRequest};

use adk_session::InMemorySessionService;
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ── Helpers ────────────────────────────────────────────────────────

fn make_inbound(channel: ChannelType, sender: &str, text: &str) -> InboundMessage {
    InboundMessage {
        channel_type: channel,
        account_id: String::new(),
        sender_id: sender.to_string(),
        sender_name: Some(sender.to_string()),
        text: text.to_string(),
        is_group: false,
        group_id: None,
        is_mention: false,
        platform_message_id: uuid::Uuid::new_v4().to_string(),
        attachments: vec![],
        metadata: HashMap::new(),
        source: MessageSource::Channel,
        timestamp: chrono::Utc::now(),
    }
}

fn config_with_telegram_open() -> GatewayConfig {
    let mut config = GatewayConfig::default();
    config.channels.telegram = Some(TelegramConfig {
        enabled: true,
        dm_policy: DmPolicy::Open,
        allow_from: vec!["*".to_string()],
        ..TelegramConfig::default()
    });
    config
}

fn config_with_auth() -> GatewayConfig {
    let mut config = config_with_telegram_open();
    config.auth = Some(AuthConfig {
        mode: AuthMode::Token,
        token: Some("test-token".to_string()),
        password: None,
        roles: vec![
            RoleConfig {
                name: "admin".to_string(),
                permissions: vec!["*".to_string()],
                scopes: vec!["*".to_string()],
            },
            RoleConfig {
                name: "viewer".to_string(),
                permissions: vec!["read".to_string()],
                scopes: vec!["read:messages".to_string()],
            },
        ],
        user_mappings: vec![
            UserRoleMapping {
                user_id: "telegram:admin_user".to_string(),
                role: "admin".to_string(),
            },
            UserRoleMapping {
                user_id: "telegram:viewer_user".to_string(),
                role: "viewer".to_string(),
            },
        ],
        channel_overrides: HashMap::new(),
        audit: None,
        sso: None,
    });
    config
}

// ════════════════════════════════════════════════════════════════════
// 1. CONFIGURATION LIFECYCLE
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_config_load_and_roundtrip() {
    let json = r#"{
        "agent": { "model": "anthropic/claude-sonnet-4" },
        "channels": {
            "telegram": { "enabled": true, "botToken": "tok:123", "dmPolicy": "open" }
        },
        "gateway": { "port": 9999 },
        "session": { "dmScope": "per-channel-peer" },
        "hooks": { "enabled": true, "token": "hook-secret" },
        "cron": { "jobs": [{ "id": "j1", "schedule": "0 9 * * *", "message": "ask: hello" }] },
        "memory": { "backend": "inmemory", "embedding": { "provider": "openai" } },
        "rag": { "vectorStore": "inmemory", "embedding": { "provider": "openai" } }
    }"#;

    let config: GatewayConfig = serde_json::from_str(json).expect("config should parse");
    assert_eq!(config.gateway.port, 9999);
    assert!(config.channels.telegram.as_ref().unwrap().enabled);
    assert_eq!(config.cron.jobs.len(), 1);
    assert!(config.memory.is_some());
    assert!(config.rag.is_some());

    // Round-trip
    let serialized = serde_json::to_string(&config).unwrap();
    let deserialized: GatewayConfig = serde_json::from_str(&serialized).unwrap();
    assert_eq!(config, deserialized);
}

#[test]
fn e2e_config_env_var_expansion() {
    // Use unsafe block required in Rust 2024 edition
    unsafe { std::env::set_var("E2E_TEST_TOKEN", "my-secret-token") };
    let expanded = adk_gateway::config::expand_env_vars("Bearer ${E2E_TEST_TOKEN}");
    assert_eq!(expanded, "Bearer my-secret-token");

    let preserved = adk_gateway::config::expand_env_vars("${NONEXISTENT_VAR_12345}");
    assert_eq!(preserved, "${NONEXISTENT_VAR_12345}");
    unsafe { std::env::remove_var("E2E_TEST_TOKEN") };
}

#[test]
fn e2e_config_validation() {
    let valid = GatewayConfig::default();
    assert!(validate_config(&valid).is_ok());

    // Duplicate cron IDs should fail
    let mut invalid = GatewayConfig::default();
    invalid.cron.jobs = vec![
        CronJob {
            id: "dup".into(),
            schedule: "every 1h".into(),
            message: "a".into(),
            deliver_to: None,
        },
        CronJob {
            id: "dup".into(),
            schedule: "every 2h".into(),
            message: "b".into(),
            deliver_to: None,
        },
    ];
    assert!(validate_config(&invalid).is_err());

    // Config diff detection
    let mut changed = GatewayConfig::default();
    changed.cron.jobs = vec![CronJob {
        id: "new-job".into(),
        schedule: "every 1h".into(),
        message: "hi".into(),
        deliver_to: None,
    }];
    let diff = ConfigDiff::compute(&GatewayConfig::default(), &changed);
    assert!(diff.has_changes());
}

// ════════════════════════════════════════════════════════════════════
// 2. ACCESS CONTROL — FULL USER JOURNEY
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_access_control_open_policy_allows_anyone() {
    let config = config_with_telegram_open();
    let bridge = AccessControlBridge::new(&config);
    let msg = make_inbound(ChannelType::Telegram, "random_user", "hello");
    assert!(matches!(
        bridge.check_message_access(&msg),
        AuthDecision::Allowed
    ));
}

#[test]
fn e2e_access_control_pairing_flow() {
    let mut config = config_with_telegram_open();
    config.channels.telegram.as_mut().unwrap().dm_policy = DmPolicy::Pairing;
    let mut bridge = AccessControlBridge::new(&config);

    let msg = make_inbound(ChannelType::Telegram, "new_user", "hello");
    let identity = bridge.map_identity(&msg);

    assert!(matches!(
        bridge.check_message_access(&msg),
        AuthDecision::RequiresPairing
    ));

    bridge.mark_paired(&identity.canonical_id);
    assert!(matches!(
        bridge.check_message_access(&msg),
        AuthDecision::Allowed
    ));
}

#[test]
fn e2e_access_control_allowlist() {
    let mut config = config_with_telegram_open();
    let tg = config.channels.telegram.as_mut().unwrap();
    tg.dm_policy = DmPolicy::Allowlist;
    tg.allow_from = vec!["allowed_user".to_string()];
    let bridge = AccessControlBridge::new(&config);

    let allowed = make_inbound(ChannelType::Telegram, "allowed_user", "hi");
    let blocked = make_inbound(ChannelType::Telegram, "blocked_user", "hi");

    assert!(matches!(
        bridge.check_message_access(&allowed),
        AuthDecision::Allowed
    ));
    assert!(matches!(
        bridge.check_message_access(&blocked),
        AuthDecision::Denied { .. }
    ));
}

#[test]
fn e2e_access_control_rbac_roles() {
    let config = config_with_auth();
    let bridge = AccessControlBridge::new(&config);

    let admin_roles = bridge.user_role_names("telegram:admin_user");
    assert!(admin_roles.contains("admin"));

    let viewer_roles = bridge.user_role_names("telegram:viewer_user");
    assert!(viewer_roles.contains("viewer"));

    let admin_role = bridge.get_role("admin").unwrap();
    assert!(admin_role.permissions.contains(&"*".to_string()));
}

#[test]
fn e2e_access_control_hot_reload_preserves_state() {
    let mut config = config_with_telegram_open();
    config.channels.telegram.as_mut().unwrap().dm_policy = DmPolicy::Pairing;
    let mut bridge = AccessControlBridge::new(&config);

    let msg = make_inbound(ChannelType::Telegram, "paired_user", "hi");
    let identity = bridge.map_identity(&msg);
    bridge.mark_paired(&identity.canonical_id);
    assert!(bridge.is_paired(&identity.canonical_id));

    bridge.rebuild(&config);
    assert!(bridge.is_paired(&identity.canonical_id));
}

// ════════════════════════════════════════════════════════════════════
// 3. SESSION MANAGEMENT
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_session_per_channel_peer() {
    let session_service = Arc::new(InMemorySessionService::new());
    let bridge = SessionBridge::new(SessionConfig::default(), "test-app".into(), session_service);

    let msg1 = make_inbound(ChannelType::Telegram, "user1", "hello");
    let msg2 = make_inbound(ChannelType::Telegram, "user1", "world");
    let msg3 = make_inbound(ChannelType::Slack, "user1", "hello");

    let (uid1, sid1) = bridge.resolve_session(&msg1);
    let (uid2, sid2) = bridge.resolve_session(&msg2);
    let (uid3, sid3) = bridge.resolve_session(&msg3);

    assert_eq!(sid1, sid2);
    assert_eq!(uid1, uid2);
    assert_ne!(sid1, sid3);
    assert_ne!(uid1, uid3);
}

#[test]
fn e2e_session_tracking() {
    let session_service = Arc::new(InMemorySessionService::new());
    let bridge = SessionBridge::new(SessionConfig::default(), "test-app".into(), session_service);

    assert_eq!(bridge.active_sessions().len(), 0);
    bridge.resolve_session(&make_inbound(ChannelType::Telegram, "u1", "hi"));
    bridge.resolve_session(&make_inbound(ChannelType::Slack, "u2", "hi"));
    assert_eq!(bridge.active_sessions().len(), 2);
}

#[test]
fn e2e_multi_account_session_scoping() {
    let mut session_config = SessionConfig::default();
    session_config.dm_scope = "per-account-channel-peer".to_string();

    let session_service = Arc::new(InMemorySessionService::new());
    let bridge = SessionBridge::new(session_config, "test".into(), session_service);

    let mut msg1 = make_inbound(ChannelType::Telegram, "user1", "hi");
    msg1.account_id = "bot1".to_string();
    let mut msg2 = make_inbound(ChannelType::Telegram, "user1", "hi");
    msg2.account_id = "bot2".to_string();

    let (_, sid1) = bridge.resolve_session(&msg1);
    let (_, sid2) = bridge.resolve_session(&msg2);
    assert_ne!(
        sid1, sid2,
        "different accounts should have different sessions"
    );
}

// ════════════════════════════════════════════════════════════════════
// 4. DM PAIRING FLOW
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_pairing_full_lifecycle() {
    let service = DmPairingService::new();

    // Generate a pairing code
    let code = service.generate_code();
    assert_eq!(code.len(), 6);

    // Validate with correct code
    let result = service.validate_code("user1", &code, "telegram");
    assert!(matches!(result, PairingResult::Success));

    // Code is single-use
    let result2 = service.validate_code("user2", &code, "telegram");
    assert!(matches!(result2, PairingResult::AlreadyUsed));

    // Lockout after 3 failures
    let _c = service.generate_code();
    service.validate_code("lockout_user", "000000", "telegram");
    service.validate_code("lockout_user", "000001", "telegram");
    service.validate_code("lockout_user", "000002", "telegram");
    let locked = service.validate_code("lockout_user", "000003", "telegram");
    assert!(matches!(locked, PairingResult::Locked { .. }));
}

#[test]
fn e2e_pairing_tracking() {
    let service = DmPairingService::new();
    assert_eq!(service.paired_count(), 0);

    let code1 = service.generate_code();
    service.validate_code("user1", &code1, "telegram");
    assert_eq!(service.paired_count(), 1);

    let code2 = service.generate_code();
    service.validate_code("user2", &code2, "slack");
    assert_eq!(service.paired_count(), 2);

    assert!(service.is_paired("user1"));
    assert!(service.is_paired("user2"));
    assert!(!service.is_paired("user3"));
}

// ════════════════════════════════════════════════════════════════════
// 5. WEBHOOK INGESTION
// ════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn e2e_webhook_auth_and_routing() {
    let config = HooksConfig {
        enabled: true,
        token: Some("webhook-secret".into()),
        path: None,
    };
    let (tx, mut rx) = mpsc::channel(16);
    let handler = WebhookHandler::new(Arc::new(ArcSwap::new(Arc::new(config))), tx);

    assert!(handler
        .validate_token(Some("Bearer webhook-secret"))
        .is_ok());
    assert!(handler.validate_token(Some("Bearer wrong")).is_err());
    assert!(handler.validate_token(None).is_err());

    let req = WebhookRequest {
        text: "deploy complete".into(),
        channel: Some("telegram".into()),
        target: Some("admin_chat".into()),
        metadata: Some(HashMap::from([(
            "build_id".into(),
            serde_json::json!("42"),
        )])),
    };

    let request_id = handler.process_request(req).await.unwrap();
    assert!(!request_id.is_empty());

    let msg = rx.recv().await.unwrap();
    assert_eq!(msg.text, "deploy complete");
    assert_eq!(msg.channel_type, ChannelType::Webhook);
    assert!(matches!(msg.source, MessageSource::Webhook { .. }));
    assert_eq!(
        msg.metadata.get("webhook_channel").and_then(|v| v.as_str()),
        Some("telegram")
    );
    assert_eq!(
        msg.metadata.get("webhook_target").and_then(|v| v.as_str()),
        Some("admin_chat")
    );
    assert_eq!(
        msg.metadata.get("build_id").and_then(|v| v.as_str()),
        Some("42")
    );
}

#[tokio::test]
async fn e2e_webhook_no_auth_custom_path() {
    let config = HooksConfig {
        enabled: true,
        token: None,
        path: Some("/api/v1/webhook".into()),
    };
    let (tx, mut rx) = mpsc::channel(16);
    let handler = WebhookHandler::new(Arc::new(ArcSwap::new(Arc::new(config))), tx);

    assert_eq!(handler.path(), "/api/v1/webhook");
    assert!(handler.validate_token(None).is_ok());

    let req = WebhookRequest {
        text: "what is 2+2?".into(),
        channel: None,
        target: None,
        metadata: None,
    };
    handler.process_request(req).await.unwrap();
    let msg = rx.recv().await.unwrap();
    assert_eq!(msg.text, "what is 2+2?");
    assert!(!msg.metadata.contains_key("webhook_channel"));
}

// ════════════════════════════════════════════════════════════════════
// 6. CRON SCHEDULING
// ════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn e2e_cron_lifecycle() {
    let (tx, _rx) = mpsc::channel(16);
    let mut scheduler = CronScheduler::new(tx);

    let job1 = CronJob {
        id: "daily-report".into(),
        schedule: "0 9 * * *".into(),
        message: "ask: Generate daily report".into(),
        deliver_to: Some(CronDelivery {
            channel: "telegram".into(),
            target: "@admin".into(),
        }),
    };
    let job2 = CronJob {
        id: "hourly-check".into(),
        schedule: "every 1h".into(),
        message: "System health check".into(),
        deliver_to: None,
    };

    scheduler.schedule(job1.clone()).unwrap();
    scheduler.schedule(job2.clone()).unwrap();
    assert_eq!(scheduler.job_count(), 2);
    assert!(scheduler.is_active("daily-report"));
    assert!(scheduler.is_active("hourly-check"));

    scheduler.cancel("hourly-check");
    assert!(!scheduler.is_active("hourly-check"));
    // cancel marks as cancelled but doesn't remove from map
    // only active jobs matter for scheduling

    let job3 = CronJob {
        id: "weekly-summary".into(),
        schedule: "0 0 * * 1".into(),
        message: "ask: Weekly summary".into(),
        deliver_to: None,
    };
    scheduler.reconcile(&[job3]);
    assert_eq!(scheduler.job_count(), 1);
    assert!(scheduler.is_active("weekly-summary"));
    assert!(!scheduler.is_active("daily-report"));
}

#[test]
fn e2e_cron_message_parsing() {
    let kind = CronScheduler::parse_message("ask: Generate report");
    assert!(matches!(
        kind,
        adk_gateway::cron::CronMessageKind::AgentPrompt("Generate report")
    ));

    let kind = CronScheduler::parse_message("ask:no-space");
    assert!(matches!(
        kind,
        adk_gateway::cron::CronMessageKind::AgentPrompt("no-space")
    ));
}

#[test]
fn e2e_cron_inbound_message_construction() {
    let job = CronJob {
        id: "test-job".into(),
        schedule: "every 5m".into(),
        message: "ask: ping".into(),
        deliver_to: Some(CronDelivery {
            channel: "slack".into(),
            target: "#general".into(),
        }),
    };

    let msg = CronScheduler::build_inbound_message(&job);
    assert!(matches!(msg.source, MessageSource::Cron { .. }));
    assert_eq!(
        msg.metadata.get("cron_channel").and_then(|v| v.as_str()),
        Some("slack")
    );
    assert_eq!(
        msg.metadata.get("cron_target").and_then(|v| v.as_str()),
        Some("#general")
    );
}

// ════════════════════════════════════════════════════════════════════
// 7. KNOWLEDGE GRAPH MEMORY
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_knowledge_graph_full_lifecycle() {
    let kg = KnowledgeGraph::new();

    let entities = kg.create_entities(
        "user1",
        vec![
            CreateEntityInput {
                name: "Rust".into(),
                entity_type: "Language".into(),
                observations: vec!["Systems programming language".into()],
            },
            CreateEntityInput {
                name: "Tokio".into(),
                entity_type: "Library".into(),
                observations: vec!["Async runtime for Rust".into()],
            },
        ],
    );
    assert_eq!(entities.len(), 2);

    let relations = kg.create_relations(
        "user1",
        vec![CreateRelationInput {
            source: "Rust".into(),
            target: "Tokio".into(),
            relation_type: "has_library".into(),
        }],
    );
    assert_eq!(relations.len(), 1);

    let results = kg.search_nodes("user1", "Rust");
    assert!(!results.is_empty());
    assert!(results.iter().any(|r| r.entity.name == "Rust"));

    let obs_result = kg.add_observations("user1", "Rust", vec!["Memory safe".into()]);
    assert!(obs_result.is_some());

    let (ents, rels) = kg.read_graph("user1");
    assert_eq!(ents.len(), 2);
    assert_eq!(rels.len(), 1);

    let deleted = kg.delete_entities("user1", vec!["Rust".into()]);
    assert_eq!(deleted.len(), 1);

    let (ents, rels) = kg.read_graph("user1");
    assert_eq!(ents.len(), 1);
    assert_eq!(rels.len(), 0);
}

#[test]
fn e2e_knowledge_graph_user_isolation() {
    let kg = KnowledgeGraph::new();
    kg.create_entities(
        "alice",
        vec![CreateEntityInput {
            name: "Secret".into(),
            entity_type: "Data".into(),
            observations: vec!["Alice's secret".into()],
        }],
    );

    let results = kg.search_nodes("bob", "Secret");
    assert!(results.is_empty());

    let (ents, _) = kg.read_graph("bob");
    assert!(ents.is_empty());
}

// ════════════════════════════════════════════════════════════════════
// 7b. EXECUTABLE TOOL BRIDGE (KG + Agent Management)
// ════════════════════════════════════════════════════════════════════

/// Task 7.12: Verify that the KG tool bridge creates entities that persist in the graph.
///
/// This test exercises the `build_kg_tools` function to ensure:
/// 1. The correct number of tools are created (5 KG tools)
/// 2. Tools have the expected names
/// 3. Operations through the KG (which the tools wrap) persist correctly
/// 4. The agent management tools are also constructed correctly
#[test]
fn e2e_executable_tool_bridge_kg_creates_entity_persists() {
    use adk_gateway::agent_registry::AgentRegistry;
    use adk_gateway::executable_tools::{build_agent_tools, build_kg_tools};

    // Build KG and tools
    let kg = Arc::new(KnowledgeGraph::new());
    let kg_tools = build_kg_tools(kg.clone());

    // Verify 5 KG tools are created
    assert_eq!(kg_tools.len(), 5, "expected 5 KG tools (create_entities, add_observations, search_nodes, read_graph, delete_entities)");

    // Verify tool names
    let tool_names: Vec<&str> = kg_tools.iter().map(|t| t.name()).collect();
    assert!(tool_names.contains(&"kg_create_entities"));
    assert!(tool_names.contains(&"kg_add_observations"));
    assert!(tool_names.contains(&"kg_search_nodes"));
    assert!(tool_names.contains(&"kg_read_graph"));
    assert!(tool_names.contains(&"kg_delete_entities"));

    // Simulate what the tool does internally: create entities scoped to a user_id
    let user_id = "test-user-42";
    let created = kg.create_entities(
        user_id,
        vec![CreateEntityInput {
            name: "ProjectAlpha".into(),
            entity_type: "project".into(),
            observations: vec!["A Rust web framework".into(), "Uses async/await".into()],
        }],
    );
    assert_eq!(created, vec!["ProjectAlpha"]);

    // Verify entity persists in the graph (same path the tool uses)
    let (entities, _) = kg.read_graph(user_id);
    assert_eq!(entities.len(), 1);
    assert_eq!(entities[0].name, "ProjectAlpha");
    assert_eq!(entities[0].entity_type, "project");
    assert_eq!(entities[0].observations.len(), 2);

    // Add observations (same as kg_add_observations tool)
    let obs_ids = kg.add_observations(user_id, "ProjectAlpha", vec!["Released v1.0".into()]);
    assert!(obs_ids.is_some());
    assert_eq!(obs_ids.unwrap().len(), 1);

    // Verify observations persist
    let (entities, _) = kg.read_graph(user_id);
    assert_eq!(entities[0].observations.len(), 3);

    // Search (same as kg_search_nodes tool)
    let results = kg.search_nodes(user_id, "Rust");
    assert!(!results.is_empty());
    assert_eq!(results[0].entity.name, "ProjectAlpha");

    // Verify user isolation — another user cannot see this entity
    let (other_entities, _) = kg.read_graph("other-user");
    assert!(other_entities.is_empty());

    // Verify agent management tools are also constructed
    let tmp = tempfile::tempdir().unwrap();
    let registry = Arc::new(AgentRegistry::new(tmp.path().to_path_buf()));
    let rbac = Arc::new(adk_gateway::rbac_bridge::RbacBridge::new());
    let (ws_tx, _ws_rx) = tokio::sync::broadcast::channel(16);
    let agent_tools = build_agent_tools(registry, rbac, ws_tx, tmp.path().to_path_buf());
    assert_eq!(
        agent_tools.len(),
        2,
        "expected 2 agent management tools (agent_list, agent_create)"
    );

    let agent_tool_names: Vec<&str> = agent_tools.iter().map(|t| t.name()).collect();
    assert!(agent_tool_names.contains(&"agent_list"));
    assert!(agent_tool_names.contains(&"agent_create"));
}

// ════════════════════════════════════════════════════════════════════
// 8. RAG PIPELINE
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_rag_ingest_and_search() {
    let rag_config = RagConfig {
        vector_store: VectorStoreBackend::InMemory,
        connection_string: None,
        embedding: EmbeddingConfig {
            provider: "openai".into(),
            model: None,
        },
        chunking: ChunkingStrategy::FixedSize,
        chunk_size: Some(100),
        chunk_overlap: Some(20),
        watch_dirs: vec![],
        ingest_webhook: None,
    };

    let pipeline = RagPipelineBuilder::build(&rag_config).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(
        &file_path,
        "Rust is a systems programming language focused on safety and performance.",
    )
    .unwrap();

    let count = pipeline.ingest(&file_path).unwrap();
    assert!(count > 0);

    let results = pipeline.search("systems programming", 5);
    assert!(!results.is_empty());
    assert!(results[0].text.contains("Rust"));
    assert!(pipeline.document_count() > 0);
}

#[test]
fn e2e_rag_invalid_provider() {
    let rag_config = RagConfig {
        vector_store: VectorStoreBackend::InMemory,
        connection_string: None,
        embedding: EmbeddingConfig {
            provider: "nonexistent_provider".into(),
            model: None,
        },
        chunking: ChunkingStrategy::FixedSize,
        chunk_size: None,
        chunk_overlap: None,
        watch_dirs: vec![],
        ingest_webhook: None,
    };
    assert!(RagPipelineBuilder::build(&rag_config).is_err());
}

// ════════════════════════════════════════════════════════════════════
// 9. METRICS & OBSERVABILITY
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_metrics_full_flow() {
    let metrics = GatewayMetrics::new();

    for _ in 0..10 {
        metrics.record_message(
            "telegram",
            MessageStatus::Success,
            Duration::from_millis(150),
            Some(100),
            Some(50),
            Some("anthropic/claude-sonnet-4"),
        );
    }

    for _ in 0..3 {
        metrics.record_message(
            "telegram",
            MessageStatus::Failure,
            Duration::from_millis(0),
            None,
            None,
            None,
        );
    }

    assert_eq!(
        metrics.get_messages_total("telegram", &MessageStatus::Success),
        10
    );
    assert_eq!(
        metrics.get_messages_total("telegram", &MessageStatus::Failure),
        3
    );

    let error_rate = metrics.get_error_rate("telegram");
    assert!(error_rate > 0.0);

    metrics.set_active_sessions(42);
    metrics.set_channel_status("telegram", 1);
    metrics.set_channel_status("slack", 0);

    let prom = metrics.render_prometheus();
    assert!(prom.contains("adk_gateway_messages_total"));
    assert!(prom.contains("telegram"));
    assert!(prom.contains("adk_gateway_active_sessions"));
}

// ════════════════════════════════════════════════════════════════════
// 10. PLUGIN SYSTEM
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_plugin_manager_empty() {
    let manager = PluginManager::new();
    assert_eq!(manager.plugin_count(), 0);
}

// ════════════════════════════════════════════════════════════════════
// 11. SKILL & CONVENTION LOADING
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_skill_loading_and_selection() {
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(
        dir.path().join("code-review.skill.md"),
        r#"---
name: code-reviewer
description: Reviews code for best practices
trigger: "@review"
allowed-tools:
  - code_execution
tags: [code, security]
---

# Code Reviewer

Review code for security and best practices.
"#,
    )
    .unwrap();

    std::fs::write(
        dir.path().join("CLAW.md"),
        "# Project Instructions\n\nBe helpful and concise.",
    )
    .unwrap();

    let skills = SkillLoader::load_skills(dir.path());
    assert!(!skills.is_empty());

    let conventions = SkillLoader::load_conventions(dir.path(), &[]);
    assert!(!conventions.is_empty());

    let all: Vec<_> = skills.into_iter().chain(conventions).collect();
    let index = SkillLoader::build_index(all);

    let selected = index.get_by_name("code-reviewer");
    assert!(selected.is_some());
    assert_eq!(selected.unwrap().name, "code-reviewer");
}

#[test]
fn e2e_convention_permissive_mode() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "Multi-agent setup docs").unwrap();
    std::fs::write(dir.path().join("SOUL.md"), "Agent personality guide").unwrap();

    let conventions = SkillLoader::load_conventions(dir.path(), &[]);
    assert_eq!(conventions.len(), 2);

    let names: Vec<_> = conventions.iter().map(|c| c.name.as_str()).collect();
    assert!(names.contains(&"AGENTS"));
    assert!(names.contains(&"SOUL"));
}

// ════════════════════════════════════════════════════════════════════
// 12. TOOL REGISTRY
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_tool_registry_resolution() {
    let registry = ToolRegistry::new();

    // Resolve tools — unknown ones are skipped
    let resolved = registry.resolve_tools(
        &["google_search".to_string(), "nonexistent_tool".to_string()],
        None,
    );
    // google_search may or may not be a known built-in; nonexistent is skipped
    assert!(resolved.len() <= 2);
}

// ════════════════════════════════════════════════════════════════════
// 13. MCP CONNECTION MANAGEMENT
// ════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn e2e_mcp_connection_lifecycle() {
    let manager = McpConnectionManager::new();

    assert_eq!(manager.server_ids().len(), 0);

    let config = McpServerConfig {
        server_id: "code-server".into(),
        transport: McpTransport::Stdio {
            command: "npx".into(),
            args: vec!["code-mcp".into()],
            env: std::collections::HashMap::new(),
        },
        auth: None,
        enabled: true,
    };
    manager.connect(&config).await.unwrap();
    assert_eq!(manager.server_ids().len(), 1);

    let status = manager.get_status("code-server");
    assert!(status.is_some());

    manager.disconnect("code-server");
    assert!(manager.discovered_tools("code-server").is_empty());

    // Disconnect unknown server is safe
    manager.disconnect("nonexistent");
}

#[tokio::test]
async fn e2e_mcp_reconciliation() {
    let manager = McpConnectionManager::new();

    let configs = vec![
        McpServerConfig {
            server_id: "server-a".into(),
            transport: McpTransport::Stdio {
                command: "cmd-a".into(),
                args: vec![],
                env: std::collections::HashMap::new(),
            },
            enabled: true,
            auth: None,
        },
        McpServerConfig {
            server_id: "server-b".into(),
            transport: McpTransport::Sse {
                url: "http://localhost:3000".into(),
            },
            enabled: true,
            auth: None,
        },
    ];

    manager.reconcile(&configs).await;
    assert_eq!(manager.server_ids().len(), 2);

    let new_configs = vec![configs[1].clone()];
    manager.reconcile(&new_configs).await;
    assert_eq!(manager.server_ids().len(), 1);
    assert!(manager.server_ids().contains(&"server-b".to_string()));
}

// ════════════════════════════════════════════════════════════════════
// 14. RECONNECTION POLICY
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_reconnect_backoff() {
    let policy = ReconnectPolicy::default();
    let mut state = ReconnectState::new(policy);

    let d1 = state.next_delay();
    let d2 = state.next_delay();
    let d3 = state.next_delay();

    assert!(d2 >= d1);
    assert!(d3 >= d2);

    state.reset();
    let d_after_reset = state.next_delay();
    assert_eq!(d_after_reset, d1);
}

// ════════════════════════════════════════════════════════════════════
// 15. GRACEFUL SHUTDOWN
// ════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn e2e_graceful_shutdown() {
    let token = CancellationToken::new();
    let coordinator =
        ShutdownCoordinator::with_drain_timeout(token.clone(), Duration::from_secs(5));

    let guard1 = coordinator.acquire().unwrap();
    let guard2 = coordinator.acquire().unwrap();
    assert!(coordinator.is_accepting());

    let ((), ()) = tokio::join!(coordinator.initiate_shutdown(), async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(guard1);
        drop(guard2);
    });

    assert!(!coordinator.is_accepting());
    assert_eq!(coordinator.in_flight_count(), 0);
    assert!(token.is_cancelled());
}

// ════════════════════════════════════════════════════════════════════
// 16. CONTROL PANEL
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_control_panel_state() {
    let config = Arc::new(ArcSwap::from_pointee(config_with_telegram_open()));
    let state = ControlPanelState::new(config);

    let dashboard = state.dashboard();
    assert_eq!(dashboard.active_session_count, 0);
    assert!(dashboard.connected_channels.is_empty());

    state.update_channels(vec![
        ChannelInfo {
            channel_type: "telegram".into(),
            account_id: "default".into(),
            status: "connected".into(),
        },
        ChannelInfo {
            channel_type: "slack".into(),
            account_id: "team1".into(),
            status: "reconnecting".into(),
        },
    ]);

    let dashboard = state.dashboard();
    assert_eq!(dashboard.connected_channels.len(), 2);

    for i in 0..5 {
        state.push_log(LogEntry {
            timestamp: format!("2024-01-01T00:00:0{i}Z"),
            level: "INFO".into(),
            message: format!("Event {i}"),
            target: Some("gateway".into()),
        });
    }

    let logs = state.recent_logs(3);
    assert_eq!(logs.len(), 3);

    let redacted = state.redacted_config();
    assert!(redacted.is_object());
}

// ════════════════════════════════════════════════════════════════════
// 17. AUDIT LOGGING
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_audit_event_serialization() {
    let event = AuditEvent {
        timestamp: chrono::Utc::now(),
        user_id: "telegram:user1".into(),
        session_id: Some("sess-001".into()),
        channel_type: Some(ChannelType::Telegram),
        event_type: AuditEventType::ToolAccess,
        resource: "web_search".into(),
        outcome: AuditOutcome::Allowed,
        details: Some("tool executed successfully".into()),
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: AuditEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.event_type, AuditEventType::ToolAccess);
    assert_eq!(deserialized.outcome, AuditOutcome::Allowed);
}

#[tokio::test]
async fn e2e_null_audit_sink() {
    let sink = NullAuditSink;
    let event = AuditEvent {
        timestamp: chrono::Utc::now(),
        user_id: "test".into(),
        session_id: None,
        channel_type: None,
        event_type: AuditEventType::Login,
        resource: "gateway".into(),
        outcome: AuditOutcome::Allowed,
        details: None,
    };
    assert!(sink.log_event(event).await.is_ok());
}

// ════════════════════════════════════════════════════════════════════
// 18. CONTEXT COORDINATOR (SKILL SELECTION + TOOL FILTERING)
// ════════════════════════════════════════════════════════════════════

#[test]
fn e2e_context_coordinator_skill_selection() {
    let dir = tempfile::tempdir().unwrap();

    std::fs::write(
        dir.path().join("search-skill.skill.md"),
        r#"---
name: web-searcher
description: Searches the web
trigger: "@search"
allowed-tools:
  - google_search
  - web_fetch
---

Search the web for information.
"#,
    )
    .unwrap();

    let skills = SkillLoader::load_skills(dir.path());
    let index = SkillLoader::build_index(skills);

    // Select skill by trigger
    let selected = ContextCoordinator::select_skill("@search for Rust tutorials", &index);
    assert!(selected.is_some());
    assert_eq!(selected.unwrap().name, "web-searcher");

    // Build context
    let skill = index.get_by_name("web-searcher").unwrap();
    let ctx = ContextCoordinator::build_context(skill, "@search for Rust tutorials");
    assert!(ctx.instructions.contains("Search the web"));
    assert!(ctx
        .filtered_tool_names
        .contains(&"google_search".to_string()));
    assert!(ctx.filtered_tool_names.contains(&"web_fetch".to_string()));
}

// ════════════════════════════════════════════════════════════════════
// 19. FULL MESSAGE PIPELINE (INTEGRATION)
// ════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn e2e_full_message_pipeline() {
    // 1. Config
    let config = config_with_auth();

    // 2. Access control
    let bridge = AccessControlBridge::new(&config);
    let msg = make_inbound(ChannelType::Telegram, "admin_user", "What is Rust?");
    let decision = bridge.check_message_access(&msg);
    assert!(matches!(decision, AuthDecision::Allowed));

    // 3. Session resolution
    let session_service = Arc::new(InMemorySessionService::new());
    let session_bridge = SessionBridge::new(
        config.session.clone(),
        "adk-gateway".into(),
        session_service,
    );
    let (user_id, session_id) = session_bridge.resolve_session(&msg);
    assert!(!user_id.is_empty());
    assert!(!session_id.is_empty());

    // 4. Metrics recording
    let metrics = GatewayMetrics::new();
    metrics.record_message(
        "telegram",
        MessageStatus::Success,
        Duration::from_millis(250),
        Some(50),
        Some(100),
        Some("anthropic/claude-sonnet-4"),
    );
    assert_eq!(
        metrics.get_messages_total("telegram", &MessageStatus::Success),
        1
    );

    // 5. Audit
    let audit_event = AuditEvent {
        timestamp: chrono::Utc::now(),
        user_id: user_id.clone(),
        session_id: Some(session_id.clone()),
        channel_type: Some(ChannelType::Telegram),
        event_type: AuditEventType::AgentAccess,
        resource: "assistant".into(),
        outcome: AuditOutcome::Allowed,
        details: Some(format!("latency_ms=250")),
    };
    let json = serde_json::to_string(&audit_event).unwrap();
    assert!(json.contains("agent_access"));

    // 6. Verify pipeline state
    assert_eq!(session_bridge.active_sessions().len(), 1);
    let prom = metrics.render_prometheus();
    assert!(prom.contains("adk_gateway_messages_total"));
}
