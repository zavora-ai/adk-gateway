//! End-to-end demonstration of the multi-agent isolation system.
//!
//! Run with: cargo run --example multi_agent_demo
//!
//! This example walks through the complete lifecycle:
//!   1. Initialize all multi-agent components
//!   2. Register the System Agent with admin RBAC
//!   3. Create two User Agents (research + writer)
//!   4. Verify RBAC isolation (user agents can't call system tools)
//!   5. Set up channel routing (telegram → research, slack → writer)
//!   6. Simulate message routing resolution
//!   7. Demonstrate agent lifecycle transitions
//!   8. Persist and restore agents across "restarts"
//!   9. Clean up: stop and delete agents
//!
//! No real LLM calls or child processes are spawned — this exercises
//! the registry, RBAC, routing, codegen schema, and persistence layers.

use std::sync::Arc;

use adk_gateway::agent_codegen::AgentCodegen;
use adk_gateway::agent_config::*;
use adk_gateway::agent_registry::AgentRegistry;
use adk_gateway::process_manager::ProcessManager;
use adk_gateway::proxy_pool::RemoteAgentProxyPool;
use adk_gateway::rbac_bridge::{RbacBridge, SYSTEM_TOOLS};
use adk_gateway::router::MessageRouter;

fn main() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(run());
}

async fn run() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();
    let persist_dir = workspace.join("agents").join("registry");

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║   Multi-Agent Isolation — End-to-End Demo               ║");
    println!("╚══════════════════════════════════════════════════════════╝\n");

    // ── Step 1: Initialize components ──────────────────────────────
    println!("── Step 1: Initialize multi-agent components ──");
    let registry = Arc::new(AgentRegistry::new(persist_dir.clone()));
    let _process_manager = Arc::new(ProcessManager::with_defaults());
    let codegen = Arc::new(AgentCodegen::new(workspace.clone(), None));
    let rbac = Arc::new(RbacBridge::new());
    let proxy_pool = Arc::new(RemoteAgentProxyPool::new());

    println!("   ✓ AgentRegistry, ProcessManager, AgentCodegen, RbacBridge, ProxyPool\n");

    // ── Step 2: Register System Agent ──────────────────────────────
    println!("── Step 2: Register System Agent with admin role ──");
    let system_config = AgentConfig {
        id: "system".to_string(),
        name: "System Agent".to_string(),
        description: "Gateway admin agent".to_string(),
        agent_type: AgentType::Llm,
        model: "anthropic/claude-sonnet-4".to_string(),
        api_key_env: "ANTHROPIC_API_KEY".to_string(),
        instruction: "You manage the gateway and its agents.".to_string(),
        tools: SYSTEM_TOOLS.iter().map(|s| s.to_string()).collect(),
        action_nodes: vec![],
        workflow_edges: vec![],
        sub_agents: vec![],
        role: AgentRoleConfig {
            allow: vec!["*".to_string()],
            deny: vec![],
        },
        channel_bindings: vec![],
        auto_start: false,
        temperature: None,
        max_output_tokens: None,
        model_override: None,
    };
    registry.register_system_agent(system_config).unwrap();
    rbac.register_system_agent("system");
    println!("   ✓ System agent registered (admin role: AllTools + AllAgents)");
    println!(
        "   ✓ System agent is singleton: {}\n",
        registry.is_system_agent("system")
    );

    // ── Step 3: Create User Agents via tool API ────────────────────
    println!("── Step 3: Create User Agents ──");

    let research_config = AgentConfig {
        id: "research".to_string(),
        name: "Research Agent".to_string(),
        description: "Searches the web and analyzes data".to_string(),
        agent_type: AgentType::Llm,
        model: "anthropic/claude-sonnet-4".to_string(),
        api_key_env: "ANTHROPIC_API_KEY".to_string(),
        instruction: "You are a research assistant. Use web_search to find information."
            .to_string(),
        tools: vec!["web_search".to_string(), "code_exec".to_string()],
        action_nodes: vec![],
        workflow_edges: vec![],
        sub_agents: vec![],
        role: AgentRoleConfig {
            allow: vec!["web_search".to_string(), "code_exec".to_string()],
            deny: vec![],
        },
        channel_bindings: vec![ChannelBinding {
            channel_type: "telegram".to_string(),
            account_id: None,
            peer_filter: None,
        }],
        auto_start: true,
        temperature: Some(0.3),
        max_output_tokens: Some(8192),
        model_override: None,
    };

    registry.create_agent(research_config.clone()).unwrap();
    rbac.register_agent("research", &research_config.role);

    // Create workspace directories for the demo.
    let research_agent_dir = workspace.join("agents").join("research");
    std::fs::create_dir_all(research_agent_dir.join("context")).unwrap();
    std::fs::create_dir_all(research_agent_dir.join("data")).unwrap();
    std::fs::create_dir_all(research_agent_dir.join("src")).unwrap();
    // Write a placeholder context file.
    std::fs::write(
        research_agent_dir.join("context").join("PROFILE.md"),
        "# Research Agent\n",
    )
    .unwrap();
    std::fs::write(
        research_agent_dir.join("context").join("USER.md"),
        "# User\n",
    )
    .unwrap();
    std::fs::write(
        research_agent_dir.join("context").join("PROJECTS.md"),
        "# Projects\n",
    )
    .unwrap();
    std::fs::write(
        research_agent_dir.join("context").join("HABITS.md"),
        "# Habits\n",
    )
    .unwrap();
    std::fs::write(
        research_agent_dir.join("context").join("NOTES.md"),
        "# Notes\n",
    )
    .unwrap();
    std::fs::write(
        research_agent_dir.join("context").join("BOOTSTRAP.md"),
        "# Bootstrap\n",
    )
    .unwrap();

    println!("   ✓ Created: research (state: Created)");

    let writer_config = AgentConfig {
        id: "writer".to_string(),
        name: "Writer Agent".to_string(),
        description: "Drafts documents and reports".to_string(),
        agent_type: AgentType::Llm,
        model: "openai/gpt-4o".to_string(),
        api_key_env: "OPENAI_API_KEY".to_string(),
        instruction: "You are a technical writer. Draft clear documents.".to_string(),
        tools: vec!["document_create".to_string(), "markdown_render".to_string()],
        action_nodes: vec![],
        workflow_edges: vec![],
        sub_agents: vec![],
        role: AgentRoleConfig {
            allow: vec!["document_create".to_string(), "markdown_render".to_string()],
            deny: vec![],
        },
        channel_bindings: vec![ChannelBinding {
            channel_type: "slack".to_string(),
            account_id: Some("team-alpha".to_string()),
            peer_filter: None,
        }],
        auto_start: false,
        temperature: Some(0.7),
        max_output_tokens: Some(4096),
        model_override: None,
    };

    registry.create_agent(writer_config.clone()).unwrap();
    rbac.register_agent("writer", &writer_config.role);
    println!("   ✓ Created: writer (state: Created)");

    // Verify workspace directories
    let research_dir = workspace.join("agents").join("research");
    println!(
        "   ✓ Workspace: agents/research/context/ — {} files",
        std::fs::read_dir(research_dir.join("context"))
            .unwrap()
            .count()
    );
    println!();

    // ── Step 4: RBAC Isolation ─────────────────────────────────────
    println!("── Step 4: Verify RBAC permission isolation ──");

    // System agent can call everything
    println!("   System agent:");
    for tool in SYSTEM_TOOLS {
        let ok = rbac.check_tool("system", tool).is_ok();
        println!("     {} {}", if ok { "✓" } else { "✗" }, tool);
    }

    // User agents cannot call system tools
    println!("   Research agent:");
    println!(
        "     ✓ web_search: {}",
        rbac.check_tool("research", "web_search").is_ok()
    );
    println!(
        "     ✓ code_exec:  {}",
        rbac.check_tool("research", "code_exec").is_ok()
    );
    for tool in &["agent_create", "agent_delete", "agent_start"] {
        let denied = rbac.check_tool("research", tool).is_err();
        println!(
            "     {} {} (denied: {})",
            if denied { "🔒" } else { "⚠️" },
            tool,
            denied
        );
    }
    println!();

    // ── Step 5: Channel Routing ────────────────────────────────────
    println!("── Step 5: Set up channel → agent routing ──");

    let routing_config = adk_gateway::config::RoutingConfig { bindings: vec![] };
    let mut router = MessageRouter::new(&routing_config, "system".to_string());

    // Add agent bindings from their configs
    router.add_agent_bindings("research", &research_config.channel_bindings);
    router.add_agent_bindings("writer", &writer_config.channel_bindings);

    println!("   Routing table:");
    println!("     telegram:*          → research");
    println!("     slack:team-alpha    → writer");
    println!("     *                   → system (default)");
    println!();

    // ── Step 6: Simulate message routing ───────────────────────────
    println!("── Step 6: Simulate message routing resolution ──");

    // Build test messages for each channel
    let now = chrono::Utc::now();
    let tg_msg = adk_gateway::channel::InboundMessage {
        channel_type: adk_gateway::channel::ChannelType::Telegram,
        account_id: "default".to_string(),
        sender_id: "user123".to_string(),
        sender_name: Some("Alice".to_string()),
        text: "Search for Rust async patterns".to_string(),
        is_group: false,
        group_id: None,
        is_mention: false,
        platform_message_id: "1".to_string(),
        attachments: vec![],
        metadata: std::collections::HashMap::new(),
        source: adk_gateway::channel::MessageSource::Channel,
        timestamp: now,
    };
    let resolved = router.resolve_agent(&tg_msg);
    println!("   Telegram msg from Alice → agent: \"{}\"", resolved);

    let slack_msg = adk_gateway::channel::InboundMessage {
        channel_type: adk_gateway::channel::ChannelType::Slack,
        account_id: "team-alpha".to_string(),
        sender_id: "U456".to_string(),
        sender_name: Some("Bob".to_string()),
        text: "Draft a project summary".to_string(),
        is_group: false,
        group_id: None,
        is_mention: false,
        platform_message_id: "2".to_string(),
        attachments: vec![],
        metadata: std::collections::HashMap::new(),
        source: adk_gateway::channel::MessageSource::Channel,
        timestamp: now,
    };
    let resolved = router.resolve_agent(&slack_msg);
    println!(
        "   Slack msg from Bob (team-alpha) → agent: \"{}\"",
        resolved
    );

    let webhook_msg = adk_gateway::channel::InboundMessage {
        channel_type: adk_gateway::channel::ChannelType::Webhook,
        account_id: "".to_string(),
        sender_id: "api-client".to_string(),
        sender_name: None,
        text: "webhook payload".to_string(),
        is_group: false,
        group_id: None,
        is_mention: false,
        platform_message_id: "3".to_string(),
        attachments: vec![],
        metadata: std::collections::HashMap::new(),
        source: adk_gateway::channel::MessageSource::Channel,
        timestamp: now,
    };
    let resolved = router.resolve_agent(&webhook_msg);
    println!(
        "   Webhook msg (no binding) → agent: \"{}\" (fallback)",
        resolved
    );
    println!();

    // ── Step 7: Agent lifecycle transitions ────────────────────────
    println!("── Step 7: Agent lifecycle state machine ──");

    // Simulate starting the research agent
    registry
        .transition("research", LifecycleState::Starting)
        .unwrap();
    println!("   research: Created → Starting");

    registry
        .transition("research", LifecycleState::Running)
        .unwrap();
    proxy_pool.register("research", 19001);
    println!("   research: Starting → Running (port 19001)");

    // Verify proxy is available
    let proxy = proxy_pool.get("research").unwrap();
    println!(
        "   research proxy: {} at {}",
        proxy.agent_id(),
        proxy.agent_url()
    );

    // Simulate a crash → Error → restart
    registry
        .transition(
            "research",
            LifecycleState::Error {
                message: "health check failed 3x".to_string(),
            },
        )
        .unwrap();
    proxy_pool.remove("research");
    println!("   research: Running → Error (crash detected)");

    // Recover from error
    registry
        .transition("research", LifecycleState::Starting)
        .unwrap();
    println!("   research: Error → Starting (retry)");
    registry
        .transition("research", LifecycleState::Running)
        .unwrap();
    proxy_pool.register("research", 19001);
    println!("   research: Starting → Running (recovered)");

    // Invalid transition should fail
    let invalid = registry.transition("research", LifecycleState::Created);
    println!(
        "   research: Running → Created = {} (invalid)",
        if invalid.is_err() {
            "rejected ✓"
        } else {
            "BUG"
        }
    );
    println!();

    // ── Step 8: Codegen schema generation ──────────────────────────
    println!("── Step 8: Agent binary code generation ──");

    let schema = codegen.to_project_schema(&research_config);
    println!("   ProjectSchema for research agent:");
    println!("     agent_type: {}", schema.agent.agent_type);
    println!("     model: {}", schema.agent.model);
    println!(
        "     tools: {:?}",
        schema.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    println!("     temperature: {:?}", schema.agent.temperature);

    let graph_config = AgentConfig {
        id: "pipeline".to_string(),
        name: "Pipeline Agent".to_string(),
        description: "Data processing pipeline".to_string(),
        agent_type: AgentType::Graph,
        model: "anthropic/claude-sonnet-4".to_string(),
        api_key_env: "ANTHROPIC_API_KEY".to_string(),
        instruction: "Process data".to_string(),
        tools: vec!["http_request".to_string()],
        action_nodes: vec![
            ActionNodeEntry {
                id: "fetch".to_string(),
                config: serde_json::json!({"type": "http", "url": "https://api.example.com"}),
            },
            ActionNodeEntry {
                id: "transform".to_string(),
                config: serde_json::json!({"type": "transform", "expr": "data.items"}),
            },
        ],
        workflow_edges: vec![
            WorkflowEdge {
                from: "start".to_string(),
                to: "fetch".to_string(),
                condition: None,
            },
            WorkflowEdge {
                from: "fetch".to_string(),
                to: "transform".to_string(),
                condition: None,
            },
        ],
        sub_agents: vec![],
        role: AgentRoleConfig {
            allow: vec!["http_request".to_string()],
            deny: vec![],
        },
        channel_bindings: vec![],
        auto_start: false,
        temperature: None,
        max_output_tokens: None,
        model_override: None,
    };
    let graph_schema = codegen.to_project_schema(&graph_config);
    println!("   ProjectSchema for pipeline agent:");
    println!("     agent_type: {}", graph_schema.agent.agent_type);
    println!("     action_nodes: {}", graph_schema.action_nodes.len());
    println!(
        "     workflow_edges: {}",
        graph_schema
            .workflow
            .as_ref()
            .map(|w| w.edges.len())
            .unwrap_or(0)
    );
    println!();

    // ── Step 9: Persistence across restarts ────────────────────────
    println!("── Step 9: Persist and restore across gateway restart ──");

    // Current state: research=Running, writer=Created
    let agent_count = registry.list().len();
    println!("   Before restart: {} agents in registry", agent_count);

    // Simulate restart: create new registry from same persist dir
    let registry2 = AgentRegistry::new(persist_dir.clone());
    let loaded = registry2.load_from_disk().unwrap();
    println!("   After restart: loaded {} agents from disk", loaded);

    for (id, record) in registry2.list() {
        println!(
            "     {} — state: {:?}, model: {}",
            id, record.state, record.config.model
        );
    }

    // Rebuild RBAC from restored registry
    let rbac2 = RbacBridge::new();
    rbac2.register_system_agent("system");
    for (id, record) in registry2.list() {
        if id != "system" {
            rbac2.register_agent(&id, &record.config.role);
        }
    }
    println!(
        "   RBAC rebuilt: research can web_search = {}",
        rbac2.check_tool("research", "web_search").is_ok()
    );
    println!(
        "   RBAC rebuilt: research can agent_create = {}",
        rbac2.check_tool("research", "agent_create").is_ok()
    );
    println!();

    // ── Step 10: List agents via tool API ──────────────────────────
    println!("── Step 10: List agents via agent_list tool ──");

    let agents = registry.list();
    for (id, record) in &agents {
        println!(
            "   {} | {} | {:?} | model: {}",
            id, record.config.name, record.state, record.config.model,
        );
    }
    println!();

    // ── Step 11: Stop and delete agents ────────────────────────────
    println!("── Step 11: Graceful shutdown — stop and delete ──");

    // Stop research agent
    registry
        .transition("research", LifecycleState::Stopping)
        .unwrap();
    proxy_pool.remove("research");
    registry
        .transition("research", LifecycleState::Stopped)
        .unwrap();
    println!("   research: Running → Stopping → Stopped");

    // Delete research agent (archives workspace)
    let delete_result = registry.delete("research");
    if delete_result.is_ok() {
        rbac.remove_agent("research");
    }
    println!("   research: deleted = {}", delete_result.is_ok());

    // Delete writer agent (already in Created → need to transition to Stopped first)
    // Actually, Created agents can't be deleted directly — they need to go through the lifecycle.
    // Let's transition writer: Created → Starting → Error (simulate failure) → delete
    registry
        .transition("writer", LifecycleState::Starting)
        .unwrap();
    registry
        .transition(
            "writer",
            LifecycleState::Error {
                message: "demo cleanup".to_string(),
            },
        )
        .unwrap();
    let delete_result = registry.delete("writer");
    if delete_result.is_ok() {
        rbac.remove_agent("writer");
    }
    println!("   writer: deleted = {}", delete_result.is_ok());

    // Final state
    let remaining: Vec<String> = registry.list().into_iter().map(|(id, _)| id).collect();
    println!("   Remaining agents: {:?}", remaining);
    println!();

    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║   Demo complete — all multi-agent features exercised    ║");
    println!("╚══════════════════════════════════════════════════════════╝");
}
