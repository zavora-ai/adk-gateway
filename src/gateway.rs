//! Gateway — the central orchestrator.
//!
//! Kept lean: delegates state construction to `gateway_state`,
//! HTTP routes to `gateway_routes`, and message processing is inline.

use crate::access_control::AuthDecision;
use crate::audit::{AuditEvent, AuditEventType, AuditOutcome};
use crate::channel::{self, ChannelKey, InboundMessage};
use crate::config::GatewayConfig;
use crate::config_watcher::{validate_config, ConfigWatcher};
use crate::context_coordinator::ContextCoordinator;
use crate::control_panel::LogEntry;
use crate::cron::CronScheduler;
use crate::delivery::{self, DeliveryStrategy, MessageRef};
use crate::event_stream::EventStreamCollector;
use crate::fallback_chain::FallbackOutcome;
use crate::gateway_routes;
use crate::gateway_state::{self, GatewayState};
use crate::metrics::MessageStatus;
use crate::model_factory;
use crate::pairing::PairingResult;
use crate::rate_limiter::{RateLimitDecision, RateLimiter};
use crate::router::MessageRouter;
use crate::webhook::WebhookHandler;

use adk_agent::LlmAgentBuilder;
use adk_core::{Agent, Content, Part};
use adk_runner::{Runner, RunnerConfig};

use anyhow::Context;
use arc_swap::ArcSwap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, Mutex};

/// Run the gateway with the given configuration.
pub async fn run(mut config: GatewayConfig, port: u16, config_path: PathBuf) -> anyhow::Result<()> {
    // ── Decrypt encrypted config values at startup ─────────────────
    {
        use crate::config_encryption::{has_encrypted_values, validate_encryption_at_startup};

        let key_file = config.gateway.encryption.as_ref().map(|e| e.key_file.as_path());
        let mut config_value = serde_json::to_value(&config)
            .context("failed to serialize config for encryption check")?;

        if has_encrypted_values(&config_value) {
            let encryptor = validate_encryption_at_startup(&config_value, key_file)
                .map_err(|e| {
                    tracing::error!(%e, "config encryption error — cannot start gateway");
                    anyhow::anyhow!("{e}")
                })?
                .ok_or_else(|| anyhow::anyhow!("encrypted values present but no decryption key available"))?;

            encryptor.decrypt_config(&mut config_value).map_err(|e| {
                tracing::error!(%e, "failed to decrypt config values");
                anyhow::anyhow!("config decryption failed: {e}")
            })?;

            config = serde_json::from_value(config_value)
                .context("failed to deserialize decrypted config")?;

            tracing::info!("decrypted encrypted config values at startup");
        }
    }

    // ── Build model with fallback chain (R16) ──────────────────────
    let primary_chain = {
        let model_ids = config
            .agent
            .model
            .resolve_chain("primary")
            .map(|chain| chain.to_vec())
            .unwrap_or_else(|| vec![config.agent.model.primary().to_string()]);

        if model_ids.len() > 1 {
            tracing::info!(
                primary = %model_ids[0],
                fallbacks = ?&model_ids[1..],
                "initializing primary model with fallback chain"
            );
            let result = crate::fallback_chain::FallbackModelChain::build(&model_ids)?;
            if !result.failed.is_empty() {
                for (id, err) in &result.failed {
                    tracing::warn!(model = %id, error = %err, "fallback model failed to initialize");
                }
            }
            result.chain
        } else {
            let model_id = config.agent.model.primary();
            tracing::info!(model = model_id, "initializing model");
            let model = model_factory::create_model(model_id)?;
            crate::fallback_chain::FallbackModelChain::single(model, model_id.to_string())
        }
    };

    let model = primary_chain.primary().clone();

    // Load memory protocol and context files if memory is configured
    let memory_protocol = config.memory.as_ref().and_then(|mem_cfg| {
        // Try relative to config dir first, then relative to working directory
        let candidates = if mem_cfg.protocol_path.is_relative() {
            vec![
                config_path
                    .parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join(&mem_cfg.protocol_path),
                std::env::current_dir()
                    .unwrap_or_default()
                    .join(&mem_cfg.protocol_path),
            ]
        } else {
            vec![mem_cfg.protocol_path.clone()]
        };
        for protocol_path in &candidates {
            if let Ok(content) = std::fs::read_to_string(protocol_path) {
                tracing::info!(
                    ?protocol_path,
                    "loaded memory protocol ({} bytes)",
                    content.len()
                );
                return Some(content);
            }
        }
        tracing::warn!(
            paths = ?candidates,
            "memory protocol file not found in any location, using default instructions"
        );
        None
    });

    // Load context files (PROFILE.md, USER.md, PROJECTS.md, etc.)
    let context_files_content = config.memory.as_ref().map(|mem_cfg| {
        let context_dir_candidates = if mem_cfg.context_dir.is_relative() {
            vec![
                config_path.parent()
                    .unwrap_or(std::path::Path::new("."))
                    .join(&mem_cfg.context_dir),
                std::env::current_dir()
                    .unwrap_or_default()
                    .join(&mem_cfg.context_dir),
            ]
        } else {
            vec![mem_cfg.context_dir.clone()]
        };

        let context_dir = context_dir_candidates.iter()
            .find(|p| p.is_dir())
            .cloned();

        let mut context = String::new();
        let files = ["PROFILE.md", "USER.md", "PROJECTS.md", "HABITS.md", "NOTES.md"];

        if let Some(ref dir) = context_dir {
            for file in &files {
                let path = dir.join(file);
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if !content.trim().is_empty() {
                        context.push_str(&format!("\n\n--- {} ---\n{}", file, content));
                        tracing::info!(file = %file, bytes = content.len(), "loaded context file");
                    }
                }
            }
            if context.is_empty() {
                tracing::info!(?dir, "context directory found but no files loaded");
            }
        } else {
            tracing::debug!(
                paths = ?context_dir_candidates,
                "context directory not found, skipping context files"
            );
        }
        context
    }).unwrap_or_default();

    let base_instruction = "You are a helpful AI assistant connected via adk-gateway. \
         Be concise and helpful. When you don't know something, say so.";

    let mut full_instruction = match memory_protocol {
        Some(ref protocol) => format!("{base_instruction}\n\n{protocol}"),
        None => base_instruction.to_string(),
    };

    if !context_files_content.is_empty() {
        full_instruction.push_str(&context_files_content);
    }

    // Build KG early so we can attach executable tools to the agent
    let kg_db_path = config_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("knowledge_graph.db");
    let knowledge_graph = Arc::new(crate::knowledge_graph::KnowledgeGraph::with_persistence(kg_db_path));

    // Build agent with executable tools when available
    let mut agent_builder = LlmAgentBuilder::new("assistant")
        .model(model)
        .instruction(&full_instruction);

    // Attach KG tools when memory is configured (R21)
    if config.memory.is_some() {
        let kg_tools = crate::executable_tools::build_kg_tools(knowledge_graph.clone());
        for tool in kg_tools {
            agent_builder = agent_builder.tool(tool);
        }
        tracing::info!("attached 5 executable KG tools to root agent");
    }

    let agent: Arc<dyn Agent> = Arc::new(agent_builder.build()?);
    tracing::info!(agent = agent.name(), "root agent ready");

    // ── Build all components ───────────────────────────────────────
    // Validate config before proceeding
    if let Err(e) = validate_config(&config) {
        tracing::error!(error = %e, "config validation failed");
        anyhow::bail!("invalid configuration: {e}");
    }

    let state = Arc::new(
        gateway_state::build(
            &config,
            agent,
            config_path.clone(),
            knowledge_graph,
            Arc::new(primary_chain),
            full_instruction.clone(),
        )
        .await?,
    );

    // Update system agent's instruction in the registry to reflect the full composed prompt
    if let Some(entry) = state.agent_registry.get("system") {
        let mut sys_config = entry.config.clone();
        drop(entry);
        sys_config.instruction = full_instruction.clone();
        let _ = state.agent_registry.update_config("system", sys_config);
    }

    // ── Attach agent management tools to the root agent ────────────
    // The management tools require subsystem references that are only available
    // after gateway_state::build(). We rebuild the root agent with them attached.
    if !state.agent_management_tools.is_empty() {
        // Inject MCP tool awareness into the instruction
        let mcp_server_ids = state.mcp_manager.server_ids();
        if !mcp_server_ids.is_empty() {
            let mut mcp_section = String::from("\n\n## MCP Tools (External Integrations)\n\nThe following MCP servers are connected with their available tools:\n\n");
            for server_id in &mcp_server_ids {
                let tools = state.mcp_manager.discovered_tools(server_id);
                if tools.is_empty() {
                    mcp_section.push_str(&format!("- **{}** — connected (no tools discovered)\n", server_id));
                } else {
                    mcp_section.push_str(&format!("- **{}** — {} tools: {}\n", server_id, tools.len(), tools.join(", ")));
                }
            }
            mcp_section.push_str("\nThese tools are available via the MCP protocol. You can call them by name when relevant to the user's request.\n");
            full_instruction.push_str(&mcp_section);
        }

        // Inject directory context so the agent knows where things are
        {
            let workspace_root = config_path.parent().unwrap_or(std::path::Path::new("."));
            let project_dir = std::env::current_dir().unwrap_or_default();
            let log_dir = project_dir.join("logs");
            let dir_context = format!(
                "\n\n## System Paths\n\n\
                - **Workspace root** (fs_pwd, config location): `{}`\n\
                - **Gateway project directory**: `{}`\n\
                - **Log files**: `{}`\n\
                - **Config file**: `{}`\n\
                - **Knowledge graph DB**: `{}`\n\n\
                When asked about logs, files, or the project, use these paths. \
                The filesystem tools (fs_list, fs_read, etc.) accept absolute paths.\n",
                workspace_root.display(),
                project_dir.display(),
                log_dir.display(),
                config_path.display(),
                workspace_root.join("knowledge_graph.db").display(),
            );
            full_instruction.push_str(&dir_context);
        }

        let mut rebuilt_builder = LlmAgentBuilder::new("assistant")
            .model(state.fallback_chain.primary().clone())
            .instruction(&full_instruction);

        // Re-attach KG tools
        if config.memory.is_some() {
            let kg_tools = crate::executable_tools::build_kg_tools(state.knowledge_graph.clone());
            for tool in kg_tools {
                rebuilt_builder = rebuilt_builder.tool(tool);
            }
        }

        // Attach all 6 agent management tools
        for tool in &state.agent_management_tools {
            rebuilt_builder = rebuilt_builder.tool(tool.clone());
        }

        // Attach MCP toolsets so the agent can call MCP tools directly
        let mcp_toolsets = state.mcp_manager.toolsets();
        for toolset in &mcp_toolsets {
            rebuilt_builder = rebuilt_builder.toolset(toolset.clone());
        }
        if !mcp_toolsets.is_empty() {
            tracing::info!("attached {} MCP toolsets to root agent", mcp_toolsets.len());
        }

        // Add observability callbacks for LLM requests and tool calls.
        // The before_model_callback also applies provider-specific schema sanitization
        // at model invocation time (Req 5.6). Original MCP tool schemas are stored
        // unmodified in gateway state; transformations happen here, lazily per-provider.
        let progress_map = state.progress_messages.clone();
        rebuilt_builder = rebuilt_builder.before_model_callback(Box::new(
            |_ctx, mut req| {
                Box::pin(async move {
                    let tool_count = req.tools.len();
                    tracing::info!(
                        model = %req.model,
                        tools_declared = tool_count,
                        "LLM request sending"
                    );

                    // Apply provider-specific schema sanitization at invocation time.
                    // Gemini requires certain JSON Schema properties to be removed or
                    // transformed; other providers receive schemas unmodified.
                    let provider = req.model.split('/').next().unwrap_or(&req.model);
                    let needs_sanitization = matches!(provider, "gemini" | "google");

                    if !req.tools.is_empty() {
                        use crate::schema_sanitizer::{GeminiSanitizer, IdentitySanitizer, SchemaSanitizer};
                        let sanitizer: &dyn SchemaSanitizer = if needs_sanitization {
                            &GeminiSanitizer
                        } else {
                            &IdentitySanitizer
                        };
                        let sanitized: std::collections::HashMap<String, serde_json::Value> = req
                            .tools
                            .into_iter()
                            .map(|(name, schema)| (name, sanitizer.sanitize(&schema)))
                            .collect();
                        req.tools = sanitized;
                        if needs_sanitization {
                            tracing::debug!(
                                tool_count = tool_count,
                                "applied Gemini schema sanitization to tool schemas"
                            );
                        }
                    }

                    Ok(adk_core::BeforeModelResult::Continue(req))
                })
            },
        ));

        rebuilt_builder = rebuilt_builder.after_model_callback(Box::new(
            |_ctx, resp| {
                Box::pin(async move {
                    // Only log when the response has actual content (final response, not intermediate chunks)
                    if let Some(ref content) = resp.content {
                        if content.parts.is_empty() {
                            return Ok(Some(resp));
                        }

                        let tool_names: Vec<&str> = content.parts.iter().filter_map(|p| {
                            if let adk_core::Part::FunctionCall { name, .. } = p {
                                Some(name.as_str())
                            } else {
                                None
                            }
                        }).collect();
                        if !tool_names.is_empty() {
                            tracing::info!(
                                tools = ?tool_names,
                                "LLM response contains tool calls"
                            );
                        }

                        if let Some(ref usage) = resp.usage_metadata {
                            tracing::info!(
                                input_tokens = usage.prompt_token_count,
                                output_tokens = usage.candidates_token_count,
                                total_tokens = usage.total_token_count,
                                "LLM response complete"
                            );
                        }
                    }
                    Ok(Some(resp))
                })
            },
        ));

        // Rate limiter: replaces the old dedup tracker with a sliding-window
        // rate limiter that detects runaway tool loops more robustly.
        let rate_limit_config = {
            let cfg = state.config.load();
            cfg.rate_limiter.clone()
        };
        let rate_limiter = Arc::new(Mutex::new(
            RateLimiter::new(rate_limit_config),
        ));
        // Log rate limiter configuration at startup for diagnostics
        {
            let mut rl = rate_limiter.lock().await;
            let rl_cfg = rl.config();
            tracing::debug!(
                max_calls = rl_cfg.max_calls,
                window_secs = rl_cfg.window_secs,
                cooldown_secs = rl_cfg.cooldown_secs,
                window_count = rl.window_count(Instant::now()),
                trigger_count = rl.trigger_count(),
                "rate limiter initialized"
            );
            // Reset to ensure clean state before first request
            rl.reset();
        }
        // Validate that with_defaults() produces a usable rate limiter
        let _ = RateLimiter::with_defaults();

        // Max iterations: gateway-level iteration counter that terminates
        // the tool-call loop when the configured limit is reached.
        // Implemented here (not in adk-rust) to avoid modifying upstream.
        // Uses resolve_max_iterations() to support per-agent overrides.
        let max_iterations_limit = {
            let cfg = state.config.load();
            cfg.runner.resolve_max_iterations(None)
        };
        let iteration_counter = Arc::new(std::sync::atomic::AtomicU32::new(0));

        rebuilt_builder = rebuilt_builder.after_tool_callback_full(Box::new(
            move |ctx, tool, args, result| {
                let progress = progress_map.clone();
                let limiter = rate_limiter.clone();
                let iter_counter = iteration_counter.clone();
                let max_iter = max_iterations_limit;
                Box::pin(async move {
                    let tool_name = tool.name().to_string();
                    let user_id = ctx.user_id().to_string();
                    tracing::info!(
                        tool = %tool_name,
                        args = %args,
                        result_size = result.to_string().len(),
                        "tool executed"
                    );

                    // Max iterations check: terminate if we've exceeded the limit
                    let iteration = iter_counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if iteration >= max_iter {
                        tracing::warn!(
                            tool = %tool_name,
                            iteration_count = iteration,
                            max_iterations = max_iter,
                            "max iterations reached, terminating request"
                        );
                        return Ok(Some(serde_json::json!({
                            "error": "MAX_ITERATIONS_REACHED",
                            "message": format!("Agent execution stopped: max iterations ({}) reached. Partial result returned.", max_iter),
                            "iteration_count": iteration,
                            "max_iterations_reached": true
                        })));
                    }

                    // Rate limiter check: sliding window detects rapid tool invocations
                    let decision = {
                        let mut rl = limiter.lock().await;
                        rl.record_invocation(&tool_name, Instant::now())
                    };

                    match decision {
                        RateLimitDecision::Pause { duration } => {
                            tracing::warn!(
                                tool = %tool_name,
                                cooldown_ms = duration.as_millis(),
                                "rate limit exceeded, pausing execution"
                            );
                            tokio::time::sleep(duration).await;
                        }
                        RateLimitDecision::Terminate { reason } => {
                            tracing::error!(
                                tool = %tool_name,
                                reason = %reason,
                                "rate limiter terminating request"
                            );
                            return Ok(Some(serde_json::json!({
                                "error": "RATE_LIMIT_TERMINATED",
                                "message": reason
                            })));
                        }
                        RateLimitDecision::Allow => {}
                    }

                    // Send progress message for user-visible tools
                    let emoji = match tool_name.as_str() {
                        name if name.starts_with("fs_") => "📂",
                        name if name.starts_with("kg_") => "🧠",
                        name if name.starts_with("browser_") => "🌐",
                        name if name.starts_with("agent_") => "🤖",
                        name if name.starts_with("task_") => "📅",
                        "screenshot" | "snapshot" => "📸",
                        "open_application" => "🚀",
                        "run_script" => "⚡",
                        "left_click" | "right_click" | "double_click" => "🖱️",
                        "type" | "key" => "⌨️",
                        "scrape" | "browser_navigate" => "🌐",
                        _ => "",
                    };

                    if !emoji.is_empty() {
                        let msg = format!("{} {}", emoji, tool_name.replace('_', " "));
                        progress.entry(user_id).or_default().push(msg);
                    }

                    Ok(None) // Don't modify the result
                })
            },
        ));

        let rebuilt_agent: Arc<dyn Agent> = Arc::new(rebuilt_builder.build()?);
        state
            .agents
            .insert(rebuilt_agent.name().to_string(), rebuilt_agent);
        tracing::info!(
            "attached {} agent management tools to root agent",
            state.agent_management_tools.len()
        );
    }

    // ── Restore paired users from previous session ─────────────────
    let pairing_file = config_path
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join("paired_users.json");
    {
        let users = crate::pairing::DmPairingService::load_persisted(&pairing_file);
        if !users.is_empty() {
            tracing::info!(count = users.len(), "restoring paired users from disk");
            state.pairing_service.restore_paired_users(users);
        }
    }

    // ── Auto-start agents with auto_start: true ────────────────────
    {
        let agents_to_start: Vec<String> = state
            .agent_registry
            .list()
            .into_iter()
            .filter(|(id, record)| {
                record.config.auto_start
                    && !state.agent_registry.is_system_agent(id)
                    && matches!(
                        record.state,
                        crate::agent_config::LifecycleState::Stopped
                            | crate::agent_config::LifecycleState::Created
                    )
            })
            .map(|(id, _)| id)
            .collect();

        for agent_id in &agents_to_start {
            tracing::info!(agent_id = %agent_id, "auto-starting agent");
            // Transition to Starting, build, spawn — best-effort at startup
            if let Err(e) = state
                .agent_registry
                .transition(agent_id, crate::agent_config::LifecycleState::Starting)
            {
                tracing::warn!(agent_id = %agent_id, error = %e, "failed to transition agent to Starting for auto-start");
                continue;
            }
            let config = match state.agent_registry.get(agent_id) {
                Some(r) => r.config.clone(),
                None => continue,
            };
            match state.agent_codegen.build_agent(&config).await {
                Ok(binary_path) => {
                    let mut env = std::collections::HashMap::new();
                    if let Ok(key_val) = std::env::var(&config.api_key_env) {
                        env.insert(config.api_key_env.clone(), key_val);
                    }
                    match state
                        .process_manager
                        .spawn(agent_id, &binary_path, env)
                        .await
                    {
                        Ok(port) => {
                            state.proxy_pool.register(agent_id, port);
                            if let Err(e) = state
                                .agent_registry
                                .transition(agent_id, crate::agent_config::LifecycleState::Running)
                            {
                                tracing::warn!(agent_id = %agent_id, error = %e, "failed to transition to Running");
                            } else {
                                tracing::info!(agent_id = %agent_id, port = port, "auto-started agent");
                            }
                        }
                        Err(e) => {
                            tracing::warn!(agent_id = %agent_id, error = %e, "failed to spawn agent for auto-start");
                            let _ = state.agent_registry.transition(
                                agent_id,
                                crate::agent_config::LifecycleState::Error {
                                    message: format!("auto-start spawn failed: {e}"),
                                },
                            );
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(agent_id = %agent_id, error = %e, "failed to build agent for auto-start");
                    let _ = state.agent_registry.transition(
                        agent_id,
                        crate::agent_config::LifecycleState::Error {
                            message: format!("auto-start build failed: {e}"),
                        },
                    );
                }
            }
        }
    }

    // ── Start ProcessManager health monitor ────────────────────────
    {
        let pm = state.process_manager.clone();
        let registry = state.agent_registry.clone();
        let pool = state.proxy_pool.clone();
        let ws_tx = state.control_panel.ws_broadcast.clone();
        pm.start_health_monitor(move |agent_id, reason| {
            tracing::error!(agent_id = %agent_id, reason = %reason, "agent health check failed");
            pool.remove(&agent_id);
            let _ = registry.transition(
                &agent_id,
                crate::agent_config::LifecycleState::Error { message: reason },
            );
            let _ = ws_tx.send(crate::control_panel::ws::WsEvent::AgentState {
                agent_id: agent_id.clone(),
                state: "Error".into(),
            });
        });
    }

    // ── Periodic dashboard broadcast (R7.3) ──────────────────────────
    // Emits WsEvent::Dashboard every 5 seconds so connected UI clients
    // receive live uptime, session count, and channel count updates.
    {
        let control_panel = state.control_panel.clone();
        let shutdown_token = state.shutdown.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let data = control_panel.dashboard();
                        let _ = control_panel.ws_broadcast.send(
                            crate::control_panel::ws::WsEvent::Dashboard {
                                uptime_secs: data.uptime_secs,
                                session_count: data.active_session_count,
                                channel_count: data.connected_channels.len(),
                            },
                        );
                    }
                    _ = shutdown_token.cancelled() => {
                        tracing::debug!("dashboard broadcast task stopping (shutdown)");
                        break;
                    }
                }
            }
        });
    }

    // ── Config watcher for hot-reload ──────────────────────────────
    let (_config_watcher, mut config_rx) = ConfigWatcher::start(config_path, state.config.clone())?;

    // Channel for forwarding cron config updates to the cron scheduler
    let (cron_update_tx, mut cron_update_rx) = mpsc::channel::<Vec<crate::config::CronJob>>(4);

    // ── Inbound message channel (created early so webhook_handler can be wired into hot-reload) ──
    let (inbound_tx, mut inbound_rx) = mpsc::channel::<InboundMessage>(256);

    // ── Webhook handler (created before hot-reload task so it can receive config updates) ──
    let webhook_handler = Arc::new(WebhookHandler::new(
        Arc::new(ArcSwap::new(Arc::new(config.hooks.clone()))),
        inbound_tx.clone(),
    ));

    // ── Hot-reload dispatch task ───────────────────────────────────
    // Receives new configs from ConfigWatcher and applies ConfigDiff-driven updates.
    tokio::spawn({
        let state = state.clone();
        let cron_update_tx = cron_update_tx;
        let webhook_handler = webhook_handler.clone();
        let mut old_config = config.clone();
        async move {
            use crate::config_watcher::ConfigDiff;
            while let Some(new_config) = config_rx.recv().await {
                let diff = ConfigDiff::compute(&old_config, &new_config);
                if !diff.has_changes() {
                    continue;
                }
                tracing::info!("applying hot-reload config changes");

                // Auth changed → rebuild access control (preserves paired users)
                if diff.auth_changed {
                    tracing::info!("hot-reload: rebuilding access control");
                    state.access_control.write().await.rebuild(&new_config);
                }

                // Cron changed → send new jobs to cron scheduler for reconciliation
                if diff.cron_changed {
                    tracing::info!("hot-reload: reconciling cron scheduler");
                    let _ = cron_update_tx.send(new_config.cron.jobs.clone()).await;
                }

                // Routing changed → update MessageRouter via ArcSwap
                if diff.routing_changed {
                    let default_agent = new_config
                        .agents
                        .list
                        .iter()
                        .find(|a| a.default)
                        .map(|a| a.id.clone())
                        .unwrap_or_else(|| "main".to_string());
                    let new_router = MessageRouter::new(&new_config.routing, default_agent);
                    state.router.store(Arc::new(new_router));
                    tracing::info!("hot-reload: message router updated");
                }

                // Channels changed → rebuild channel connections
                if diff.channels_changed {
                    tracing::info!("hot-reload: rebuilding channel connections");
                    state.channel_map.clear();
                    let new_channels = channel::build_channels(&new_config.channels);
                    for (key, ch) in &new_channels {
                        state.channel_map.insert(key.clone(), ch.clone());
                    }
                    tracing::info!(
                        count = new_channels.len(),
                        "hot-reload: channel map rebuilt"
                    );
                }

                // MCP servers changed → reconcile MCP connections
                if diff.plugins_changed || new_config.mcp_servers != old_config.mcp_servers {
                    tracing::info!("hot-reload: reconciling MCP connections");
                    state.mcp_manager.reconcile(&new_config.mcp_servers).await;
                }

                // RAG changed → reload RAG pipeline
                if diff.rag_changed {
                    if let (Some(rag_pipeline), Some(rag_config)) =
                        (&state.rag_pipeline, &new_config.rag)
                    {
                        tracing::info!("hot-reload: RAG config changed, pipeline reload requested");
                        let _ = rag_config;
                        let _ = rag_pipeline;
                    }
                }

                // Hooks changed → update webhook handler config
                if diff.hooks_changed {
                    tracing::info!("hot-reload: updating webhook handler config");
                    webhook_handler.update_config(new_config.hooks.clone());
                }

                // Coding agents changed → reload backends in registry
                if diff.coding_agents_changed {
                    if let Some(ref ca_registry) = state.coding_agent_registry {
                        tracing::info!("hot-reload: reloading coding agent backends");
                        ca_registry.reload_from_config(&new_config.coding_agents);
                    }
                }

                old_config = new_config;
            }
            tracing::info!("config watcher channel closed, hot-reload task exiting");
        }
    });

    // ── Start channels ─────────────────────────────────────────────

    let channels = channel::build_channels(&config.channels);
    if channels.is_empty() {
        tracing::warn!("no channels configured — gateway will only serve HTTP");
    }
    for (key, ch) in &channels {
        state.channel_map.insert(key.clone(), ch.clone());
        match ch.start(inbound_tx.clone()).await {
            Ok(()) => {
                tracing::info!(channel = %key, "channel started");
                state.metrics.set_channel_status(&key.to_string(), 1);
                state.control_panel.update_channels(
                    state
                        .channel_map
                        .iter()
                        .map(|e| crate::control_panel::ChannelInfo {
                            channel_type: e.key().channel_type.to_string(),
                            account_id: e.key().account_id.clone(),
                            status: "connected".into(),
                        })
                        .collect(),
                );
            }
            Err(e) => {
                tracing::error!(channel = %key, error = %e, "channel failed to start");
                state.metrics.set_channel_status(&key.to_string(), -1);
            }
        }
    }

    // ── Cron scheduler ─────────────────────────────────────────────
    let cron_scheduler = {
        let mut scheduler = match state.coding_agent_delegator.as_ref() {
            Some(delegator) => CronScheduler::with_delegator(inbound_tx.clone(), delegator.clone()),
            None => CronScheduler::new(inbound_tx.clone()),
        };

        // Auto-create system heartbeat job if not already configured
        let has_heartbeat = config.cron.jobs.iter().any(|j| j.id == "heartbeat");
        if !has_heartbeat {
            // Load HEARTBEAT.md content to inject into the prompt
            let heartbeat_content = {
                let cfg_parent = state.config_path.parent().unwrap_or(std::path::Path::new("."));
                let project_dir = std::env::current_dir().unwrap_or_default();
                let candidates = vec![
                    cfg_parent.join("context/HEARTBEAT.md"),
                    project_dir.join("context/HEARTBEAT.md"),
                    cfg_parent.join("HEARTBEAT.md"),
                ];
                candidates.iter()
                    .find_map(|p| std::fs::read_to_string(p).ok())
                    .unwrap_or_else(|| "Check if anything needs attention. If nothing, reply HEARTBEAT_OK.".to_string())
            };

            let heartbeat_prompt = format!("ask:{}\n\nIf nothing needs attention, reply with just HEARTBEAT_OK.", heartbeat_content);

            let heartbeat_job = crate::config::CronJob {
                id: "heartbeat".to_string(),
                schedule: "@every 1h".to_string(),
                message: heartbeat_prompt,
                deliver_to: Some(crate::config::CronDelivery {
                    channel: "telegram".to_string(),
                    target: "last".to_string(),
                }),
                suppress_keyword: Some("HEARTBEAT_OK".to_string()),
                target: None,
                workspace: None,
            };
            if let Err(e) = scheduler.schedule(heartbeat_job) {
                tracing::warn!(error = %e, "failed to schedule system heartbeat");
            } else {
                tracing::info!("system heartbeat scheduled (every 30m)");
            }
        }

        for job in &config.cron.jobs {
            if let Err(e) = scheduler.schedule(job.clone()) {
                tracing::error!(job_id = %job.id, error = %e, "failed to schedule cron job");
            }
        }
        if scheduler.job_count() > 0 {
            tracing::info!(jobs = scheduler.job_count(), "cron scheduler started");
        }
        scheduler
    };

    // Store the cron scheduler in shared state so HTTP endpoints can access it
    *state.cron_scheduler.lock().await = Some(cron_scheduler);

    // ── Cron hot-reload receiver ───────────────────────────────────
    // Receives cron job updates from the hot-reload dispatch task.
    {
        let cron_scheduler = state.cron_scheduler.clone();
        tokio::spawn(async move {
            while let Some(new_jobs) = cron_update_rx.recv().await {
                let mut guard = cron_scheduler.lock().await;
                if let Some(ref mut scheduler) = *guard {
                    scheduler.reconcile(&new_jobs);
                    tracing::info!(
                        active = scheduler.active_job_ids().len(),
                        "cron scheduler reconciled"
                    );
                }
            }
        });
    }

    // ── Message processing loop ────────────────────────────────────
    let processor = tokio::spawn({
        let state = state.clone();
        async move {
            tracing::info!("message processor started");
            while let Some(msg) = inbound_rx.recv().await {
                if !state.shutdown_coordinator.is_accepting() {
                    continue;
                }
                let state = state.clone();
                tokio::spawn(async move {
                    let _guard = match state.shutdown_coordinator.acquire() {
                        Some(g) => g,
                        None => return,
                    };
                    // Per-message timeout to prevent hung operations
                    let timeout = std::time::Duration::from_secs(90); // 90 seconds max per message
                    match tokio::time::timeout(timeout, process_message(msg, &state)).await {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => tracing::error!(error = %e, "failed to process message"),
                        Err(_) => {
                            tracing::error!("message processing timed out after {:?}", timeout)
                        }
                    }
                });
            }
        }
    });

    // ── SIGUSR1 graceful restart handler (Unix only) ───────────────
    #[cfg(unix)]
    {
        let coordinator = state.shutdown_coordinator.clone();
        crate::shutdown::register_sigusr1_handler(coordinator).await;
        tracing::info!("SIGUSR1 graceful restart handler registered");
    }

    // ── Log retention background task ──────────────────────────────
    if let Some(ref log_dir) = config.telemetry.log_dir {
        let _log_retention_handle = crate::telemetry::spawn_log_retention_task(
            log_dir.clone(),
            config.telemetry.log_rotation.clone(),
            state.shutdown.clone(),
        );
        tracing::info!(log_dir = %log_dir, "log retention task started");
    }

    // ── HTTP server ────────────────────────────────────────────────
    let app = gateway_routes::build_router(&state, webhook_handler);
    let bind_addr = format!("{}:{}", config.gateway.bind.to_addr(), port);
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    const ADK_VERSION: &str = "0.8.4";
    tracing::info!(
        gateway = env!("CARGO_PKG_VERSION"),
        adk_rust = ADK_VERSION,
        addr = %bind_addr,
        "HTTP server listening"
    );
    tracing::info!("📊 Control panel: http://{bind_addr}/ui");

    // ── sd_notify(READY=1) — Systemd readiness signaling ───────────────────
    // When running under systemd with Type=notify, the gateway signals that
    // initialization is complete and it is ready to accept connections.
    // This is called AFTER the TCP listener is bound and all subsystems are
    // initialized, but BEFORE entering the main event loop.
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = sd_notify::notify(false, &[sd_notify::NotifyState::Ready]) {
            tracing::warn!("sd_notify READY=1 failed: {e}");
        } else {
            tracing::info!("sd_notify: READY=1 (systemd readiness signal sent)");
        }
    }

    let shutdown_coordinator = state.shutdown_coordinator.clone();
    let shutdown = state.shutdown.clone();

    tokio::select! {
        result = axum::serve(listener, app) => {
            if let Err(e) = result { tracing::error!(error = %e, "HTTP server error"); }
        }
        _ = processor => {
            tracing::info!("message processor stopped");
        }
        _ = shutdown.cancelled() => {
            tracing::info!("shutdown signal received");
            // Shut down channels in parallel with a timeout
            let shutdown_futures: Vec<_> = state.channel_map.iter().map(|entry| {
                let ch = entry.value().clone();
                let key = entry.key().to_string();
                let metrics = state.metrics.clone();
                async move {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        ch.shutdown(),
                    ).await {
                        Ok(Ok(())) => tracing::info!(channel = %key, "channel shut down"),
                        Ok(Err(e)) => tracing::warn!(channel = %key, error = %e, "channel shutdown error"),
                        Err(_) => tracing::warn!(channel = %key, "channel shutdown timed out"),
                    }
                    metrics.set_channel_status(&key, 0);
                }
            }).collect();
            futures::future::join_all(shutdown_futures).await;
        }
        _ = tokio::signal::ctrl_c() => {
            tracing::info!(
                drain_timeout_secs = shutdown_coordinator.drain_timeout().as_secs(),
                "ctrl-c received, draining..."
            );
            shutdown_coordinator.initiate_shutdown().await;
            if shutdown_coordinator.is_restart() {
                tracing::info!("shutdown was triggered by restart (SIGUSR1)");
            }
            // Shut down channels in parallel with a timeout
            let shutdown_futures: Vec<_> = state.channel_map.iter().map(|entry| {
                let ch = entry.value().clone();
                let key = entry.key().to_string();
                let metrics = state.metrics.clone();
                async move {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        ch.shutdown(),
                    ).await {
                        Ok(Ok(())) => tracing::info!(channel = %key, "channel shut down"),
                        Ok(Err(e)) => tracing::warn!(channel = %key, error = %e, "channel shutdown error"),
                        Err(_) => tracing::warn!(channel = %key, "channel shutdown timed out"),
                    }
                    metrics.set_channel_status(&key, 0);
                }
            }).collect();
            futures::future::join_all(shutdown_futures).await;
        }
    }

    // ── Stop all running agent processes before exiting ───────────
    {
        let running_agents: Vec<String> = state
            .agent_registry
            .list()
            .into_iter()
            .filter(|(_, record)| {
                matches!(record.state, crate::agent_config::LifecycleState::Running)
            })
            .map(|(id, _)| id)
            .collect();

        if !running_agents.is_empty() {
            tracing::info!(
                count = running_agents.len(),
                "stopping running agent processes during shutdown"
            );
        }

        for agent_id in &running_agents {
            tracing::info!(agent_id = %agent_id, "stopping agent process during shutdown");
            if let Err(e) = state
                .process_manager
                .stop(agent_id, std::time::Duration::from_secs(5))
                .await
            {
                tracing::warn!(agent_id = %agent_id, error = %e, "failed to stop agent during shutdown");
            }
            state.proxy_pool.remove(agent_id);
            let _ = state
                .agent_registry
                .transition(agent_id, crate::agent_config::LifecycleState::Stopping);
            let _ = state
                .agent_registry
                .transition(agent_id, crate::agent_config::LifecycleState::Stopped);
        }

        if !running_agents.is_empty() {
            tracing::info!("all agent processes stopped during shutdown");
        }
    }

    // ── Persist paired users before exiting ────────────────────────
    if let Err(e) = state.pairing_service.persist(&pairing_file) {
        tracing::error!(error = %e, "failed to persist paired users on shutdown");
    }

    Ok(())
}

// ── Message processing ─────────────────────────────────────────────

async fn process_message(msg: InboundMessage, state: &GatewayState) -> anyhow::Result<()> {
    let start = Instant::now();
    let channel_name = msg.channel_type.to_string();

    // Handle /stop command — cancel any active request for this sender
    if msg.text.trim().eq_ignore_ascii_case("/stop") || msg.text.trim().eq_ignore_ascii_case("stop") {
        if let Some((_, token)) = state.active_requests.remove(&msg.sender_id) {
            token.cancel();
            send_reply(&msg, state, "⏹ Stopped. Your previous request has been cancelled.").await;
            tracing::info!(sender = %msg.sender_id, "user cancelled active request via /stop");
        } else {
            send_reply(&msg, state, "Nothing running to stop.").await;
        }
        return Ok(());
    }

    // Access control — skip for cron messages (system-generated)
    if !matches!(msg.source, crate::channel::MessageSource::Cron { .. }) {
        let ac = state.access_control.read().await;
        match ac.check_message_access(&msg) {
            AuthDecision::Allowed => {}
            AuthDecision::Denied { reason } => {
                tracing::info!(sender = %msg.sender_id, reason = %reason, "access denied");
                state.metrics.record_message(
                    &channel_name,
                    MessageStatus::Failure,
                    start.elapsed(),
                    None,
                    None,
                    None,
                );
                send_reply(&msg, state, &format!("⛔ Access denied: {reason}")).await;
                return Ok(());
            }
            AuthDecision::RequiresPairing => {
                let code = msg.text.trim();
                if code.len() == 6 && code.chars().all(|c| c.is_ascii_alphanumeric()) {
                    let result =
                        state
                            .pairing_service
                            .validate_code(&msg.sender_id, code, &channel_name);
                    match result {
                        PairingResult::Success => {
                            let id = ac.map_identity(&msg);
                            drop(ac);
                            state
                                .access_control
                                .write()
                                .await
                                .mark_paired(&id.canonical_id);
                            send_reply(&msg, state, "✅ Paired! You can now use the bot.").await;
                            return Ok(());
                        }
                        PairingResult::Locked { .. } => {
                            send_reply(
                                &msg,
                                state,
                                "🔒 Too many failed attempts. Try again later.",
                            )
                            .await;
                            return Ok(());
                        }
                        _ => {
                            send_reply(&msg, state, "❌ Invalid code. Try again.").await;
                            return Ok(());
                        }
                    }
                }
                send_reply(
                    &msg,
                    state,
                    "🔑 Send your 6-digit pairing code to get started.",
                )
                .await;
                return Ok(());
            }
        }
    } // end access control (skipped for cron)

    // For cron messages: verify at least one user is paired on the target channel.
    // If no one is paired, skip processing (don't waste LLM calls with no delivery target).
    // Also verify the paired user would pass access control (not stale pairing).
    if matches!(msg.source, crate::channel::MessageSource::Cron { .. }) {
        let paired = state.pairing_service.paired_users();
        let channel_str = format!("{}", msg.channel_type);
        let paired_on_channel: Vec<_> = paired.iter()
            .filter(|u| u.channel_type == channel_str)
            .collect();

        if paired_on_channel.is_empty() {
            tracing::info!(
                job_id = %msg.sender_id,
                channel = %msg.channel_type,
                "cron job skipped — no paired users on target channel"
            );
            if let crate::channel::MessageSource::Cron { ref job_id } = msg.source {
                state.task_log.log(job_id, crate::task_log::EVENT_SKIPPED, "No paired users on target channel");
            }
            return Ok(());
        }

        // Verify the paired user would pass access control
        let ac = state.access_control.read().await;
        let test_msg = crate::channel::InboundMessage {
            sender_id: paired_on_channel[0].user_id.clone(),
            ..msg.clone()
        };
        match ac.check_message_access(&test_msg) {
            AuthDecision::Allowed => {} // Good — deliver
            _ => {
                tracing::info!(
                    job_id = %msg.sender_id,
                    user = %paired_on_channel[0].user_id,
                    "cron job skipped — paired user would not pass access control"
                );
                if let crate::channel::MessageSource::Cron { ref job_id } = msg.source {
                    state.task_log.log(job_id, crate::task_log::EVENT_SKIPPED, "Paired user would not pass access control");
                }
                return Ok(());
            }
        }

        // Log that the cron job is being processed
        if let crate::channel::MessageSource::Cron { ref job_id } = msg.source {
            state.task_log.log(job_id, crate::task_log::EVENT_FIRED, &format!("Processing: {}", msg.text));
        }
    }

    // ── Plugin: OnUserMessage hook (R8) — fired when message first arrives ──
    {
        let mut on_msg_ctx = crate::plugin_manager::HookContext::default();
        on_msg_ctx.user_message = Some(msg.text.clone());
        let hook_result = state.plugin_manager.invoke_hook(
            crate::plugin_manager::PluginHook::OnUserMessage,
            &mut on_msg_ctx,
        );
        if let crate::plugin_manager::HookResult::ShortCircuit { response } = hook_result {
            send_reply(&msg, state, &response).await;
            return Ok(());
        }
    }

    // Session
    // Capture last_activity BEFORE resolve_session updates it (for stale context detection)
    let previous_last_activity = state.session_bridge.get_last_activity(&msg);
    let (user_id, session_id) = state.session_bridge.resolve_session(&msg);
    let router_guard = state.router.load();
    let agent_id = router_guard.resolve_agent(&msg);
    tracing::info!(channel = %msg.channel_type, sender = %msg.sender_id, agent = agent_id, session = %session_id, "processing");

    // ── Stale Context Detection (Req 2) ──
    // Check if the user has been idle beyond the configured threshold.
    // We captured last_activity before resolve_session updated it to now.
    {
        let config_guard = state.config.load();
        let stale_config = &config_guard.stale_context;
        let detector = crate::stale_context::StaleContextDetector::new(stale_config.clone());
        let default_detector = crate::stale_context::StaleContextDetector::with_defaults();
        tracing::debug!(
            idle_threshold_secs = detector.idle_threshold_secs(),
            using_custom_config = (detector.config() != default_detector.config()),
            "stale context check"
        );

        if let Some(last_activity) = previous_last_activity {
            let now = chrono::Utc::now();
            if detector.is_stale(last_activity, now) {
                let idle_duration = detector.idle_duration(last_activity, now);
                // Build welcome-back message (no pending tasks/alerts yet — those require
                // heartbeat V2 integration which is a separate task)
                let welcome_msg = detector.build_welcome_back(
                    idle_duration,
                    &[], // pending tasks — populated when heartbeat V2 is integrated
                    &[], // heartbeat alerts — populated when heartbeat V2 is integrated
                );
                tracing::info!(
                    sender = %msg.sender_id,
                    idle_secs = idle_duration.num_seconds(),
                    "stale context detected, sending welcome-back"
                );
                send_reply(&msg, state, &welcome_msg).await;
            }
        }
    }

    // Send typing indicator to show the bot is working.
    // Spawn a background task that re-sends typing every 4 seconds until dropped.
    // Also sends progress messages when tools are being executed.
    let typing_cancel = tokio_util::sync::CancellationToken::new();
    {
        let key = crate::channel::ChannelKey {
            channel_type: msg.channel_type,
            account_id: msg.account_id.clone(),
        };
        if let Some(ch) = state.channel_map.get(&key) {
            let chat_id = if msg.is_group {
                msg.group_id.clone().unwrap_or_else(|| msg.sender_id.clone())
            } else {
                msg.sender_id.clone()
            };
            // Send initial typing
            let _ = ch.send_typing(&chat_id).await;

            // Spawn a loop that keeps typing alive and sends progress updates
            let channel = ch.value().clone();
            let cancel = typing_cancel.clone();
            let progress_map = state.progress_messages.clone();
            let progress_user_id = user_id.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = tokio::time::sleep(std::time::Duration::from_secs(4)) => {
                            // Check for progress messages to send
                            let messages: Vec<String> = progress_map
                                .get_mut(&progress_user_id)
                                .map(|mut entry| entry.value_mut().drain(..).collect())
                                .unwrap_or_default();

                            for msg_text in messages {
                                let out = crate::channel::OutboundMessage {
                                    channel_type: crate::channel::ChannelType::Telegram,
                                    account_id: String::new(),
                                    recipient_id: chat_id.clone(),
                                    text: msg_text,
                                    reply_to: None,
                                    is_partial: true,
                                };
                                let _ = channel.send(out).await;
                            }
                            let _ = channel.send_typing(&chat_id).await;
                        }
                    }
                }
            });
        }
    }

    // Log to control panel
    state.control_panel.push_log(LogEntry {
        timestamp: chrono::Utc::now().to_rfc3339(),
        level: "INFO".into(),
        message: format!("msg from {} via {}", msg.sender_id, msg.channel_type),
        target: Some("gateway".into()),
    });

    // Update active session count on control panel
    state.control_panel.update_sessions(
        state
            .session_bridge
            .active_sessions()
            .iter()
            .map(|s| crate::control_panel::SessionInfo {
                session_id: s.session_id.clone(),
                user_id: s.user_id.clone(),
                channel_type: s.channel_type.to_string(),
                last_activity: s.last_activity.to_rfc3339(),
            })
            .collect(),
    );

    // Get existing session or create a new one.
    // IMPORTANT: create() overwrites the session (wiping conversation history),
    // so we must only call it when the session doesn't exist yet.
    let session_exists = state
        .session_service
        .get(adk_session::GetRequest {
            app_name: state.session_bridge.app_name().to_string(),
            user_id: user_id.clone(),
            session_id: session_id.clone(),
            num_recent_events: None,
            after: None,
        })
        .await
        .is_ok();

    if !session_exists {
        let _ = state
            .session_service
            .create(adk_session::CreateRequest {
                app_name: state.session_bridge.app_name().to_string(),
                user_id: user_id.clone(),
                session_id: Some(session_id.clone()),
                state: std::collections::HashMap::new(),
            })
            .await;
        tracing::info!(user = %user_id, session = %session_id, "created new adk session");
    } else {
        tracing::debug!(user = %user_id, session = %session_id, "reusing existing adk session");
    }

    // Resolve tools from registry for this turn
    let resolved_tools = state.tool_registry.resolve_all();

    // Log resolved tools for this agent turn (R17.5)
    crate::tool_registry::ToolRegistry::log_registered_tools(agent_id, &resolved_tools);

    // Scope resolution and tool access checks (R4)
    let canonical_id = format!("{}:{}", msg.channel_type, msg.sender_id);
    let user_scopes = state.scope_resolver.resolve_scopes(&canonical_id);
    tracing::debug!(
        user = %canonical_id,
        scopes = ?user_scopes,
        "resolved user scopes"
    );

    // Check tool access for each resolved tool via wrap_tool_check
    let mut allowed_tools = Vec::new();
    let mut denied_reasons = Vec::new();
    {
        let ac = state.access_control.read().await;
        for tool in &resolved_tools {
            let decision = ac.wrap_tool_check(
                &canonical_id,
                &tool.name,
                None, // required_role: tools don't carry role requirements in config
                &[],  // required_scopes: tools don't carry scope requirements in config
                state.scope_resolver.as_ref(),
            );
            match decision {
                crate::access_control::ToolAccessDecision::Allowed => {
                    allowed_tools.push(tool.clone());
                }
                crate::access_control::ToolAccessDecision::Denied { reason } => {
                    denied_reasons.push(reason);
                }
            }
        }
    }

    // If any tools were denied, log and inform the user about denied tools
    if !denied_reasons.is_empty() {
        tracing::info!(
            user = %canonical_id,
            denied_count = denied_reasons.len(),
            "some tools denied by access control"
        );
        // If ALL tools were denied, inform the user and stop processing
        if allowed_tools.is_empty() && !resolved_tools.is_empty() {
            let reasons = denied_reasons.join("; ");
            send_reply(&msg, state, &format!("⛔ Tool access denied: {reasons}")).await;
            return Ok(());
        }
    }

    // Skill selection and context building (R16)
    let skill_context =
        ContextCoordinator::select_skill(&msg.text, &state.skill_index).map(|skill| {
            let filtered = ContextCoordinator::filter_tools(&resolved_tools, skill);
            let ctx = ContextCoordinator::build_context(skill, &msg.text);
            tracing::info!(
                skill = %skill.name,
                filtered_tools = filtered.len(),
                references = ctx.references.len(),
                "skill selected for message"
            );
            (filtered, ctx)
        });

    // If a skill was selected, narrow allowed_tools to only those the skill permits
    if let Some((ref skill_tools, _)) = skill_context {
        if !skill_tools.is_empty() {
            let skill_tool_names: std::collections::HashSet<&str> =
                skill_tools.iter().map(|t| t.name.as_str()).collect();
            allowed_tools.retain(|t| skill_tool_names.contains(t.name.as_str()));
            tracing::debug!(
                allowed = allowed_tools.len(),
                "tools filtered by active skill"
            );
        }
    }

    // Invoke plugin BeforeRun hook (legacy, kept for backward compat)
    let mut hook_ctx = crate::plugin_manager::HookContext::default();
    hook_ctx.user_message = Some(msg.text.clone());
    let hook_result = state
        .plugin_manager
        .invoke_hook(crate::plugin_manager::PluginHook::BeforeRun, &mut hook_ctx);
    if let crate::plugin_manager::HookResult::ShortCircuit { response } = hook_result {
        send_reply(&msg, state, &response).await;
        return Ok(());
    }

    // ── RBAC check before routing (Task 10.7) ──
    // Only check RBAC for agents managed by the multi-agent registry.
    // In-process agents (in state.agents map) bypass RBAC — they are
    // the gateway's own agents configured via the config file.
    let is_managed_agent =
        state.agent_registry.get(agent_id).is_some() && !state.agents.contains_key(agent_id);
    if is_managed_agent {
        if let Err(denied) = state.rbac.check_tool(agent_id, "message_receive") {
            tracing::info!(
                agent_id = agent_id,
                error = %denied,
                "RBAC denied message routing to agent"
            );
            send_reply(
                &msg,
                state,
                &format!(
                    "⛔ Agent '{}' is not permitted to receive messages.",
                    agent_id
                ),
            )
            .await;
            state.metrics.record_message(
                &channel_name,
                MessageStatus::Failure,
                start.elapsed(),
                None,
                None,
                None,
            );
            return Ok(());
        }
    }

    // ── Multi-agent routing (Tasks 10.3–10.6) ──
    // Check if the target agent is a User Agent in the proxy pool.
    // If so, route via the remote proxy. If it's the system agent,
    // fall through to the existing in-process path.
    let is_system = state.agent_registry.is_system_agent(agent_id);
    if !is_system {
        if let Some(proxy) = state.proxy_pool.get(agent_id) {
            // Task 10.4: Target is a running User Agent — route via proxy.
            let proxy_url = format!("{}/run", proxy.agent_url());
            tracing::info!(agent_id = agent_id, url = %proxy_url, "routing message to user agent via proxy");

            let client = reqwest::Client::new();
            let a2a_payload = serde_json::json!({
                "app_name": state.session_bridge.app_name(),
                "user_id": user_id,
                "session_id": session_id,
                "content": { "role": "user", "parts": [{ "text": msg.text }] },
            });

            match client
                .post(&proxy_url)
                .json(&a2a_payload)
                .timeout(std::time::Duration::from_secs(120))
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    let body = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "Agent responded but body unreadable.".into());
                    // Try to extract text from the A2A response
                    let response_text = serde_json::from_str::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|v| v.get("text").and_then(|t| t.as_str()).map(String::from))
                        .unwrap_or(body);
                    send_reply(&msg, state, &response_text).await;
                    state.metrics.record_message(
                        &channel_name,
                        MessageStatus::Success,
                        start.elapsed(),
                        None,
                        None,
                        None,
                    );
                    return Ok(());
                }
                Ok(resp) => {
                    let status = resp.status();
                    tracing::warn!(agent_id = agent_id, status = %status, "proxy agent returned error");
                    send_reply(
                        &msg,
                        state,
                        &format!(
                            "⚠️ Agent '{}' returned an error (HTTP {}).",
                            agent_id, status
                        ),
                    )
                    .await;
                    state.metrics.record_message(
                        &channel_name,
                        MessageStatus::Failure,
                        start.elapsed(),
                        None,
                        None,
                        None,
                    );
                    return Ok(());
                }
                Err(e) => {
                    tracing::error!(agent_id = agent_id, error = %e, "failed to reach proxy agent");
                    send_reply(
                        &msg,
                        state,
                        &format!(
                            "⚠️ Agent '{}' is unreachable. Please try again later.",
                            agent_id
                        ),
                    )
                    .await;
                    state.metrics.record_message(
                        &channel_name,
                        MessageStatus::Failure,
                        start.elapsed(),
                        None,
                        None,
                        None,
                    );
                    return Ok(());
                }
            }
        } else {
            // Task 10.6: Target agent is not running — check registry state.
            if let Some(record) = state.agent_registry.get(agent_id) {
                let state_desc = match &record.state {
                    crate::agent_config::LifecycleState::Created => "created but not started",
                    crate::agent_config::LifecycleState::Starting => "still starting up",
                    crate::agent_config::LifecycleState::Stopping => "shutting down",
                    crate::agent_config::LifecycleState::Stopped => "stopped",
                    crate::agent_config::LifecycleState::Error { message } => {
                        tracing::warn!(agent_id = agent_id, error = %message, "routed to errored agent");
                        "in error state"
                    }
                    crate::agent_config::LifecycleState::Running => "running but not in proxy pool",
                };
                send_reply(
                    &msg,
                    state,
                    &format!(
                        "⚠️ Agent '{}' is unavailable ({}). Please try again later.",
                        agent_id, state_desc
                    ),
                )
                .await;
                state.metrics.record_message(
                    &channel_name,
                    MessageStatus::Failure,
                    start.elapsed(),
                    None,
                    None,
                    None,
                );
                return Ok(());
            }
            // Agent not in registry at all — fall through to in-process lookup
        }
    }

    // ── Graph workflow execution path (R1.3–R1.5) ──
    // If the routed agent_id is not in the agents map and a graph workflow is configured,
    // execute the workflow instead of the normal agent path.
    if !state.agents.contains_key(agent_id) {
        if let Some(ref workflow) = state.graph_workflow {
            tracing::info!(agent_id = agent_id, "routed to graph workflow");
            let mut initial_state = crate::graph_workflow::WorkflowState::new();
            initial_state.insert(
                "user_message".to_string(),
                serde_json::Value::String(msg.text.clone()),
            );
            let exec_ctx = crate::graph_workflow::WorkflowExecutionContext {
                action_executor: &state.action_executor,
                agents: &state.agents,
                session_service: &state.session_service,
            };
            match workflow.execute(initial_state, &exec_ctx) {
                Ok(result) => {
                    let result_text = result
                        .state
                        .get("response")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| {
                            format!("Workflow completed in {} iterations.", result.iterations)
                        });
                    tracing::info!(
                        iterations = result.iterations,
                        nodes_executed = result.executed_nodes.len(),
                        "graph workflow completed"
                    );
                    send_reply(&msg, state, &result_text).await;
                    state.metrics.record_message(
                        &channel_name,
                        MessageStatus::Success,
                        start.elapsed(),
                        None,
                        None,
                        None,
                    );
                    return Ok(());
                }
                Err(e) => {
                    tracing::error!(error = %e, "graph workflow execution failed");
                    send_reply(
                        &msg,
                        state,
                        "⚠️ Workflow execution failed. Please try again later.",
                    )
                    .await;
                    state.metrics.record_message(
                        &channel_name,
                        MessageStatus::Failure,
                        start.elapsed(),
                        None,
                        None,
                        None,
                    );
                    return Ok(());
                }
            }
        }
    }

    // Task 10.5: System Agent or in-process agent lookup
    let agent = state
        .agents
        .get(agent_id)
        .or_else(|| state.agents.get("assistant"))
        .map(|e| e.value().clone())
        .ok_or_else(|| anyhow::anyhow!("no agent: {agent_id}"))?;

    // ── Plugin: BeforeAgent hook (R8) — before agent execution starts ──
    {
        let mut before_agent_ctx = crate::plugin_manager::HookContext::default();
        before_agent_ctx.user_message = Some(msg.text.clone());
        before_agent_ctx.agent_name = Some(agent.name().to_string());
        let hook_result = state.plugin_manager.invoke_hook(
            crate::plugin_manager::PluginHook::BeforeAgent,
            &mut before_agent_ctx,
        );
        if let crate::plugin_manager::HookResult::ShortCircuit { response } = hook_result {
            send_reply(&msg, state, &response).await;
            return Ok(());
        }
    }

    // ── Plugin: BeforeModel hook (R8) — before model invocation ──
    {
        let mut before_model_ctx = crate::plugin_manager::HookContext::default();
        before_model_ctx.user_message = Some(msg.text.clone());
        before_model_ctx.agent_name = Some(agent.name().to_string());
        before_model_ctx.model_name = Some(state.config.load().agent.model.primary().to_string());
        let hook_result = state.plugin_manager.invoke_hook(
            crate::plugin_manager::PluginHook::BeforeModel,
            &mut before_model_ctx,
        );
        if let crate::plugin_manager::HookResult::ShortCircuit { response } = hook_result {
            send_reply(&msg, state, &response).await;
            return Ok(());
        }
    }

    // ── Memory context: inject full entity summary for this user ────
    // Instead of searching by query (which misses unrelated entities),
    // we always inject the full entity summary so the agent has complete
    // awareness of everything stored about this user.
    let mut memory_context = String::new();
    if let Some(ref mem_cfg) = state.config.load().memory {
        // First, check the cached summary
        let cached = state.memory_summaries.get(&user_id).map(|v| v.clone());
        let summary = cached.unwrap_or_else(|| {
            // Build fresh if not cached
            let s = state
                .knowledge_graph
                .build_entity_summary(&user_id, mem_cfg.summary_observations);
            if !s.is_empty() {
                state.memory_summaries.insert(user_id.clone(), s.clone());
            }
            s
        });

        if !summary.is_empty() {
            // Check if onboarding is complete by looking for the marker entity
            let onboarding_done = state
                .knowledge_graph
                .search_nodes(&user_id, "onboarding_complete")
                .iter()
                .any(|r| r.entity.name == "onboarding_complete");

            memory_context.push_str("\n\n");
            memory_context.push_str(&summary);

            if !onboarding_done {
                // Inject bootstrap instructions — user has entities but hasn't
                // completed onboarding (e.g., only auto-stored conversation turns)
                let bootstrap =
                    load_context_file(&state.config.load(), &state.config_path, "BOOTSTRAP.md");
                if !bootstrap.is_empty() {
                    memory_context.push_str(
                        "\n\n[Onboarding not yet complete — follow BOOTSTRAP.md instructions]\n",
                    );
                    memory_context.push_str(&bootstrap);
                    tracing::info!(user = %user_id, "memory: injecting bootstrap (onboarding incomplete)");
                }
            }

            // Also do a targeted search for query-specific context that might
            // surface relevant observations not in the top-N summary
            let kg_results = state.knowledge_graph.search_nodes(&user_id, &msg.text);
            if !kg_results.is_empty() {
                memory_context.push_str("\n[Query-relevant memory matches]\n");
                for r in &kg_results {
                    memory_context.push_str(&format!(
                        "- {} ({}){}\n",
                        r.entity.name,
                        r.entity.entity_type,
                        if r.entity.observations.is_empty() {
                            String::new()
                        } else {
                            format!(
                                ": {}",
                                r.entity
                                    .observations
                                    .iter()
                                    .rev()
                                    .take(5)
                                    .map(|o| o.content.as_str())
                                    .collect::<Vec<_>>()
                                    .into_iter()
                                    .rev()
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            )
                        },
                    ));
                }
            }

            tracing::info!(
                user = %user_id,
                summary_len = summary.len(),
                search_results = kg_results.len(),
                "memory: injecting entity summary + search context"
            );
        } else {
            // Empty memory — this is a brand new user. Inject bootstrap.
            let bootstrap =
                load_context_file(&state.config.load(), &state.config_path, "BOOTSTRAP.md");
            if !bootstrap.is_empty() {
                memory_context.push_str(
                    "\n\n[New user — no memory stored yet. Follow onboarding instructions.]\n",
                );
                memory_context.push_str(&bootstrap);
                tracing::info!(user = %user_id, "memory: new user, injecting bootstrap for onboarding");
            } else {
                tracing::debug!(user = %user_id, "memory: no entities stored yet, no bootstrap file");
            }
        }
    }

    // ── RAG context: search for relevant documents ─────────────────
    let mut rag_context = String::new();
    if let Some(ref rag) = state.rag_pipeline {
        let rag_results = rag.search(&msg.text, 3);
        if !rag_results.is_empty() {
            rag_context.push_str("\n\n[Relevant knowledge base context]\n");
            for r in &rag_results {
                rag_context.push_str(&format!("- {}\n", r.text));
            }
            tracing::info!(
                results = rag_results.len(),
                scores = ?rag_results.iter().map(|r| r.score).collect::<Vec<_>>(),
                "rag: injecting document context into prompt"
            );
        } else {
            tracing::debug!(query = %msg.text, "rag: no results for query");
        }
    }

    // Build the user content with injected context
    let mut augmented_text = if memory_context.is_empty() && rag_context.is_empty() {
        msg.text.clone()
    } else {
        format!("{}{}{}", msg.text, memory_context, rag_context)
    };

    // Inject skill instructions if a skill was selected
    if let Some((_, ref ctx)) = skill_context {
        if !ctx.instructions.is_empty() {
            augmented_text = format!(
                "[Skill: {}]\n{}\n\n{}",
                ctx.instructions,
                if ctx.references.is_empty() {
                    String::new()
                } else {
                    format!("[References: {}]\n", ctx.references.join(", "))
                },
                augmented_text
            );
            tracing::debug!("skill instructions injected into prompt");
        }
    }

    // Run agent — register cancellation token for /stop support
    let request_cancel = tokio_util::sync::CancellationToken::new();
    state.active_requests.insert(msg.sender_id.clone(), request_cancel.clone());

    let runner = Runner::new(RunnerConfig {
        app_name: state.session_bridge.app_name().to_string(),
        agent: agent.clone(),
        session_service: state.session_service.clone(),
        artifact_service: None,
        memory_service: None,
        plugin_manager: None,
        run_config: Some({
            let rc = adk_core::RunConfig::default();
            rc
        }),
        compaction_config: None,
        context_cache_config: None,
        cache_capable: None,
        request_context: None,
        cancellation_token: Some(request_cancel.clone()),
        intra_compaction_config: None,
        intra_compaction_summarizer: None,
    })?;

    let uid = adk_core::UserId::try_from(user_id.as_str())?;
    let sid = adk_core::SessionId::try_from(session_id.as_str())?;
    let content = Content {
        role: "user".into(),
        parts: vec![Part::Text {
            text: augmented_text,
        }],
    };

    // ── Fallback chain integration (R16, Task 4.2) ─────────────────
    // When no fallbacks are configured, use the direct call path (zero overhead).
    // When fallbacks exist, wrap with run_with_fallback to retry on failure.
    let stream = if !state.fallback_chain.has_fallbacks() {
        // Direct path — no fallback overhead (backward compat)
        match runner.run(uid, sid, content).await {
            Ok(s) => {
                if let Some(ref awp) = state.awp_state {
                    crate::awp::report_healthy(awp).await;
                }
                s
            }
            Err(e) => {
                if let Some(ref awp) = state.awp_state {
                    crate::awp::report_degrading(awp, &format!("LLM error: {e}")).await;
                }
                anyhow::bail!("agent run failed: {e}");
            }
        }
    } else {
        // Fallback path — try each model in the chain until one succeeds
        let fallback_chain = state.fallback_chain.clone();
        let session_service = state.session_service.clone();
        let app_name = state.session_bridge.app_name().to_string();
        let instruction = state.agent_instruction.clone();

        let result = fallback_chain
            .run_with_fallback(|model| {
                let app_name = app_name.clone();
                let session_service = session_service.clone();
                let instruction = instruction.clone();
                let uid = uid.clone();
                let sid = sid.clone();
                let content = content.clone();
                async move {
                    // Rebuild agent with the given model for this attempt
                    let fallback_agent: Arc<dyn Agent> = Arc::new(
                        LlmAgentBuilder::new("assistant")
                            .model(model)
                            .instruction(instruction.as_str())
                            .build()
                            .map_err(|e| format!("failed to build fallback agent: {e}"))?,
                    );
                    let runner = Runner::new(RunnerConfig {
                        app_name,
                        agent: fallback_agent,
                        session_service,
                        artifact_service: None,
                        memory_service: None,
                        plugin_manager: None,
                        run_config: Some({
                            let rc = adk_core::RunConfig::default();
                            rc
                        }),
                        compaction_config: None,
                        context_cache_config: None,
                        cache_capable: None,
                        request_context: None,
                        cancellation_token: None,
                        intra_compaction_config: None,
                        intra_compaction_summarizer: None,
                    })
                    .map_err(|e| format!("failed to create runner: {e}"))?;
                    runner
                        .run(uid, sid, content)
                        .await
                        .map_err(|e| e.to_string())
                }
            })
            .await;

        match result {
            Ok((stream, outcome)) => {
                match &outcome {
                    FallbackOutcome::PrimarySuccess => {
                        // Primary succeeded — no action needed
                    }
                    FallbackOutcome::FallbackUsed {
                        primary_id,
                        fallback_id,
                        fallback_index,
                        primary_error,
                    } => {
                        tracing::warn!(
                            primary = %primary_id,
                            fallback = %fallback_id,
                            fallback_index = fallback_index,
                            primary_error = %primary_error,
                            "LLM call used fallback model after primary failure"
                        );
                    }
                    FallbackOutcome::AllFailed { .. } => {
                        // This branch shouldn't be reached here since AllFailed is the Err case
                    }
                }
                if let Some(ref awp) = state.awp_state {
                    if outcome.is_degraded() {
                        crate::awp::report_degrading(awp, "using fallback model").await;
                    } else {
                        crate::awp::report_healthy(awp).await;
                    }
                }
                stream
            }
            Err(errors) => {
                // All models failed
                if let Some(ref awp) = state.awp_state {
                    crate::awp::report_degrading(awp, "all fallback models failed").await;
                }
                let error_summary: Vec<String> = errors
                    .iter()
                    .map(|(id, err)| format!("{}: {}", id, err))
                    .collect();
                tracing::error!(
                    errors = ?error_summary,
                    "all models in fallback chain failed"
                );
                send_reply(
                    &msg,
                    state,
                    "⚠️ All configured models are currently unavailable. Please try again later.",
                )
                .await;
                state.metrics.record_message(
                    &channel_name,
                    MessageStatus::Failure,
                    start.elapsed(),
                    None,
                    None,
                    None,
                );
                return Ok(());
            }
        }
    };

    // Build delivery strategy and message ref before stream processing (R17)
    // so we can call on_partial during streaming and on_complete at the end.
    let key = ChannelKey {
        channel_type: msg.channel_type,
        account_id: msg.account_id.clone(),
    };
    let delivery_strategy: Option<(Arc<dyn DeliveryStrategy>, MessageRef)> =
        state.channel_map.get(&key).map(|ch| {
            let mut recipient = if msg.is_group {
                msg.group_id.clone().unwrap_or(msg.sender_id.clone())
            } else {
                msg.sender_id.clone()
            };

            // For cron messages: if recipient is non-numeric (username like @bot),
            // resolve to the first paired user on this channel type.
            if matches!(msg.source, crate::channel::MessageSource::Cron { .. }) {
                let is_numeric = recipient.chars().all(|c| c.is_ascii_digit());
                if !is_numeric {
                    let paired = state.pairing_service.paired_users();
                    let channel_str = format!("{}", msg.channel_type);
                    if let Some(user) = paired.iter().find(|u| u.channel_type == channel_str) {
                        tracing::info!(
                            original_target = %recipient,
                            resolved_to = %user.user_id,
                            "cron delivery: resolved non-numeric target to paired user"
                        );
                        recipient = user.user_id.clone();
                    } else {
                        tracing::warn!(
                            target = %recipient,
                            channel = %msg.channel_type,
                            "cron delivery: no paired users found for channel, delivery may fail"
                        );
                    }
                }
            }

            let cfg = state.config.load();
            let stream_mode = resolve_stream_mode(&cfg, msg.channel_type);
            let strategy = delivery::select_strategy(ch.value().clone(), stream_mode.as_deref());
            let msg_ref = MessageRef {
                channel_type: msg.channel_type,
                account_id: msg.account_id.clone(),
                recipient_id: recipient,
                message_id: None,
                reply_to: Some(msg.platform_message_id.clone()),
            };
            (strategy, msg_ref)
        });

    // Collect the event stream, calling on_partial for each partial chunk (R17.1)
    let collected = if let Some((ref strategy, ref msg_ref)) = delivery_strategy {
        let strategy = strategy.clone();
        let msg_ref = msg_ref.clone();
        EventStreamCollector::new(stream)
            .collect_with_partial(move |partial_text| {
                let strategy = strategy.clone();
                let msg_ref = msg_ref.clone();
                async move {
                    if let Err(e) = strategy.on_partial(&partial_text, &msg_ref).await {
                        tracing::warn!(error = %e, "streaming on_partial failed");
                    }
                }
            })
            .await
    } else {
        EventStreamCollector::new(stream).collect().await
    };

    let response = if collected.text.is_empty() {
        "I received your message but couldn't generate a response.".into()
    } else {
        collected.text
    };

    let latency = start.elapsed();
    let tool_call_count = collected.tool_calls.len();
    let stream_duration = collected.duration;

    // Stop the typing indicator — response is ready
    typing_cancel.cancel();

    // Remove from active requests (processing complete)
    state.active_requests.remove(&msg.sender_id);
    // Clean up any remaining progress messages
    state.progress_messages.remove(&user_id);

    tracing::info!(
        channel = %channel_name,
        latency_ms = %latency.as_millis(),
        stream_duration_ms = %stream_duration.as_millis(),
        tool_call_count = tool_call_count,
        tools = ?collected.tool_calls.iter().map(|tc| tc.name.as_str()).collect::<Vec<_>>(),
        max_iterations_reached = collected.max_iterations_reached,
        iteration_count = ?collected.iteration_count,
        "message processed"
    );

    state.metrics.record_message(
        &channel_name,
        MessageStatus::Success,
        latency,
        collected
            .token_count
            .as_ref()
            .map(|t| t.prompt_tokens as u64),
        collected
            .token_count
            .as_ref()
            .map(|t| t.completion_tokens as u64),
        Some(state.config.load().agent.model.primary()),
    );
    state
        .metrics
        .set_active_sessions(state.session_bridge.active_sessions().len() as u64);

    // ── Plugin: AfterModel hook (R8) — after model response received ──
    {
        let mut after_model_ctx = crate::plugin_manager::HookContext::default();
        after_model_ctx.response_text = Some(response.clone());
        after_model_ctx.model_name = Some(state.config.load().agent.model.primary().to_string());
        state.plugin_manager.invoke_hook(
            crate::plugin_manager::PluginHook::AfterModel,
            &mut after_model_ctx,
        );
    }

    // ── Plugin: BeforeTool / AfterTool hooks (R8) — for each tool call observed ──
    for tool_call in &collected.tool_calls {
        // BeforeTool
        let mut before_tool_ctx = crate::plugin_manager::HookContext::default();
        before_tool_ctx.tool_name = Some(tool_call.name.clone());
        before_tool_ctx.metadata = Some(tool_call.args.clone());
        state.plugin_manager.invoke_hook(
            crate::plugin_manager::PluginHook::BeforeTool,
            &mut before_tool_ctx,
        );

        // AfterTool
        let mut after_tool_ctx = crate::plugin_manager::HookContext::default();
        after_tool_ctx.tool_name = Some(tool_call.name.clone());
        after_tool_ctx.metadata = Some(tool_call.args.clone());
        state.plugin_manager.invoke_hook(
            crate::plugin_manager::PluginHook::AfterTool,
            &mut after_tool_ctx,
        );
    }

    // ── Plugin: AfterAgent hook (R8) — after agent execution completes ──
    {
        let mut after_agent_ctx = crate::plugin_manager::HookContext::default();
        after_agent_ctx.response_text = Some(response.clone());
        after_agent_ctx.agent_name = Some(agent.name().to_string());
        state.plugin_manager.invoke_hook(
            crate::plugin_manager::PluginHook::AfterAgent,
            &mut after_agent_ctx,
        );
    }

    // Invoke plugin AfterRun hook (legacy, kept for backward compat)
    let mut after_ctx = crate::plugin_manager::HookContext::default();
    after_ctx.response_text = Some(response.clone());
    state
        .plugin_manager
        .invoke_hook(crate::plugin_manager::PluginHook::AfterRun, &mut after_ctx);

    // ── Memory: store conversation facts and rebuild entity summary ──
    // Skip memory storage for cron messages (they shouldn't pollute the KG)
    let is_cron = matches!(msg.source, crate::channel::MessageSource::Cron { .. });
    if !is_cron {
    if let Some(ref mem_cfg) = state.config.load().memory {
        let user_entity_name = msg
            .sender_name
            .clone()
            .unwrap_or_else(|| msg.sender_id.clone());

        // Ensure user entity exists
        state.knowledge_graph.create_entities(
            &user_id,
            vec![crate::knowledge_graph::CreateEntityInput {
                name: user_entity_name.clone(),
                entity_type: "user".to_string(),
                observations: vec![],
            }],
        );

        // Store the exchange as observations
        let observations = vec![
            format!("User said: {}", msg.text),
            format!(
                "Assistant replied: {}",
                if response.len() > 200 {
                    &response[..200]
                } else {
                    &response
                }
            ),
        ];
        state
            .knowledge_graph
            .add_observations(&user_id, &user_entity_name, observations);

        // Trim old observations to prevent unbounded growth
        let trimmed = state
            .knowledge_graph
            .trim_observations(&user_id, mem_cfg.max_observations);
        if trimmed > 0 {
            tracing::debug!(user = %user_id, trimmed, "memory: trimmed old observations");
        }

        // Rebuild and cache the entity summary for next turn
        let summary = state
            .knowledge_graph
            .build_entity_summary(&user_id, mem_cfg.summary_observations);
        state.memory_summaries.insert(user_id.clone(), summary);

        let node_count = state.knowledge_graph.search_nodes(&user_id, "").len();
        tracing::info!(
            user = %user_id,
            entity = %user_entity_name,
            total_entities = node_count,
            "memory: stored conversation turn, summary rebuilt"
        );
    }
    } // end !is_cron

    // Audit log
    let tool_names: Vec<&str> = collected
        .tool_calls
        .iter()
        .map(|tc| tc.name.as_str())
        .collect();
    let audit_details = if tool_names.is_empty() {
        format!(
            "latency_ms={}, stream_duration_ms={}, tool_calls=0",
            latency.as_millis(),
            stream_duration.as_millis(),
        )
    } else {
        format!(
            "latency_ms={}, stream_duration_ms={}, tool_calls={}, tool_names=[{}]",
            latency.as_millis(),
            stream_duration.as_millis(),
            tool_names.len(),
            tool_names.join(", "),
        )
    };
    let audit_event = AuditEvent {
        timestamp: chrono::Utc::now(),
        user_id: user_id.clone(),
        session_id: Some(session_id.clone()),
        channel_type: Some(msg.channel_type),
        event_type: AuditEventType::AgentAccess,
        resource: agent_id.to_string(),
        outcome: AuditOutcome::Allowed,
        details: Some(audit_details),
    };
    let _ = state.audit_sink.log_event(audit_event).await;

    // Suppress delivery if response matches the task's suppress_keyword
    if let crate::channel::MessageSource::Cron { ref job_id } = msg.source {
        // Look up the suppress_keyword for this task
        let suppress_keyword = {
            let config = state.config.load();
            config.cron.jobs.iter()
                .find(|j| j.id == *job_id)
                .and_then(|j| j.suppress_keyword.clone())
        }.or_else(|| {
            // For runtime-only tasks (heartbeat), check the scheduler
            if job_id == "heartbeat" {
                Some("HEARTBEAT_OK".to_string())
            } else {
                None
            }
        });

        if let Some(keyword) = suppress_keyword {
            let trimmed = response.trim();
            if trimmed == keyword || trimmed.starts_with(&keyword) || trimmed.ends_with(&keyword) {
                tracing::info!(job_id = %job_id, keyword = %keyword, "task response suppressed (matched suppress_keyword)");
                state.task_log.log(job_id, crate::task_log::EVENT_DELIVERED, &format!("{} (suppressed)", keyword));
                return Ok(());
            }
        }
    }

    if let Some((strategy, msg_ref)) = delivery_strategy {
        let max_len = msg_ref.channel_type.max_message_length();
        let chunks = split_message(&response, max_len);
        for chunk in &chunks {
            strategy.on_complete(chunk, &msg_ref).await?;
        }
        // Log successful delivery for cron tasks
        if let crate::channel::MessageSource::Cron { ref job_id } = msg.source {
            let preview = if response.len() > 100 { &response[..100] } else { &response };
            state.task_log.log(job_id, crate::task_log::EVENT_DELIVERED, &format!("Delivered: {}", preview));
            state.task_log.log(job_id, crate::task_log::EVENT_RESPONSE, &response);
        }

        // Send any images that were in the response
        if !collected.images.is_empty() {
            let img_key = ChannelKey {
                channel_type: msg.channel_type,
                account_id: msg.account_id.clone(),
            };
            if let Some(ch) = state.channel_map.get(&img_key) {
                for img in &collected.images {
                    if let Err(e) = ch.send_photo(&msg_ref.recipient_id, &img.data, &img.mime_type, None).await {
                        tracing::warn!(error = %e, mime_type = %img.mime_type, "failed to send image");
                    }
                }
            }
        }
    } else {
        tracing::warn!(
            channel = %msg.channel_type,
            account_id = %msg.account_id,
            sender_id = %msg.sender_id,
            "no delivery channel found — response not delivered (channel not in channel_map)"
        );
        // Log delivery failure for cron tasks
        if let crate::channel::MessageSource::Cron { ref job_id } = msg.source {
            state.task_log.log(job_id, crate::task_log::EVENT_FAILED, "No delivery channel found");
        }
    }

    Ok(())
}

async fn deliver_response(
    msg: &InboundMessage,
    state: &GatewayState,
    text: &str,
) -> anyhow::Result<()> {
    let key = ChannelKey {
        channel_type: msg.channel_type,
        account_id: msg.account_id.clone(),
    };
    if let Some(ch) = state.channel_map.get(&key) {
        let recipient = if msg.is_group {
            msg.group_id.clone().unwrap_or(msg.sender_id.clone())
        } else {
            msg.sender_id.clone()
        };
        let cfg = state.config.load();
        let stream_mode = resolve_stream_mode(&cfg, msg.channel_type);
        let strategy: Arc<dyn DeliveryStrategy> =
            delivery::select_strategy(ch.value().clone(), stream_mode.as_deref());
        let msg_ref = MessageRef {
            channel_type: msg.channel_type,
            account_id: msg.account_id.clone(),
            recipient_id: recipient,
            message_id: None,
            reply_to: Some(msg.platform_message_id.clone()),
        };
        let max_len = msg.channel_type.max_message_length();
        let chunks = split_message(text, max_len);
        for chunk in &chunks {
            strategy.on_complete(chunk, &msg_ref).await?;
        }
    }
    Ok(())
}

async fn send_reply(msg: &InboundMessage, state: &GatewayState, text: &str) {
    if let Err(e) = deliver_response(msg, state, text).await {
        tracing::error!(error = %e, "failed to send reply");
    }
}

/// Load a single context file from the configured context directory.
/// Searches config dir first, then working directory.
fn load_context_file(
    config: &GatewayConfig,
    config_path: &std::path::Path,
    filename: &str,
) -> String {
    let context_dir = config
        .memory
        .as_ref()
        .map(|m| m.context_dir.clone())
        .unwrap_or_else(|| std::path::PathBuf::from("context"));

    let candidates = if context_dir.is_relative() {
        vec![
            config_path
                .parent()
                .unwrap_or(std::path::Path::new("."))
                .join(&context_dir)
                .join(filename),
            std::env::current_dir()
                .unwrap_or_default()
                .join(&context_dir)
                .join(filename),
        ]
    } else {
        vec![context_dir.join(filename)]
    };

    for path in &candidates {
        if let Ok(content) = std::fs::read_to_string(path) {
            return content;
        }
    }
    String::new()
}

fn resolve_stream_mode(
    config: &GatewayConfig,
    channel_type: crate::channel::ChannelType,
) -> Option<String> {
    match channel_type {
        crate::channel::ChannelType::Telegram => config
            .channels
            .telegram
            .as_ref()
            .and_then(|tg| tg.stream_mode.clone()),
        _ => None,
    }
}

pub fn split_message(text: &str, max_len: usize) -> Vec<String> {
    crate::delivery::split_message(text, max_len)
}
