//! Executable ADK tool wrappers for gateway operations.
//!
//! Wraps KnowledgeGraph and AgentRegistry operations as real `FunctionTool`
//! instances that the LLM agent can call. The ToolContext provides `user_id()`
//! for scoping KG operations to the correct user.

use adk_core::ToolContext;
use adk_tool::FunctionTool;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use tokio::sync::broadcast;

use crate::agent_codegen::AgentCodegen;
use crate::agent_registry::AgentRegistry;
use crate::control_panel::ws::WsEvent;
use crate::knowledge_graph::KnowledgeGraph;
use crate::process_manager::ProcessManager;
use crate::proxy_pool::RemoteAgentProxyPool;
use crate::rbac_bridge::RbacBridge;
use crate::router::MessageRouter;

// ── Knowledge Graph Tools ──────────────────────────────────────────

/// Build executable KG tools that scope operations to the current user_id.
pub fn build_kg_tools(kg: Arc<KnowledgeGraph>) -> Vec<Arc<dyn adk_core::Tool>> {
    vec![
        Arc::new(kg_create_entities(kg.clone())),
        Arc::new(kg_add_observations(kg.clone())),
        Arc::new(kg_search_nodes(kg.clone())),
        Arc::new(kg_read_graph(kg.clone())),
        Arc::new(kg_delete_entities(kg.clone())),
    ]
}

fn kg_create_entities(kg: Arc<KnowledgeGraph>) -> FunctionTool {
    FunctionTool::new(
        "kg_create_entities",
        "Create entities in the knowledge graph. Each entity has a name, type, and optional observations. Use this to store facts about the user, their projects, preferences, and important context.",
        move |ctx: Arc<dyn ToolContext>, args: Value| {
            let kg = kg.clone();
            async move {
                let user_id = ctx.user_id().to_string();
                tracing::info!(user_id = %user_id, "kg_create_entities: storing for user");
                let entities = args.get("entities")
                    .and_then(|v| v.as_array())
                    .ok_or_else(|| adk_core::AdkError::tool("'entities' array is required".to_string()))?;

                let inputs: Vec<crate::knowledge_graph::CreateEntityInput> = entities.iter()
                    .filter_map(|e| {
                        let name = e.get("name")?.as_str()?.to_string();
                        let entity_type = e.get("entity_type")
                            .or_else(|| e.get("type"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("general")
                            .to_string();
                        let observations = e.get("observations")
                            .and_then(|v| v.as_array())
                            .map(|arr| arr.iter().filter_map(|o| o.as_str().map(String::from)).collect())
                            .unwrap_or_default();
                        Some(crate::knowledge_graph::CreateEntityInput { name, entity_type, observations })
                    })
                    .collect();

                if inputs.is_empty() {
                    return Ok(serde_json::json!({"error": "no valid entities provided"}));
                }

                let created_names = kg.create_entities(&user_id, inputs);
                Ok(serde_json::json!({
                    "created": created_names.len(),
                    "entities": created_names
                }))
            }
        },
    )
}

fn kg_add_observations(kg: Arc<KnowledgeGraph>) -> FunctionTool {
    FunctionTool::new(
        "kg_add_observations",
        "Add observations (facts, notes, preferences) to an existing entity in the knowledge graph. The entity must already exist.",
        move |ctx: Arc<dyn ToolContext>, args: Value| {
            let kg = kg.clone();
            async move {
                let user_id = ctx.user_id().to_string();
                let entity_name = args.get("entity_name")
                    .or_else(|| args.get("entity"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| adk_core::AdkError::tool("'entity_name' is required".to_string()))?
                    .to_string();
                let observations: Vec<String> = args.get("observations")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|o| o.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                if observations.is_empty() {
                    return Ok(serde_json::json!({"error": "no observations provided"}));
                }

                let count = observations.len();
                match kg.add_observations(&user_id, &entity_name, observations) {
                    Some(ids) => Ok(serde_json::json!({
                        "entity": entity_name,
                        "added": ids.len()
                    })),
                    None => Ok(serde_json::json!({
                        "error": format!("entity '{}' not found", entity_name),
                        "attempted": count
                    })),
                }
            }
        },
    )
}

fn kg_search_nodes(kg: Arc<KnowledgeGraph>) -> FunctionTool {
    FunctionTool::new(
        "kg_search_nodes",
        "Search for entities in the knowledge graph by text query. Returns matching entities with their observations.",
        move |ctx: Arc<dyn ToolContext>, args: Value| {
            let kg = kg.clone();
            async move {
                let user_id = ctx.user_id().to_string();
                let query = args.get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                let results = kg.search_nodes(&user_id, query);
                let entries: Vec<Value> = results.iter().map(|r| {
                    serde_json::json!({
                        "name": r.entity.name,
                        "type": r.entity.entity_type,
                        "observations": r.entity.observations.iter()
                            .map(|o| &o.content)
                            .collect::<Vec<_>>(),
                    })
                }).collect();

                Ok(serde_json::json!({
                    "results": entries,
                    "count": entries.len()
                }))
            }
        },
    )
    .with_read_only(true)
}

fn kg_read_graph(kg: Arc<KnowledgeGraph>) -> FunctionTool {
    FunctionTool::new(
        "kg_read_graph",
        "Read the entire knowledge graph for the current user. Returns all entities, their types, observations, and relations.",
        move |ctx: Arc<dyn ToolContext>, args: Value| {
            let kg = kg.clone();
            async move {
                let user_id = ctx.user_id().to_string();
                let _ = args; // no args needed

                tracing::info!(user_id = %user_id, "kg_read_graph: querying KG");

                let (entities, relations) = kg.read_graph(&user_id);

                tracing::info!(
                    user_id = %user_id,
                    entity_count = entities.len(),
                    relation_count = relations.len(),
                    "kg_read_graph: results"
                );

                let entity_values: Vec<Value> = entities.iter().map(|e| {
                    serde_json::json!({
                        "name": e.name,
                        "type": e.entity_type,
                        "observations": e.observations.iter()
                            .map(|o| &o.content)
                            .collect::<Vec<_>>(),
                    })
                }).collect();
                let relation_values: Vec<Value> = relations.iter().map(|r| {
                    serde_json::json!({
                        "source": r.source,
                        "target": r.target,
                        "relation_type": r.relation_type,
                    })
                }).collect();

                Ok(serde_json::json!({
                    "entities": entity_values,
                    "relations": relation_values,
                    "entity_count": entity_values.len(),
                    "relation_count": relation_values.len()
                }))
            }
        },
    )
    .with_read_only(true)
}

fn kg_delete_entities(kg: Arc<KnowledgeGraph>) -> FunctionTool {
    FunctionTool::new(
        "kg_delete_entities",
        "Delete entities from the knowledge graph by name. Also removes associated relations.",
        move |ctx: Arc<dyn ToolContext>, args: Value| {
            let kg = kg.clone();
            async move {
                let user_id = ctx.user_id().to_string();
                let names: Vec<String> = args
                    .get("names")
                    .or_else(|| args.get("entities"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|n| n.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                if names.is_empty() {
                    return Ok(serde_json::json!({"error": "no entity names provided"}));
                }

                let deleted = kg.delete_entities(&user_id, names);
                Ok(serde_json::json!({
                    "deleted": deleted
                }))
            }
        },
    )
}

// ── Agent Management Tools ─────────────────────────────────────────

/// Build executable agent management tools (system agent only).
/// Note: This is a simplified version that creates tools without full wiring.
/// For production use, prefer `build_agent_management_tools()` which includes
/// RBAC, WebSocket events, and workspace directory creation.
/// Retained for integration tests that only need basic agent_list/agent_create.
#[allow(dead_code)]
pub fn build_agent_tools(
    registry: Arc<AgentRegistry>,
    rbac: Arc<RbacBridge>,
    ws_broadcast: broadcast::Sender<WsEvent>,
    workspace_root: PathBuf,
) -> Vec<Arc<dyn adk_core::Tool>> {
    vec![
        Arc::new(agent_list_tool(registry.clone())),
        Arc::new(agent_create_tool(
            registry.clone(),
            rbac,
            ws_broadcast,
            workspace_root,
        )),
    ]
}

/// Build all 6 executable agent management tools with full subsystem wiring.
pub fn build_agent_management_tools(
    registry: Arc<AgentRegistry>,
    process_manager: Arc<ProcessManager>,
    proxy_pool: Arc<RemoteAgentProxyPool>,
    rbac: Arc<RbacBridge>,
    router: Arc<ArcSwap<MessageRouter>>,
    codegen: Arc<AgentCodegen>,
    ws_broadcast: broadcast::Sender<WsEvent>,
    workspace_root: PathBuf,
    global_config: Arc<arc_swap::ArcSwap<crate::config::GatewayConfig>>,
) -> Vec<Arc<dyn adk_core::Tool>> {
    vec![
        Arc::new(agent_list_tool(registry.clone())),
        Arc::new(agent_create_tool(
            registry.clone(),
            rbac.clone(),
            ws_broadcast.clone(),
            workspace_root.clone(),
        )),
        Arc::new(agent_start_tool(
            registry.clone(),
            process_manager.clone(),
            proxy_pool.clone(),
            rbac.clone(),
            router.clone(),
            codegen.clone(),
            ws_broadcast.clone(),
            workspace_root.clone(),
            global_config.clone(),
        )),
        Arc::new(agent_stop_tool(
            registry.clone(),
            process_manager.clone(),
            proxy_pool.clone(),
            router.clone(),
            ws_broadcast.clone(),
        )),
        Arc::new(agent_delete_tool(
            registry.clone(),
            rbac.clone(),
            router.clone(),
            ws_broadcast.clone(),
        )),
        Arc::new(agent_configure_tool(
            registry.clone(),
            process_manager.clone(),
            proxy_pool.clone(),
            rbac.clone(),
            router.clone(),
            codegen.clone(),
            ws_broadcast.clone(),
            workspace_root,
            global_config,
        )),
    ]
}

fn agent_list_tool(registry: Arc<AgentRegistry>) -> FunctionTool {
    FunctionTool::new(
        "agent_list",
        "List all registered agents with their current lifecycle state, model, and description.",
        move |_ctx: Arc<dyn ToolContext>, _args: Value| {
            let registry = registry.clone();
            async move {
                let agents = registry.list();
                let entries: Vec<Value> = agents
                    .iter()
                    .map(|(id, record)| {
                        serde_json::json!({
                            "id": id,
                            "name": record.config.name,
                            "description": record.config.description,
                            "state": format!("{:?}", record.state),
                            "model": record.config.model,
                            "auto_start": record.config.auto_start,
                        })
                    })
                    .collect();

                Ok(serde_json::json!({
                    "agents": entries,
                    "count": entries.len()
                }))
            }
        },
    )
    .with_read_only(true)
}

fn agent_create_tool(
    registry: Arc<AgentRegistry>,
    rbac: Arc<RbacBridge>,
    ws_broadcast: broadcast::Sender<WsEvent>,
    workspace_root: PathBuf,
) -> FunctionTool {
    FunctionTool::new(
        "agent_create",
        "Create a new specialist agent. Requires: name, description, model (e.g. 'anthropic/claude-sonnet-4'), instruction. Optional: tools (array of tool names), auto_start (bool).",
        move |_ctx: Arc<dyn ToolContext>, args: Value| {
            let registry = registry.clone();
            let rbac = rbac.clone();
            let ws_broadcast = ws_broadcast.clone();
            let workspace_root = workspace_root.clone();
            async move {
                let name = args.get("name")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| adk_core::AdkError::tool("'name' is required".to_string()))?
                    .to_string();

                let description = args.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let model = args.get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("anthropic/claude-sonnet-4")
                    .to_string();

                let instruction = args.get("instruction")
                    .and_then(|v| v.as_str())
                    .unwrap_or("You are a helpful specialist agent.")
                    .to_string();

                let tools: Vec<String> = args.get("tools")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                    .unwrap_or_default();

                let auto_start = args.get("auto_start")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let id = name.to_lowercase()
                    .chars()
                    .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
                    .collect::<String>();

                let config = crate::agent_config::AgentConfig {
                    id: id.clone(),
                    name,
                    description,
                    agent_type: crate::agent_config::AgentType::Llm,
                    model,
                    api_key_env: String::new(),
                    instruction,
                    tools: tools.clone(),
                    action_nodes: vec![],
                    workflow_edges: vec![],
                    sub_agents: vec![],
                    role: crate::agent_config::AgentRoleConfig {
                        allow: tools,
                        deny: vec![],
                    },
                    channel_bindings: vec![],
                    auto_start,
                    temperature: None,
                    max_output_tokens: None,
                    model_override: None,
                };

                // 1. Register agent in the AgentRegistry
                let agent_id = match registry.create_agent(config.clone()) {
                    Ok(aid) => aid,
                    Err(e) => {
                        return Ok(serde_json::json!({
                            "created": false,
                            "error": e.to_string()
                        }));
                    }
                };

                // 2. Create workspace directories (context/, data/, src/)
                let agent_dir = workspace_root.join("agents").join(&agent_id);
                let context_dir = agent_dir.join("context");
                let data_dir = agent_dir.join("data");
                let src_dir = agent_dir.join("src");

                for dir in [&context_dir, &data_dir, &src_dir] {
                    if let Err(e) = std::fs::create_dir_all(dir) {
                        tracing::warn!(
                            agent_id = %agent_id,
                            dir = %dir.display(),
                            error = %e,
                            "failed to create workspace directory"
                        );
                    }
                }

                // 3. Write default context files
                let default_context_files: &[(&str, &str)] = &[
                    ("PROFILE.md", "# Agent Profile\n\nSpecialist agent profile.\n"),
                    ("USER.md", "# User Context\n\nUser-specific context and preferences.\n"),
                    ("PROJECTS.md", "# Projects\n\nActive projects and tasks.\n"),
                    ("HABITS.md", "# Habits\n\nUser habits and patterns.\n"),
                    ("NOTES.md", "# Notes\n\nGeneral notes and observations.\n"),
                    ("BOOTSTRAP.md", "# Bootstrap\n\nInitial setup and configuration context.\n"),
                ];

                for (filename, content) in default_context_files {
                    let file_path = context_dir.join(filename);
                    if let Err(e) = std::fs::write(&file_path, content) {
                        tracing::warn!(
                            agent_id = %agent_id,
                            file = %file_path.display(),
                            error = %e,
                            "failed to write default context file"
                        );
                    }
                }

                // 4. Register RBAC role (strips system tools)
                let stripped = rbac.register_agent(&agent_id, &config.role);
                if !stripped.is_empty() {
                    tracing::info!(
                        agent_id = %agent_id,
                        stripped = ?stripped,
                        "stripped system tool permissions from agent role"
                    );
                }

                // 5. Emit WsEvent::AgentState { state: "Created" }
                let _ = ws_broadcast.send(WsEvent::AgentState {
                    agent_id: agent_id.clone(),
                    state: "Created".into(),
                });

                Ok(serde_json::json!({
                    "created": true,
                    "agent_id": agent_id,
                    "message": format!("Agent '{}' created successfully.", agent_id)
                }))
            }
        },
    )
}

fn agent_stop_tool(
    registry: Arc<AgentRegistry>,
    process_manager: Arc<ProcessManager>,
    proxy_pool: Arc<RemoteAgentProxyPool>,
    router: Arc<ArcSwap<MessageRouter>>,
    ws_broadcast: broadcast::Sender<WsEvent>,
) -> FunctionTool {
    FunctionTool::new(
        "agent_stop",
        "Stop a running User Agent by ID. Gracefully drains the process, removes it from the proxy pool and message router.",
        move |_ctx: Arc<dyn ToolContext>, args: Value| {
            let registry = registry.clone();
            let process_manager = process_manager.clone();
            let proxy_pool = proxy_pool.clone();
            let router = router.clone();
            let ws_broadcast = ws_broadcast.clone();
            async move {
                let agent_id = args.get("agent_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| adk_core::AdkError::tool("'agent_id' is required".to_string()))?
                    .to_string();

                // Verify agent exists in registry
                {
                    let _entry = registry.get(&agent_id)
                        .ok_or_else(|| adk_core::AdkError::tool(
                            format!("agent '{}' not found in registry", agent_id)
                        ))?;
                }

                // 1. Transition to Stopping
                registry.transition(&agent_id, crate::agent_config::LifecycleState::Stopping)
                    .map_err(|e| adk_core::AdkError::tool(
                        format!("failed to transition '{}' to Stopping: {}", agent_id, e)
                    ))?;

                // Emit Stopping event
                let _ = ws_broadcast.send(WsEvent::AgentState {
                    agent_id: agent_id.clone(),
                    state: "Stopping".into(),
                });

                // 2. Stop the process with 10s drain timeout
                if let Err(e) = process_manager.stop(&agent_id, Duration::from_secs(10)).await {
                    tracing::warn!(
                        agent_id = %agent_id,
                        error = %e,
                        "process stop failed, continuing with cleanup"
                    );
                }

                // 3. Remove from ProxyPool
                proxy_pool.remove(&agent_id);

                // 4. Remove agent bindings from MessageRouter via ArcSwap clone-mutate-store
                let current = router.load();
                let mut new_router = (**current).clone();
                new_router.remove_agent_bindings(&agent_id);
                router.store(Arc::new(new_router));

                // 5. Transition to Stopped
                let _ = registry.transition(&agent_id, crate::agent_config::LifecycleState::Stopped);

                // 6. Emit Stopped WebSocket event
                let _ = ws_broadcast.send(WsEvent::AgentState {
                    agent_id: agent_id.clone(),
                    state: "Stopped".into(),
                });

                Ok(serde_json::json!({
                    "stopped": true,
                    "agent_id": agent_id,
                    "state": "Stopped"
                }))
            }
        },
    )
}

fn agent_start_tool(
    registry: Arc<AgentRegistry>,
    process_manager: Arc<ProcessManager>,
    proxy_pool: Arc<RemoteAgentProxyPool>,
    rbac: Arc<RbacBridge>,
    router: Arc<ArcSwap<MessageRouter>>,
    codegen: Arc<AgentCodegen>,
    ws_broadcast: broadcast::Sender<WsEvent>,
    workspace_root: PathBuf,
    global_config: Arc<arc_swap::ArcSwap<crate::config::GatewayConfig>>,
) -> FunctionTool {
    FunctionTool::new(
        "agent_start",
        "Start a User Agent by ID. Builds the agent binary, spawns the process, waits for readiness, and registers it for message routing.",
        move |_ctx: Arc<dyn ToolContext>, args: Value| {
            let registry = registry.clone();
            let process_manager = process_manager.clone();
            let proxy_pool = proxy_pool.clone();
            let rbac = rbac.clone();
            let router = router.clone();
            let codegen = codegen.clone();
            let ws_broadcast = ws_broadcast.clone();
            let workspace_root = workspace_root.clone();
            let global_config = global_config.clone();
            async move {
                let agent_id = args.get("agent_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| adk_core::AdkError::tool("'agent_id' is required".to_string()))?
                    .to_string();

                // Get the agent config from the registry
                let config = {
                    let entry = registry.get(&agent_id)
                        .ok_or_else(|| adk_core::AdkError::tool(
                            format!("agent '{}' not found in registry", agent_id)
                        ))?;
                    entry.config.clone()
                };

                // Resolve the effective primary model using per-agent category overrides (R9.3, R9.5, R9.6)
                let global_cfg = global_config.load();
                let effective_model = config
                    .resolve_model("primary", &global_cfg.agent.model)
                    .unwrap_or(config.model.as_str());

                // 1. Transition to Starting
                registry.transition(&agent_id, crate::agent_config::LifecycleState::Starting)
                    .map_err(|e| adk_core::AdkError::tool(
                        format!("failed to transition '{}' to Starting: {}", agent_id, e)
                    ))?;

                // Emit Starting WebSocket event
                let _ = ws_broadcast.send(WsEvent::AgentState {
                    agent_id: agent_id.clone(),
                    state: "Starting".into(),
                });

                // 2. Build agent binary via codegen
                let binary_path = match codegen.build_agent(&config).await {
                    Ok(path) => path,
                    Err(e) => {
                        let _ = registry.transition(
                            &agent_id,
                            crate::agent_config::LifecycleState::Error {
                                message: format!("build failed: {}", e),
                            },
                        );
                        let _ = ws_broadcast.send(WsEvent::AgentState {
                            agent_id: agent_id.clone(),
                            state: "Error".into(),
                        });
                        return Ok(serde_json::json!({
                            "started": false,
                            "agent_id": agent_id,
                            "error": format!("build failed: {}", e)
                        }));
                    }
                };

                // 3. Resolve API key env and build env map
                let api_key_env = config.resolve_api_key_env().to_string();
                let mut env = HashMap::new();
                env.insert("AGENT_ID".to_string(), agent_id.clone());
                env.insert("AGENT_MODEL".to_string(), effective_model.to_string());
                if let Ok(val) = std::env::var(&api_key_env) {
                    env.insert(api_key_env.clone(), val);
                }
                env.insert(
                    "AGENT_DATA_DIR".to_string(),
                    workspace_root
                        .join("agents")
                        .join(&agent_id)
                        .join("data")
                        .display()
                        .to_string(),
                );

                // 4. Spawn process
                let port = match process_manager.spawn(&agent_id, &binary_path, env).await {
                    Ok(port) => port,
                    Err(e) => {
                        let _ = registry.transition(
                            &agent_id,
                            crate::agent_config::LifecycleState::Error {
                                message: format!("spawn failed: {}", e),
                            },
                        );
                        let _ = ws_broadcast.send(WsEvent::AgentState {
                            agent_id: agent_id.clone(),
                            state: "Error".into(),
                        });
                        return Ok(serde_json::json!({
                            "started": false,
                            "agent_id": agent_id,
                            "error": format!("spawn failed: {}", e)
                        }));
                    }
                };

                // 5. Wait for readiness (30s timeout)
                if let Err(e) = process_manager.wait_ready(&agent_id, Duration::from_secs(30)).await {
                    // Cleanup on timeout
                    let _ = process_manager.stop(&agent_id, Duration::from_secs(5)).await;
                    proxy_pool.remove(&agent_id);
                    let _ = registry.transition(
                        &agent_id,
                        crate::agent_config::LifecycleState::Error {
                            message: format!("readiness check failed: {}", e),
                        },
                    );
                    let _ = ws_broadcast.send(WsEvent::AgentState {
                        agent_id: agent_id.clone(),
                        state: "Error".into(),
                    });
                    return Ok(serde_json::json!({
                        "started": false,
                        "agent_id": agent_id,
                        "error": format!("agent '{}' failed readiness check: {}", agent_id, e)
                    }));
                }

                // 6. Register proxy
                proxy_pool.register(&agent_id, port);

                // 7. Register RBAC role
                rbac.register_agent(&agent_id, &config.role);

                // 8. Add router bindings (clone-mutate-store via ArcSwap)
                if !config.channel_bindings.is_empty() {
                    let current = router.load();
                    let mut new_router = (**current).clone();
                    new_router.add_agent_bindings(&agent_id, &config.channel_bindings);
                    router.store(Arc::new(new_router));
                }

                // 9. Transition to Running
                let _ = registry.transition(&agent_id, crate::agent_config::LifecycleState::Running);

                // 10. Emit WebSocket event
                let _ = ws_broadcast.send(WsEvent::AgentState {
                    agent_id: agent_id.clone(),
                    state: "Running".into(),
                });

                Ok(serde_json::json!({
                    "started": true,
                    "agent_id": agent_id,
                    "port": port,
                    "state": "Running"
                }))
            }
        },
    )
}

fn agent_delete_tool(
    registry: Arc<AgentRegistry>,
    rbac: Arc<RbacBridge>,
    router: Arc<ArcSwap<MessageRouter>>,
    ws_broadcast: broadcast::Sender<WsEvent>,
) -> FunctionTool {
    FunctionTool::new(
        "agent_delete",
        "Delete a User Agent by ID. The agent must be in Stopped or Error state. Removes the agent from the registry, RBAC roles, and message router bindings.",
        move |_ctx: Arc<dyn ToolContext>, args: Value| {
            let registry = registry.clone();
            let rbac = rbac.clone();
            let router = router.clone();
            let ws_broadcast = ws_broadcast.clone();
            async move {
                let agent_id = args.get("agent_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| adk_core::AdkError::tool("'agent_id' is required".to_string()))?
                    .to_string();

                // 1. Verify agent is in Stopped or Error state (AgentRegistry::delete enforces this)
                //    We call delete which checks the precondition internally.
                registry.delete(&agent_id)
                    .map_err(|e| adk_core::AdkError::tool(
                        format!("cannot delete agent '{}': {}", agent_id, e)
                    ))?;

                // 2. Remove RBAC role
                rbac.remove_agent(&agent_id);

                // 3. Remove any residual router bindings via ArcSwap clone-mutate-store
                let current = router.load();
                let mut new_router = (**current).clone();
                new_router.remove_agent_bindings(&agent_id);
                router.store(Arc::new(new_router));

                // 4. Emit Deleted WebSocket event
                let _ = ws_broadcast.send(WsEvent::AgentState {
                    agent_id: agent_id.clone(),
                    state: "Deleted".into(),
                });

                Ok(serde_json::json!({
                    "deleted": true,
                    "agent_id": agent_id,
                    "state": "Deleted"
                }))
            }
        },
    )
}

fn agent_configure_tool(
    registry: Arc<AgentRegistry>,
    process_manager: Arc<ProcessManager>,
    proxy_pool: Arc<RemoteAgentProxyPool>,
    rbac: Arc<RbacBridge>,
    router: Arc<ArcSwap<MessageRouter>>,
    codegen: Arc<AgentCodegen>,
    ws_broadcast: broadcast::Sender<WsEvent>,
    workspace_root: PathBuf,
    global_config: Arc<arc_swap::ArcSwap<crate::config::GatewayConfig>>,
) -> FunctionTool {
    FunctionTool::new(
        "agent_configure",
        "Update a User Agent's configuration. Accepts the agent_id and a new config object. If the agent is Running, it will be stopped, reconfigured, and restarted automatically.",
        move |_ctx: Arc<dyn ToolContext>, args: Value| {
            let registry = registry.clone();
            let process_manager = process_manager.clone();
            let proxy_pool = proxy_pool.clone();
            let rbac = rbac.clone();
            let router = router.clone();
            let codegen = codegen.clone();
            let ws_broadcast = ws_broadcast.clone();
            let workspace_root = workspace_root.clone();
            let global_config = global_config.clone();
            async move {
                let agent_id = args.get("agent_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| adk_core::AdkError::tool("'agent_id' is required".to_string()))?
                    .to_string();

                let config_value = args.get("config")
                    .ok_or_else(|| adk_core::AdkError::tool("'config' is required".to_string()))?;

                let new_config: crate::agent_config::AgentConfig = serde_json::from_value(config_value.clone())
                    .map_err(|e| adk_core::AdkError::tool(
                        format!("invalid config: {}", e)
                    ))?;

                // Validate config ID matches agent_id
                if new_config.id != agent_id {
                    return Err(adk_core::AdkError::tool(
                        format!("config id '{}' does not match agent_id '{}'", new_config.id, agent_id)
                    ));
                }

                // Get current state and old config to determine if restart is needed
                let (was_running, old_channel_bindings) = {
                    let entry = registry.get(&agent_id)
                        .ok_or_else(|| adk_core::AdkError::tool(
                            format!("agent '{}' not found in registry", agent_id)
                        ))?;
                    let running = entry.state == crate::agent_config::LifecycleState::Running;
                    let old_bindings = entry.config.channel_bindings.clone();
                    (running, old_bindings)
                };

                // If agent was Running: stop it first
                if was_running {
                    // Transition to Stopping
                    registry.transition(&agent_id, crate::agent_config::LifecycleState::Stopping)
                        .map_err(|e| adk_core::AdkError::tool(
                            format!("failed to transition '{}' to Stopping: {}", agent_id, e)
                        ))?;

                    let _ = ws_broadcast.send(WsEvent::AgentState {
                        agent_id: agent_id.clone(),
                        state: "Stopping".into(),
                    });

                    // Stop the process with 10s drain timeout
                    if let Err(e) = process_manager.stop(&agent_id, Duration::from_secs(10)).await {
                        tracing::warn!(
                            agent_id = %agent_id,
                            error = %e,
                            "process stop failed during configure, continuing with cleanup"
                        );
                    }

                    // Remove from ProxyPool
                    proxy_pool.remove(&agent_id);

                    // Remove old router bindings
                    let current = router.load();
                    let mut new_router = (**current).clone();
                    new_router.remove_agent_bindings(&agent_id);
                    router.store(Arc::new(new_router));

                    // Transition to Stopped
                    let _ = registry.transition(&agent_id, crate::agent_config::LifecycleState::Stopped);

                    let _ = ws_broadcast.send(WsEvent::AgentState {
                        agent_id: agent_id.clone(),
                        state: "Stopped".into(),
                    });
                }

                // Update config in registry
                registry.update_config(&agent_id, new_config.clone())
                    .map_err(|e| adk_core::AdkError::tool(
                        format!("failed to update config for '{}': {}", agent_id, e)
                    ))?;

                // Re-register RBAC role with new config
                rbac.register_agent(&agent_id, &new_config.role);

                // Update router bindings if channel_bindings changed
                if new_config.channel_bindings != old_channel_bindings {
                    let current = router.load();
                    let mut new_router = (**current).clone();
                    new_router.update_agent_bindings(&agent_id, &new_config.channel_bindings);
                    router.store(Arc::new(new_router));
                }

                // If agent was Running: restart it
                if was_running {
                    // Transition to Starting
                    registry.transition(&agent_id, crate::agent_config::LifecycleState::Starting)
                        .map_err(|e| adk_core::AdkError::tool(
                            format!("failed to transition '{}' to Starting: {}", agent_id, e)
                        ))?;

                    // Emit Starting WebSocket event
                    let _ = ws_broadcast.send(WsEvent::AgentState {
                        agent_id: agent_id.clone(),
                        state: "Starting".into(),
                    });

                    // Build agent binary via codegen
                    let binary_path = match codegen.build_agent(&new_config).await {
                        Ok(path) => path,
                        Err(e) => {
                            let _ = registry.transition(
                                &agent_id,
                                crate::agent_config::LifecycleState::Error {
                                    message: format!("build failed during configure: {}", e),
                                },
                            );
                            let _ = ws_broadcast.send(WsEvent::AgentState {
                                agent_id: agent_id.clone(),
                                state: "Error".into(),
                            });
                            return Ok(serde_json::json!({
                                "configured": true,
                                "restarted": false,
                                "agent_id": agent_id,
                                "error": format!("config updated but restart failed: build error: {}", e)
                            }));
                        }
                    };

                    // Resolve API key env and build env map
                    let global_cfg = global_config.load();
                    let effective_model = new_config
                        .resolve_model("primary", &global_cfg.agent.model)
                        .unwrap_or(new_config.model.as_str());
                    let api_key_env = new_config.resolve_api_key_env().to_string();
                    let mut env = HashMap::new();
                    env.insert("AGENT_ID".to_string(), agent_id.clone());
                    env.insert("AGENT_MODEL".to_string(), effective_model.to_string());
                    if let Ok(val) = std::env::var(&api_key_env) {
                        env.insert(api_key_env.clone(), val);
                    }
                    env.insert(
                        "AGENT_DATA_DIR".to_string(),
                        workspace_root
                            .join("agents")
                            .join(&agent_id)
                            .join("data")
                            .display()
                            .to_string(),
                    );

                    // Spawn process
                    let port = match process_manager.spawn(&agent_id, &binary_path, env).await {
                        Ok(port) => port,
                        Err(e) => {
                            let _ = registry.transition(
                                &agent_id,
                                crate::agent_config::LifecycleState::Error {
                                    message: format!("spawn failed during configure: {}", e),
                                },
                            );
                            let _ = ws_broadcast.send(WsEvent::AgentState {
                                agent_id: agent_id.clone(),
                                state: "Error".into(),
                            });
                            return Ok(serde_json::json!({
                                "configured": true,
                                "restarted": false,
                                "agent_id": agent_id,
                                "error": format!("config updated but restart failed: spawn error: {}", e)
                            }));
                        }
                    };

                    // Wait for readiness (30s timeout)
                    if let Err(e) = process_manager.wait_ready(&agent_id, Duration::from_secs(30)).await {
                        // Cleanup on timeout
                        let _ = process_manager.stop(&agent_id, Duration::from_secs(5)).await;
                        proxy_pool.remove(&agent_id);
                        let _ = registry.transition(
                            &agent_id,
                            crate::agent_config::LifecycleState::Error {
                                message: format!("readiness check failed during configure: {}", e),
                            },
                        );
                        let _ = ws_broadcast.send(WsEvent::AgentState {
                            agent_id: agent_id.clone(),
                            state: "Error".into(),
                        });
                        return Ok(serde_json::json!({
                            "configured": true,
                            "restarted": false,
                            "agent_id": agent_id,
                            "error": format!("config updated but restart failed: readiness timeout: {}", e)
                        }));
                    }

                    // Register proxy
                    proxy_pool.register(&agent_id, port);

                    // Re-add router bindings for the new config (if not already done above)
                    if !new_config.channel_bindings.is_empty() {
                        let current = router.load();
                        let mut new_router = (**current).clone();
                        // Ensure bindings are current (update_agent_bindings replaces)
                        new_router.update_agent_bindings(&agent_id, &new_config.channel_bindings);
                        router.store(Arc::new(new_router));
                    }

                    // Transition to Running
                    let _ = registry.transition(&agent_id, crate::agent_config::LifecycleState::Running);

                    // Emit Running WebSocket event
                    let _ = ws_broadcast.send(WsEvent::AgentState {
                        agent_id: agent_id.clone(),
                        state: "Running".into(),
                    });

                    return Ok(serde_json::json!({
                        "configured": true,
                        "restarted": true,
                        "agent_id": agent_id,
                        "port": port,
                        "state": "Running"
                    }));
                }

                // Agent was not running — just emit state event for the config update
                let _ = ws_broadcast.send(WsEvent::AgentState {
                    agent_id: agent_id.clone(),
                    state: "Configured".into(),
                });

                Ok(serde_json::json!({
                    "configured": true,
                    "restarted": false,
                    "agent_id": agent_id,
                    "state": "Configured"
                }))
            }
        },
    )
}

// ── Scheduled Task Tools ───────────────────────────────────────────

/// Build executable scheduled task tools.
pub fn build_scheduled_task_tools(
    cron_scheduler: Arc<tokio::sync::Mutex<Option<crate::cron::CronScheduler>>>,
    config: Arc<ArcSwap<crate::config::GatewayConfig>>,
    config_path: PathBuf,
) -> Vec<Arc<dyn adk_core::Tool>> {
    vec![
        Arc::new(task_list_tool(cron_scheduler.clone())),
        Arc::new(task_create_tool(
            cron_scheduler.clone(),
            config.clone(),
            config_path.clone(),
        )),
        Arc::new(task_cancel_tool(cron_scheduler.clone())),
        Arc::new(task_delete_tool(
            cron_scheduler.clone(),
            config.clone(),
            config_path,
        )),
    ]
}

fn task_list_tool(
    scheduler: Arc<tokio::sync::Mutex<Option<crate::cron::CronScheduler>>>,
) -> FunctionTool {
    FunctionTool::new(
        "task_list",
        "List all scheduled tasks (cron jobs) with their ID, schedule, message, delivery target, and status.",
        move |_ctx: Arc<dyn ToolContext>, _args: Value| {
            let scheduler = scheduler.clone();
            async move {
                let guard = scheduler.lock().await;
                let jobs: Vec<Value> = match guard.as_ref() {
                    Some(sched) => {
                        sched.list_all_jobs().iter().map(|(job, status)| {
                            serde_json::json!({
                                "id": job.id,
                                "schedule": job.schedule,
                                "message": job.message,
                                "delivery": job.deliver_to.as_ref().map(|d| serde_json::json!({
                                    "channel": d.channel,
                                    "target": d.target,
                                })),
                                "status": format!("{:?}", status),
                            })
                        }).collect()
                    }
                    None => vec![],
                };

                Ok(serde_json::json!({
                    "tasks": jobs,
                    "count": jobs.len()
                }))
            }
        },
    )
    .with_read_only(true)
}

fn task_create_tool(
    scheduler: Arc<tokio::sync::Mutex<Option<crate::cron::CronScheduler>>>,
    config: Arc<ArcSwap<crate::config::GatewayConfig>>,
    config_path: PathBuf,
) -> FunctionTool {
    FunctionTool::new(
        "task_create",
        "Create a new scheduled task. Required: id (unique string), schedule (e.g. '@every 5m', '@every 1h'), message (text to send or 'ask:prompt' for agent processing). Optional: delivery (object with 'channel' and 'target' fields).",
        move |_ctx: Arc<dyn ToolContext>, args: Value| {
            let scheduler = scheduler.clone();
            let config = config.clone();
            let config_path = config_path.clone();
            async move {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                let schedule = args.get("schedule").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                let message = args.get("message").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();

                if id.is_empty() || schedule.is_empty() || message.is_empty() {
                    return Err(adk_core::AdkError::tool(
                        "Required fields: 'id', 'schedule', 'message'".to_string()
                    ));
                }

                let delivery = args.get("delivery").and_then(|d| {
                    let channel = d.get("channel")?.as_str()?.to_string();
                    let target = d.get("target")?.as_str()?.to_string();
                    if channel.is_empty() { return None; }
                    Some(crate::config::CronDelivery { channel, target })
                });

                let new_job = crate::config::CronJob {
                    id: id.clone(),
                    schedule: schedule.clone(),
                    message: message.clone(),
                    deliver_to: delivery,
                };

                // Persist to config
                let mut cfg = config.load().as_ref().clone();
                if cfg.cron.jobs.iter().any(|j| j.id == id) {
                    return Err(adk_core::AdkError::tool(
                        format!("Task with ID '{}' already exists", id)
                    ));
                }
                cfg.cron.jobs.push(new_job.clone());

                let output = serde_json::to_string_pretty(&cfg)
                    .map_err(|e| adk_core::AdkError::tool(format!("Serialize error: {e}")))?;
                std::fs::write(&config_path, &output)
                    .map_err(|e| adk_core::AdkError::tool(format!("Write error: {e}")))?;

                // Hot-reload
                config.store(std::sync::Arc::new(cfg.clone()));
                let mut guard = scheduler.lock().await;
                if let Some(sched) = guard.as_mut() {
                    sched.reconcile(&cfg.cron.jobs);
                }

                Ok(serde_json::json!({
                    "created": true,
                    "id": id,
                    "schedule": schedule,
                    "message": message
                }))
            }
        },
    )
}

fn task_cancel_tool(
    scheduler: Arc<tokio::sync::Mutex<Option<crate::cron::CronScheduler>>>,
) -> FunctionTool {
    FunctionTool::new(
        "task_cancel",
        "Cancel (pause) a running scheduled task by ID. The task remains in config but stops firing.",
        move |_ctx: Arc<dyn ToolContext>, args: Value| {
            let scheduler = scheduler.clone();
            async move {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if id.is_empty() {
                    return Err(adk_core::AdkError::tool("'id' is required".to_string()));
                }

                let mut guard = scheduler.lock().await;
                if let Some(sched) = guard.as_mut() {
                    sched.cancel(&id);
                }

                Ok(serde_json::json!({
                    "cancelled": true,
                    "id": id
                }))
            }
        },
    )
}

fn task_delete_tool(
    scheduler: Arc<tokio::sync::Mutex<Option<crate::cron::CronScheduler>>>,
    config: Arc<ArcSwap<crate::config::GatewayConfig>>,
    config_path: PathBuf,
) -> FunctionTool {
    FunctionTool::new(
        "task_delete",
        "Permanently delete a scheduled task by ID. Removes it from config and stops it if running.",
        move |_ctx: Arc<dyn ToolContext>, args: Value| {
            let scheduler = scheduler.clone();
            let config = config.clone();
            let config_path = config_path.clone();
            async move {
                let id = args.get("id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
                if id.is_empty() {
                    return Err(adk_core::AdkError::tool("'id' is required".to_string()));
                }

                // Remove from config
                let mut cfg = config.load().as_ref().clone();
                let before = cfg.cron.jobs.len();
                cfg.cron.jobs.retain(|j| j.id != id);

                if cfg.cron.jobs.len() == before {
                    return Err(adk_core::AdkError::tool(
                        format!("Task '{}' not found", id)
                    ));
                }

                let output = serde_json::to_string_pretty(&cfg)
                    .map_err(|e| adk_core::AdkError::tool(format!("Serialize error: {e}")))?;
                std::fs::write(&config_path, &output)
                    .map_err(|e| adk_core::AdkError::tool(format!("Write error: {e}")))?;

                // Hot-reload
                config.store(std::sync::Arc::new(cfg.clone()));
                let mut guard = scheduler.lock().await;
                if let Some(sched) = guard.as_mut() {
                    sched.reconcile(&cfg.cron.jobs);
                }

                Ok(serde_json::json!({
                    "deleted": true,
                    "id": id
                }))
            }
        },
    )
}
