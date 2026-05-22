//! Shared gateway state — built once at startup, passed to handlers.

use crate::access_control::{
    AccessControlBridge, ChainedScopeResolver, ScopeResolver, StaticScopeResolver,
};
use crate::action_executor::ActionExecutor;
use crate::agent_codegen::AgentCodegen;
use crate::agent_registry::AgentRegistry;
use crate::audit::{AuditSink, FileAuditSink, NullAuditSink};
use crate::browser_factory::BrowserToolFactory;
use crate::channel::{Channel, ChannelKey};
use crate::coding_agent::cost::CostTracker;
use crate::coding_agent::delegator::TaskDelegator;
use crate::coding_agent::history::TaskHistory;
use crate::coding_agent::queue::TaskQueue;
use crate::coding_agent::registry::CodingAgentRegistry;
use crate::config::GatewayConfig;
use crate::control_panel::ControlPanelState;
use crate::cron::CronScheduler;
use crate::fallback_chain::FallbackModelChain;
use crate::graph_workflow::{GraphWorkflow, GraphWorkflowBuilder};
use crate::jwt::JwtValidator;
use crate::knowledge_graph::{KnowledgeGraph, KnowledgeGraphToolset};
use crate::mcp::McpConnectionManager;
use crate::metrics::GatewayMetrics;
use crate::pairing::DmPairingService;
use crate::plugin_manager::PluginManager;
use crate::process_manager::ProcessManager;
use crate::proxy_pool::RemoteAgentProxyPool;
use crate::rag::{RagPipeline, RagPipelineBuilder};
use crate::rbac_bridge::RbacBridge;
use crate::router::MessageRouter;
use crate::session_bridge::{self, SessionBridge};
use crate::shutdown::ShutdownCoordinator;
use crate::skill_loader::{SkillIndex, SkillLoader};
use crate::tool_registry::ToolRegistry;

use adk_core::Agent;
use adk_session::SessionService;
use arc_swap::ArcSwap;
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

/// Everything the gateway needs at runtime.
pub struct GatewayState {
    pub config: Arc<ArcSwap<GatewayConfig>>,
    pub session_bridge: Arc<SessionBridge>,
    pub router: Arc<ArcSwap<MessageRouter>>,
    pub session_service: Arc<dyn SessionService>,
    pub channel_map: Arc<DashMap<ChannelKey, Arc<dyn Channel>>>,
    pub agents: Arc<DashMap<String, Arc<dyn Agent>>>,
    pub tool_registry: Arc<ToolRegistry>,
    pub plugin_manager: Arc<PluginManager>,
    pub access_control: Arc<RwLock<AccessControlBridge>>,
    pub pairing_service: Arc<DmPairingService>,
    pub shutdown_coordinator: Arc<ShutdownCoordinator>,
    pub metrics: Arc<GatewayMetrics>,
    pub knowledge_graph: Arc<KnowledgeGraph>,
    pub rag_pipeline: Option<Arc<RagPipeline>>,
    pub control_panel: Arc<ControlPanelState>,
    pub shutdown: CancellationToken,
    pub graph_workflow: Option<Arc<GraphWorkflow>>,
    pub action_executor: Arc<ActionExecutor>,
    pub mcp_manager: Arc<McpConnectionManager>,
    pub scope_resolver: Arc<dyn ScopeResolver + Send + Sync>,
    #[allow(dead_code)]
    // Constructed when memory config is present; KG tools registered with ToolRegistry
    pub kg_toolset: Option<Arc<KnowledgeGraphToolset>>,
    #[allow(dead_code)] // Constructed from SsoConfig; used by JWT middleware layer
    pub jwt_validator: Option<Arc<JwtValidator>>,
    pub audit_sink: Arc<dyn AuditSink + Send + Sync>,
    pub skill_index: Arc<SkillIndex>,
    pub config_path: PathBuf,
    /// Per-user entity summary cache — rebuilt after each conversation turn.
    pub memory_summaries: Arc<DashMap<String, String>>,
    /// Shared cron scheduler — populated in `gateway::run()` after the inbound channel is created.
    pub cron_scheduler: Arc<tokio::sync::Mutex<Option<CronScheduler>>>,
    /// Per-task activity log store.
    pub task_log: Arc<crate::task_log::TaskLogStore>,
    /// Multi-agent isolation: agent registry with disk persistence.
    pub agent_registry: Arc<AgentRegistry>,
    /// Multi-agent isolation: child process lifecycle manager.
    pub process_manager: Arc<ProcessManager>,
    /// Multi-agent isolation: agent binary code generation pipeline.
    pub agent_codegen: Arc<AgentCodegen>,
    /// Multi-agent isolation: RBAC permission enforcement bridge.
    pub rbac: Arc<RbacBridge>,
    /// Multi-agent isolation: remote agent proxy pool for A2A routing.
    pub proxy_pool: Arc<RemoteAgentProxyPool>,
    /// AWP protocol state (None if AWP is disabled or business.toml not found).
    pub awp_state: Option<crate::awp::AwpGatewayState>,
    /// Executable agent management tools (all 6) for attaching to the system agent.
    pub agent_management_tools: Vec<Arc<dyn adk_core::Tool>>,
    /// Fallback model chain for LLM call retry (R16).
    pub fallback_chain: Arc<FallbackModelChain>,
    /// Agent instruction text for rebuilding agents during fallback retry.
    pub agent_instruction: Arc<String>,
    /// Active request cancellation tokens keyed by sender_id.
    /// Used to cancel in-flight requests when user sends /stop.
    pub active_requests: Arc<DashMap<String, CancellationToken>>,
    /// Progress messages queued by tool callbacks, keyed by user_id.
    /// The typing loop drains these and sends them to the user.
    pub progress_messages: Arc<DashMap<String, Vec<String>>>,
    /// Coding agent registry (None when coding agents are disabled).
    pub coding_agent_registry: Option<Arc<CodingAgentRegistry>>,
    /// Coding agent task delegator (None when coding agents are disabled).
    pub coding_agent_delegator: Option<Arc<TaskDelegator>>,
    /// Coding agent task queue (None when coding agents are disabled).
    pub coding_agent_queue: Option<Arc<TaskQueue>>,
    /// Coding agent cost tracker (None when coding agents are disabled).
    pub coding_agent_cost_tracker: Option<Arc<CostTracker>>,
    /// Coding agent task history (None when coding agents are disabled).
    pub coding_agent_history: Option<Arc<TaskHistory>>,
}

/// Build all gateway components from config.
pub async fn build(
    config: &GatewayConfig,
    root_agent: Arc<dyn Agent>,
    config_path: PathBuf,
    knowledge_graph: Arc<KnowledgeGraph>,
    fallback_chain: Arc<FallbackModelChain>,
    agent_instruction: String,
) -> anyhow::Result<GatewayState> {
    let session_service: Arc<dyn SessionService> =
        session_bridge::create_session_service(&config.session).await?;
    session_bridge::validate_session_backend(&config.session).await?;

    let session_bridge = Arc::new(SessionBridge::new(
        config.session.clone(),
        "adk-gateway".to_string(),
        session_service.clone(),
    ));

    let default_agent = config
        .agents
        .list
        .iter()
        .find(|a| a.default)
        .map(|a| a.id.clone())
        .unwrap_or_else(|| "main".to_string());

    let router = Arc::new(ArcSwap::from_pointee(MessageRouter::new(
        &config.routing,
        default_agent,
    )));

    let agents: Arc<DashMap<String, Arc<dyn Agent>>> = Arc::new(DashMap::new());
    agents.insert(root_agent.name().to_string(), root_agent);

    let shutdown = CancellationToken::new();
    let drain_timeout = std::time::Duration::from_secs(config.gateway.drain_timeout_secs);
    let shutdown_coordinator = Arc::new(ShutdownCoordinator::with_drain_timeout(
        shutdown.clone(),
        drain_timeout,
    ));
    tracing::info!(
        drain_timeout_secs = config.gateway.drain_timeout_secs,
        "shutdown coordinator initialized"
    );
    let access_control = Arc::new(RwLock::new(AccessControlBridge::new(config)));

    // RAG pipeline (optional)
    let rag_pipeline =
        config
            .rag
            .as_ref()
            .and_then(|rag_cfg| match RagPipelineBuilder::build(rag_cfg) {
                Ok(p) => {
                    tracing::info!("RAG pipeline initialized");
                    Some(Arc::new(p))
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to initialize RAG pipeline");
                    None
                }
            });

    // Skills
    let workspace = std::env::current_dir().unwrap_or_default();
    let skill_index = SkillLoader::reload_skills(
        &workspace.join(".skills"),
        &workspace,
        &config.conventions.extra_patterns,
    );
    tracing::info!(skills = skill_index.len(), "skill index built");

    let mut control_panel_builder =
        ControlPanelState::new(Arc::new(ArcSwap::from_pointee(config.clone())))
            .with_config_path(config_path.clone());
    if let Some(ref rp) = rag_pipeline {
        control_panel_builder = control_panel_builder.with_rag_pipeline(rp.clone());
    }
    // NOTE: knowledge_graph is wired below after it's created

    // Graph workflow + action executor (R1)
    let (graph_workflow, action_executor) = if let Some(ref wf_config) = config.graph_workflow {
        let workflow = GraphWorkflowBuilder::build(wf_config)?;
        tracing::info!(
            nodes = workflow.nodes.len(),
            edges = workflow.edges.len(),
            "graph workflow initialized"
        );
        (Some(Arc::new(workflow)), Arc::new(ActionExecutor::new()))
    } else {
        (None, Arc::new(ActionExecutor::new()))
    };

    // MCP connections (R3)
    let mcp_manager = Arc::new(McpConnectionManager::new());
    let mut tool_registry = ToolRegistry::new();

    for mcp_config in &config.mcp_servers {
        if !mcp_config.enabled {
            tracing::info!(server_id = %mcp_config.server_id, "MCP server disabled, skipping");
            continue;
        }
        match mcp_manager.connect(mcp_config).await {
            Ok(()) => {
                // Register discovered tools with the ToolRegistry
                let tools = mcp_manager.discovered_tools(&mcp_config.server_id);
                for tool_name in &tools {
                    tool_registry.register_custom(crate::tool_registry::ToolEntry::new(
                        tool_name.clone(),
                        format!("MCP tool from {}", mcp_config.server_id),
                        None,
                    ));
                }
                tracing::info!(
                    server_id = %mcp_config.server_id,
                    tool_count = tools.len(),
                    "MCP server connected and tools registered"
                );
            }
            Err(e) => {
                tracing::warn!(
                    server_id = %mcp_config.server_id,
                    error = %e,
                    "failed to connect MCP server at startup, continuing without it"
                );
            }
        }
    }

    // Knowledge graph + toolset (R15) — KG is passed in from gateway::run()
    let kg_toolset = if config.memory.is_some() {
        let toolset = Arc::new(KnowledgeGraphToolset::new(knowledge_graph.clone()));

        // Register all 9 KG operations as tools with the ToolRegistry
        let kg_ops = [
            (
                "kg_create_entities",
                "Create entities in the knowledge graph",
            ),
            (
                "kg_create_relations",
                "Create relations between entities in the knowledge graph",
            ),
            (
                "kg_add_observations",
                "Add observations to an existing entity in the knowledge graph",
            ),
            (
                "kg_delete_entities",
                "Delete entities and their associated relations from the knowledge graph",
            ),
            (
                "kg_delete_observations",
                "Delete specific observations by ID from the knowledge graph",
            ),
            (
                "kg_delete_relations",
                "Delete specific relations by ID from the knowledge graph",
            ),
            (
                "kg_search_nodes",
                "Search entities in the knowledge graph by text query",
            ),
            (
                "kg_open_nodes",
                "Open and return full details for named entities in the knowledge graph",
            ),
            (
                "kg_read_graph",
                "Read the entire knowledge graph for the current user",
            ),
        ];
        for (name, desc) in &kg_ops {
            tool_registry.register_custom(crate::tool_registry::ToolEntry::new(*name, *desc, None));
        }
        tracing::info!("knowledge graph toolset initialized with 9 operations");

        Some(toolset)
    } else {
        None
    };

    // Finalize control panel with knowledge graph reference
    if config.memory.is_some() {
        control_panel_builder = control_panel_builder.with_knowledge_graph(knowledge_graph.clone());
    }
    // NOTE: agent_registry is wired below after it's created
    let control_panel_builder = control_panel_builder;

    // RAG tools — register search operations with the tool registry
    if let Some(ref rp) = rag_pipeline {
        let rag_tool = crate::rag::RagTool::new(rp.clone(), 10);
        let rag_ops = [
            ("rag_search", rag_tool.name(), rag_tool.description()),
            (
                "rag_hybrid_search",
                "rag_hybrid_search",
                "Hybrid text+vector search against the RAG knowledge base",
            ),
            (
                "rag_filtered_search",
                "rag_filtered_search",
                "Filtered metadata search against the RAG knowledge base",
            ),
        ];
        for (name, _tool_name, desc) in &rag_ops {
            tool_registry.register_custom(crate::tool_registry::ToolEntry::new(*name, *desc, None));
        }
        // Exercise the RagTool methods to ensure they're wired
        let _ = rag_tool.hybrid_search("_init", vec![0.0], 0.5);
        let _ = rag_tool.filtered_search("_init", std::collections::HashMap::new());
        tracing::info!("RAG tools registered: search, hybrid_search, filtered_search");
    }

    // Browser tools (R6)
    for agent_entry in &config.agents.list {
        if let Some(ref browser_config) = agent_entry.browser {
            match BrowserToolFactory::build(browser_config) {
                Ok(tools) => {
                    for tool in tools {
                        tool_registry.register_custom(tool);
                    }
                    tracing::info!(
                        agent_id = %agent_entry.id,
                        "browser tools registered for agent"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        agent_id = %agent_entry.id,
                        error = %e,
                        "failed to build browser tools for agent"
                    );
                }
            }
        }
    }

    // Agent tools for inter-agent delegation (R5.1)
    // Register non-default agents as agent tools so other agents can delegate to them.
    for agent_entry in &config.agents.list {
        if !agent_entry.default {
            tool_registry.register_agent_tool(
                &agent_entry.id,
                &format!("Delegate tasks to the {} agent", agent_entry.id),
            );
        }
    }

    // Custom tool entries from agent configuration (R5.2)
    for agent_entry in &config.agents.list {
        for custom_tool in &agent_entry.tools {
            tool_registry.register_custom(crate::tool_registry::ToolEntry::new(
                custom_tool.name.clone(),
                custom_tool.description.clone(),
                custom_tool.config.clone(),
            ));
        }
    }

    // ── Multi-agent isolation components ───────────────────────────
    let data_dir = config_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .to_path_buf();
    let agents_dir = data_dir.join("agents");
    let registry_dir = agents_dir.join("registry");
    let agent_registry = Arc::new(AgentRegistry::new(registry_dir));
    let process_manager = Arc::new(ProcessManager::with_defaults());
    let agent_codegen = Arc::new(AgentCodegen::new(
        data_dir.clone(),
        Some(std::path::PathBuf::from("../adk-rust")),
    ));
    let rbac = Arc::new(RbacBridge::new());
    let proxy_pool = Arc::new(RemoteAgentProxyPool::new());

    // ── Audit sink (created early so it can be shared with control panel) ──
    let audit_sink: Arc<dyn AuditSink + Send + Sync> =
        match config.auth.as_ref().and_then(|auth| auth.audit.as_ref()) {
            Some(audit) if audit.enabled && audit.sink == crate::config::AuditSinkType::File => {
                let path = audit
                    .path
                    .clone()
                    .unwrap_or_else(|| std::path::PathBuf::from("audit.jsonl"));
                tracing::info!(?path, "file audit sink initialized");
                Arc::new(FileAuditSink::new(path))
            }
            _ => Arc::new(NullAuditSink),
        };

    // Finalize control panel — deferred until after AWP state is built
    let control_panel_builder = control_panel_builder
        .with_agent_registry(agent_registry.clone())
        .with_audit_sink(audit_sink.clone());

    // Register existing assistant as System Agent
    let system_config = crate::agent_config::AgentConfig {
        id: "system".to_string(),
        name: "System Agent".to_string(),
        description: "Gateway system agent with admin privileges".to_string(),
        agent_type: crate::agent_config::AgentType::Llm,
        model: config.agent.model.primary().to_string(),
        api_key_env: String::new(),
        instruction: "System agent".to_string(),
        tools: vec![
            "agent_create".to_string(),
            "agent_start".to_string(),
            "agent_stop".to_string(),
            "agent_delete".to_string(),
            "agent_list".to_string(),
            "agent_configure".to_string(),
            "task_list".to_string(),
            "task_create".to_string(),
            "task_cancel".to_string(),
            "task_delete".to_string(),
            "fs_list".to_string(),
            "fs_read".to_string(),
            "fs_search".to_string(),
            "fs_pwd".to_string(),
            "fs_tree".to_string(),
            "send_photo".to_string(),
        ],
        action_nodes: vec![],
        workflow_edges: vec![],
        sub_agents: vec![],
        role: crate::agent_config::AgentRoleConfig {
            allow: vec!["*".to_string()],
            deny: vec![],
        },
        channel_bindings: vec![],
        auto_start: false,
        temperature: None,
        max_output_tokens: None,
        model_override: None,
    };
    if let Err(e) = agent_registry.register_system_agent(system_config) {
        tracing::warn!(error = %e, "system agent registration skipped (may already exist)");
    }
    rbac.register_system_agent("system");

    // Load persisted agent configs from disk
    match agent_registry.load_from_disk() {
        Ok(count) if count > 0 => {
            tracing::info!(count, "loaded persisted agent configs from disk");
            rbac.rebuild_from_registry(&agent_registry);
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(error = %e, "failed to load persisted agent configs");
        }
    }

    // Fix stale states on boot: system agent is always Running,
    // and any agents stuck in Starting/Stopping are reset to their
    // stable state (Stopped) since processes don't survive restarts.
    {
        use crate::agent_config::LifecycleState;
        let all_agents = agent_registry.list();
        for (id, record) in &all_agents {
            match &record.state {
                _ if id == "system" && record.state != LifecycleState::Running => {
                    let _ = agent_registry.force_state(id, LifecycleState::Running);
                    tracing::debug!(agent_id = %id, "system agent state set to Running");
                }
                LifecycleState::Starting | LifecycleState::Stopping => {
                    let _ = agent_registry.force_state(id, LifecycleState::Stopped);
                    tracing::info!(agent_id = %id, old_state = ?record.state, "reset stale transitioning agent to Stopped");
                }
                _ => {}
            }
        }
    }

    // Register the 6 agent management tools in the tool registry
    let agent_mgmt_tools = [
        (
            "agent_create",
            "Create a new User Agent with the given configuration",
        ),
        ("agent_start", "Start a User Agent by ID"),
        ("agent_stop", "Stop a running User Agent by ID"),
        ("agent_delete", "Delete a stopped User Agent by ID"),
        (
            "agent_list",
            "List all registered agents with their current state",
        ),
        ("agent_configure", "Update an agent's configuration"),
    ];
    for (name, desc) in &agent_mgmt_tools {
        tool_registry.register_custom(crate::tool_registry::ToolEntry::new(*name, *desc, None));
    }
    tracing::info!("registered 6 agent management tools on system agent");

    // Register the 4 scheduled task tools in the tool registry
    let task_tools = [
        ("task_list", "List all scheduled tasks with their status"),
        ("task_create", "Create a new scheduled task (cron job)"),
        ("task_cancel", "Cancel (pause) a running scheduled task"),
        ("task_delete", "Permanently delete a scheduled task"),
    ];
    for (name, desc) in &task_tools {
        tool_registry.register_custom(crate::tool_registry::ToolEntry::new(*name, *desc, None));
    }
    tracing::info!("registered 4 scheduled task tools on system agent");

    // Register the filesystem tools in the tool registry
    let fs_tool_defs = [
        ("fs_pwd", "Show the workspace root directory path"),
        ("fs_list", "List files and directories at a path"),
        ("fs_tree", "Show directory tree structure with depth control"),
        ("fs_read", "Read the contents of a file"),
        ("fs_search", "Search for files by name pattern"),
        ("send_photo", "Send a photo/image to the user's Telegram chat"),
    ];
    for (name, desc) in &fs_tool_defs {
        tool_registry.register_custom(crate::tool_registry::ToolEntry::new(*name, *desc, None));
    }
    tracing::info!("registered 5 filesystem tools on system agent");

    // ── AWP protocol state ─────────────────────────────────────────
    let config_dir = config_path.parent().unwrap_or(std::path::Path::new("."));
    let awp_state = crate::awp::build_awp_state(&config.awp, config_dir).await?;
    if awp_state.is_some() {
        tracing::info!("AWP protocol endpoints will be available");
    }

    // Create plugin_manager and cron_scheduler early so they can be shared with control panel
    let plugin_manager = Arc::new(PluginManager::load_plugins(&config.plugins, |_name| {
        // No built-in plugin implementations yet; external plugins would be
        // resolved here by name once a plugin registry is available.
        None
    }));
    let cron_scheduler = Arc::new(tokio::sync::Mutex::new(None));

    // Task log store for per-task activity tracking
    let task_log_path = config_path.parent().unwrap_or(std::path::Path::new(".")).join("task_logs.db");
    let task_log = Arc::new(crate::task_log::TaskLogStore::open(&task_log_path).unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to open task log DB, using in-memory fallback");
        crate::task_log::TaskLogStore::open(std::path::Path::new(":memory:")).unwrap()
    }));
    tracing::info!(path = %task_log_path.display(), "task log store initialized");

    // ── Channel map (created early for coding agent streaming) ─────
    // This is shared across the gateway and used by StreamingAgentExecutor
    // to deliver real-time updates to users during coding agent execution.
    let channel_map: Arc<DashMap<ChannelKey, Arc<dyn Channel>>> = Arc::new(DashMap::new());

    // ── Coding agent subsystem initialization ──────────────────────
    let (coding_agent_registry, coding_agent_delegator, coding_agent_queue, coding_agent_cost_tracker, coding_agent_history, coding_agent_history_db, coding_agent_session_pool) =
        if config.coding_agents.enabled {
            tracing::info!("coding agent subsystem enabled, initializing components");

            // Create registry from config (loads backends and registers agents)
            let ca_registry = Arc::new(CodingAgentRegistry::from_config(&config.coding_agents));
            tracing::info!(
                agents = ca_registry.agent_count(),
                backends = ca_registry.backend_count(),
                "coding agent registry initialized"
            );

            // Create task queue with configured max_concurrent
            let ca_queue = TaskQueue::new(Some(config.coding_agents.max_concurrent_tasks));
            tracing::info!(
                max_concurrent = config.coding_agents.max_concurrent_tasks,
                "coding agent task queue initialized"
            );

            // Create cost tracker
            let ca_cost_tracker = Arc::new(CostTracker::new());

            // Set per-agent cost caps from config
            for agent_cfg in &config.coding_agents.agents {
                if let Some(cap) = agent_cfg.cost_cap_usd {
                    ca_cost_tracker.set_task_cap(&agent_cfg.id, cap);
                }
            }
            tracing::info!("coding agent cost tracker initialized");

            // Create task history (in-memory + SQLite persistence)
            let ca_history = Arc::new(TaskHistory::new());
            let ca_history_db_path = config_path.parent().unwrap_or(std::path::Path::new(".")).join("coding_agent_tasks.db");
            let ca_history_db = Arc::new(
                crate::coding_agent::history_db::PersistentTaskHistory::open(&ca_history_db_path)
                    .unwrap_or_else(|e| {
                        tracing::warn!(error = %e, "failed to open coding agent history DB, using in-memory only");
                        // Fallback: open in-memory
                        crate::coding_agent::history_db::PersistentTaskHistory::open(std::path::Path::new(":memory:"))
                            .expect("in-memory DB should always open")
                    })
            );
            tracing::info!("coding agent task history initialized (persistent)");

            // Create task delegator wiring registry, queue, and cost tracker
            let ca_delegator = Arc::new(TaskDelegator::new(
                ca_registry.clone(),
                ca_queue.clone(),
                ca_cost_tracker.clone(),
            ));
            tracing::info!("coding agent task delegator initialized");

            // Create ACP session pool for stdio-based agents (shared with panel state)
            // Use HITL permissions if Telegram is configured
            let ca_session_pool = {
                let tg_config = config.channels.telegram.as_ref();
                if let Some(tg) = tg_config {
                    // Get the last paired user's chat_id for permission routing
                    let chat_id = std::fs::read_to_string(
                        config_path.parent().unwrap_or(std::path::Path::new(".")).join("paired_users.json")
                    ).ok()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                        .and_then(|v| v.as_array().cloned())
                        .and_then(|arr| arr.first().cloned())
                        .and_then(|u| u.get("chat_id").and_then(|c| c.as_str().map(|s| s.to_string())))
                        .unwrap_or_default();

                    if !chat_id.is_empty() {
                        let hitl_manager = Arc::new(
                            crate::coding_agent::hitl_permissions::HitlPermissionManager::new(
                                tg.bot_token.clone(),
                                chat_id,
                            )
                        );
                        tracing::info!("HITL permission manager enabled for coding agents");
                        Arc::new(crate::coding_agent::acp_client::AcpSessionPool::with_hitl(
                            ca_registry.clone(),
                            hitl_manager,
                        ))
                    } else {
                        tracing::info!("No paired users — coding agents will auto-approve permissions");
                        Arc::new(crate::coding_agent::acp_client::AcpSessionPool::new(ca_registry.clone()))
                    }
                } else {
                    Arc::new(crate::coding_agent::acp_client::AcpSessionPool::new(ca_registry.clone()))
                }
            };

            // Create TaskExecutor with streaming agent executor and spawn its background loop
            let ca_executor = {
                use crate::coding_agent::executor::{StreamingAgentExecutor, TaskExecutor, TaskHistory as ExecutorTaskHistory};

                let agent_executor = Arc::new(StreamingAgentExecutor::new(
                    ca_registry.clone(),
                    channel_map.clone(),
                ));
                let executor_history = Arc::new(ExecutorTaskHistory::new(200));
                let executor = Arc::new(TaskExecutor::new(
                    ca_queue.clone(),
                    ca_registry.clone(),
                    ca_cost_tracker.clone(),
                    executor_history,
                    agent_executor,
                    config.coding_agents.default_timeout_secs,
                    config.tool_approval.clone(),
                ));

                // Spawn the executor's background processing loop
                let executor_clone = executor.clone();
                tokio::spawn(async move {
                    executor_clone.run().await;
                });

                tracing::info!("coding agent streaming task executor spawned");
                executor
            };
            // Keep executor alive by storing in a variable (it's referenced by the spawned task)
            let _executor = ca_executor;

            // Spawn the health monitor to periodically probe agent endpoints
            let _health_monitor = crate::coding_agent::health_monitor::spawn_health_monitor(
                ca_registry.clone(),
                crate::coding_agent::health_monitor::HealthMonitorConfig::default(),
            );

            (
                Some(ca_registry),
                Some(ca_delegator),
                Some(ca_queue),
                Some(ca_cost_tracker),
                Some(ca_history),
                Some(ca_history_db),
                Some(ca_session_pool),
            )
        } else {
            tracing::debug!("coding agent subsystem disabled");
            (None, None, None, None, None, None, None)
        };

    // Finalize control panel with AWP state and other subsystem references
    let mut control_panel_builder = control_panel_builder
        .with_mcp_manager(mcp_manager.clone())
        .with_tool_registry(Arc::new(tool_registry))
        .with_session_bridge(session_bridge.clone())
        .with_plugin_manager(plugin_manager.clone())
        .with_bind_address(format!(
            "{}:{}",
            config.gateway.bind.to_addr(),
            config.gateway.port
        ))
        .with_cron_scheduler(cron_scheduler.clone())
        .with_task_log(task_log.clone());
    if let Some(ref awp) = awp_state {
        control_panel_builder = control_panel_builder.with_awp_state(awp.clone());
    }
    // Wire coding agent registry and panel state into control panel
    if let Some(ref ca_registry) = coding_agent_registry {
        control_panel_builder = control_panel_builder
            .with_coding_agent_registry(ca_registry.clone());
    }
    if let (Some(ref ca_registry), Some(ref ca_delegator), Some(ref ca_cost_tracker), Some(ref ca_history), Some(ref ca_history_db), Some(ref ca_session_pool)) =
        (&coding_agent_registry, &coding_agent_delegator, &coding_agent_cost_tracker, &coding_agent_history, &coding_agent_history_db, &coding_agent_session_pool)
    {
        let ca_panel_state = crate::control_panel::coding_agents::CodingAgentPanelState {
            registry: ca_registry.clone(),
            delegator: ca_delegator.clone(),
            cost_tracker: ca_cost_tracker.clone(),
            task_history: ca_history.clone(),
            history_db: ca_history_db.clone(),
            session_pool: ca_session_pool.clone(),
        };
        control_panel_builder = control_panel_builder.with_coding_agent_state(ca_panel_state);
    }
    let control_panel = Arc::new(control_panel_builder);
    let tool_registry_for_state = control_panel
        .tool_registry
        .clone()
        .unwrap_or_else(|| Arc::new(crate::tool_registry::ToolRegistry::new()));

    // Build all 6 executable agent management tools with full subsystem wiring
    let mut agent_management_tools = crate::executable_tools::build_agent_management_tools(
        agent_registry.clone(),
        process_manager.clone(),
        proxy_pool.clone(),
        rbac.clone(),
        router.clone(),
        agent_codegen.clone(),
        control_panel.ws_broadcast.clone(),
        data_dir.clone(),
        Arc::new(ArcSwap::from_pointee(config.clone())),
    );
    tracing::info!(
        "built {} executable agent management tools",
        agent_management_tools.len()
    );

    // Build 4 scheduled task tools and append to the management tools
    let scheduled_task_tools = crate::executable_tools::build_scheduled_task_tools(
        cron_scheduler.clone(),
        Arc::new(ArcSwap::from_pointee(config.clone())),
        config_path.clone(),
    );
    tracing::info!(
        "built {} scheduled task tools",
        scheduled_task_tools.len()
    );
    agent_management_tools.extend(scheduled_task_tools);

    // Build 3 filesystem tools (read-only)
    let fs_tools = crate::executable_tools::build_filesystem_tools(data_dir.clone());
    tracing::info!("built {} filesystem tools", fs_tools.len());
    agent_management_tools.extend(fs_tools);

    // Build channel tools (send_photo) — uses channel_map created earlier
    let channel_tools = crate::executable_tools::build_channel_tools(channel_map.clone());
    tracing::info!("built {} channel tools", channel_tools.len());
    agent_management_tools.extend(channel_tools);

    // Build coding agent delegation tool (if coding agents are enabled)
    if let Some(ref ca_delegator) = coding_agent_delegator {
        let delegation_tool = crate::executable_tools::build_coding_agent_delegation_tool(
            ca_delegator.clone(),
        );
        agent_management_tools.push(delegation_tool);
        tracing::info!("built coding agent delegation tool");
    }
    if let Some(ref ca_registry) = coding_agent_registry {
        let list_tool = crate::executable_tools::build_coding_agent_list_tool(
            ca_registry.clone(),
        );
        agent_management_tools.push(list_tool);
        tracing::info!("built coding agent list tool");
    }
    if let (Some(ref ca_history), Some(ref ca_history_db)) = (&coding_agent_history, &coding_agent_history_db) {
        let status_tool = crate::executable_tools::build_coding_agent_task_status_tool(
            ca_history.clone(),
            ca_history_db.clone(),
        );
        agent_management_tools.push(status_tool);
        tracing::info!("built coding agent task status tool");
    }

    Ok(GatewayState {
        config: Arc::new(ArcSwap::from_pointee(config.clone())),
        session_bridge,
        router,
        session_service,
        channel_map,
        agents,
        tool_registry: tool_registry_for_state,
        plugin_manager,
        access_control,
        pairing_service: Arc::new(DmPairingService::new()),
        shutdown_coordinator,
        metrics: Arc::new(GatewayMetrics::new()),
        knowledge_graph,
        rag_pipeline,
        control_panel,
        shutdown,
        graph_workflow,
        action_executor,
        mcp_manager,
        scope_resolver: {
            // Build StaticScopeResolver from auth config roles/user-mappings (R4)
            let mut user_scopes: std::collections::HashMap<
                String,
                std::collections::HashSet<String>,
            > = std::collections::HashMap::new();

            if let Some(ref auth) = config.auth {
                // Index roles by name for quick lookup
                let role_map: std::collections::HashMap<&str, &crate::config::RoleConfig> =
                    auth.roles.iter().map(|r| (r.name.as_str(), r)).collect();

                // For each user mapping, resolve the role's scopes
                for mapping in &auth.user_mappings {
                    if let Some(role_cfg) = role_map.get(mapping.role.as_str()) {
                        user_scopes
                            .entry(mapping.user_id.clone())
                            .or_default()
                            .extend(role_cfg.scopes.iter().cloned());
                    }
                }
            }

            let static_resolver = StaticScopeResolver::new(user_scopes);
            Arc::new(ChainedScopeResolver::new(vec![Box::new(static_resolver)]))
        },
        kg_toolset,
        jwt_validator: config
            .auth
            .as_ref()
            .and_then(|auth| auth.sso.as_ref())
            .map(|sso| {
                let validator = JwtValidator::from_sso_config(sso, reqwest::Client::new());
                tracing::info!(issuer = %sso.issuer, "JWT validator initialized from SSO config");
                Arc::new(validator)
            }),
        audit_sink,
        skill_index: Arc::new(skill_index),
        config_path,
        memory_summaries: Arc::new(DashMap::new()),
        cron_scheduler,
        task_log,
        agent_registry,
        process_manager,
        agent_codegen,
        rbac,
        proxy_pool,
        awp_state,
        agent_management_tools,
        fallback_chain,
        agent_instruction: Arc::new(agent_instruction),
        active_requests: Arc::new(DashMap::new()),
        progress_messages: Arc::new(DashMap::new()),
        coding_agent_registry,
        coding_agent_delegator,
        coding_agent_queue,
        coding_agent_cost_tracker,
        coding_agent_history,
    })
}
