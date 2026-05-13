//! adk-gateway — Multi-channel AI gateway for adk-rust agents
//!
//! OpenClaw-compatible configuration. Connects Telegram, Slack, and more
//! to your adk-rust agents via a single long-running binary.

mod access_control;
mod action_executor;
mod agent_codegen;
mod agent_config;
mod agent_registry;
mod audit;
mod awp;
mod browser_factory;
mod channel;
mod config;
mod config_watcher;
mod context_coordinator;
mod control_panel;
mod cron;
mod delivery;
mod event_stream;
mod executable_tools;
mod fallback_chain;
mod gateway;
mod gateway_routes;
mod gateway_state;
mod graph_workflow;
mod jwt;
mod knowledge_graph;
mod mcp;
mod metrics;
mod model_factory;
mod pairing;
mod plugin_manager;
mod process_manager;
mod proxy_pool;
mod rag;
mod rbac_bridge;
mod reconnect;
mod router;
mod session_bridge;
mod shutdown;
mod skill_loader;
mod sqlrite_store;
mod telemetry;
mod tool_registry;
mod webhook;

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "adk-gateway",
    version,
    about = "Multi-channel AI gateway for adk-rust agents"
)]
struct Cli {
    /// Path to config file (default: ~/.openclaw/openclaw.json)
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the gateway (default)
    Gateway {
        /// Port to listen on
        #[arg(short, long)]
        port: Option<u16>,

        /// Force start (kill existing listener)
        #[arg(long)]
        force: bool,
    },

    /// Validate configuration
    ConfigValidate,

    /// Show current configuration
    ConfigShow,

    /// Check channel health
    ChannelsStatus {
        /// Run connection probes
        #[arg(long)]
        probe: bool,
    },

    /// Knowledge graph memory management
    #[command(subcommand)]
    Memory(MemoryCommands),

    /// RAG pipeline management
    #[command(subcommand)]
    Rag(RagCommands),

    /// DM pairing management
    #[command(subcommand)]
    Pairing(PairingCommands),

    /// MCP server management
    #[command(subcommand)]
    Mcp(McpCommands),
}

#[derive(Subcommand)]
enum MemoryCommands {
    /// Semantic search against the knowledge graph for a user
    Search {
        /// Search query
        query: String,

        /// User ID to scope the search
        #[arg(long)]
        user_id: String,
    },

    /// Delete the entire knowledge graph for a user
    DeleteUser {
        /// User ID whose graph should be deleted
        user_id: String,
    },
}

#[derive(Subcommand)]
enum RagCommands {
    /// Ingest files or directories into the RAG pipeline
    Ingest {
        /// Path to file or directory to ingest
        path: PathBuf,
    },

    /// Search the RAG knowledge base
    Search {
        /// Search query
        query: String,

        /// Maximum number of results
        #[arg(long, default_value = "5")]
        top_k: usize,
    },
}

#[derive(Subcommand)]
enum PairingCommands {
    /// Generate a new pairing code
    GenerateCode,
}

#[derive(Subcommand)]
enum McpCommands {
    /// Add an MCP server
    Add {
        /// Server name/ID
        #[arg(long)]
        name: String,
        /// Command to run (for stdio transport)
        #[arg(long)]
        command: Option<String>,
        /// Arguments for the command
        #[arg(long)]
        args: Vec<String>,
        /// Environment variables (KEY=VALUE format)
        #[arg(long)]
        env: Vec<String>,
        /// URL for HTTP/SSE transport
        #[arg(long)]
        url: Option<String>,
        /// Disable the server (default: enabled)
        #[arg(long)]
        disabled: bool,
    },
    /// Remove an MCP server
    Remove {
        /// Server name/ID to remove
        name: String,
    },
    /// List configured MCP servers
    List,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Load configuration first so telemetry can be configured from it
    let config_path = cli.config.unwrap_or_else(config::default_config_path);
    let cfg = config::load_config(&config_path)?;

    // Initialize tracing via TelemetrySetup from config
    let filter = if cli.verbose {
        EnvFilter::new("adk_gateway=debug,tower_http=debug")
    } else {
        EnvFilter::new("adk_gateway=info,tower_http=info")
    };

    let telemetry = telemetry::TelemetrySetup::from_config(&cfg.telemetry);
    telemetry.init(filter);

    tracing::info!(telemetry = %telemetry.describe(), "telemetry initialized");

    match cli.command {
        None | Some(Commands::Gateway { .. }) => {
            let port = match &cli.command {
                Some(Commands::Gateway { port, .. }) => port.unwrap_or(cfg.gateway.port),
                _ => cfg.gateway.port,
            };
            let force = matches!(&cli.command, Some(Commands::Gateway { force: true, .. }));

            // If --force, kill any existing process on the port
            if force {
                kill_port_holder(port);
            }

            tracing::info!(port, "starting adk-gateway");
            gateway::run(cfg, port, config_path).await?;
        }
        Some(Commands::ConfigValidate) => {
            tracing::info!("configuration is valid");
            println!("{}", serde_json::to_string_pretty(&cfg)?);
        }
        Some(Commands::ConfigShow) => {
            // Redact sensitive fields before displaying
            let mut display_cfg = serde_json::to_value(&cfg)?;
            if let Some(channels) = display_cfg.get_mut("channels") {
                if let Some(tg) = channels.get_mut("telegram") {
                    if tg.get("bot_token").is_some() {
                        tg["bot_token"] = serde_json::Value::String("***REDACTED***".into());
                    }
                }
                if let Some(slack) = channels.get_mut("slack") {
                    for key in &["bot_token", "app_token", "signing_secret"] {
                        if slack.get(*key).is_some() {
                            slack[*key] = serde_json::Value::String("***REDACTED***".into());
                        }
                    }
                }
            }
            if let Some(hooks) = display_cfg.get_mut("hooks") {
                if hooks.get("token").is_some() {
                    hooks["token"] = serde_json::Value::String("***REDACTED***".into());
                }
            }
            if let Some(auth) = display_cfg.get_mut("auth") {
                if let Some(sso) = auth.get_mut("sso") {
                    if sso.get("client_secret").is_some() {
                        sso["client_secret"] = serde_json::Value::String("***REDACTED***".into());
                    }
                }
            }
            println!("{}", serde_json::to_string_pretty(&display_cfg)?);
        }
        Some(Commands::ChannelsStatus { probe }) => {
            channel::print_status(&cfg.channels, probe).await;
        }
        Some(Commands::Memory(mem_cmd)) => {
            let kg = knowledge_graph::KnowledgeGraph::new();
            match mem_cmd {
                MemoryCommands::Search { query, user_id } => {
                    let results = kg.search_nodes(&user_id, &query);
                    println!("{}", serde_json::to_string_pretty(&results)?);
                }
                MemoryCommands::DeleteUser { user_id } => {
                    let deleted = kg.delete_user_graph(&user_id);
                    if deleted {
                        println!("Deleted knowledge graph for user '{user_id}'");
                    } else {
                        println!("No knowledge graph found for user '{user_id}'");
                    }
                }
            }
        }
        Some(Commands::Rag(rag_cmd)) => match rag_cmd {
            RagCommands::Ingest { path } => {
                let rag_config = cfg
                    .rag
                    .ok_or_else(|| anyhow::anyhow!("no RAG configuration found in config file"))?;
                let pipeline = rag::RagPipelineBuilder::build(&rag_config)?;
                let count = pipeline.ingest(&path)?;
                println!("Ingested {count} chunks from {}", path.display());
            }
            RagCommands::Search { query, top_k } => {
                let rag_config = cfg
                    .rag
                    .ok_or_else(|| anyhow::anyhow!("no RAG configuration found in config file"))?;
                let pipeline = rag::RagPipelineBuilder::build(&rag_config)?;
                let results = pipeline.search(&query, top_k);
                println!("{}", serde_json::to_string_pretty(&results)?);
            }
        },
        Some(Commands::Pairing(pairing_cmd)) => match pairing_cmd {
            PairingCommands::GenerateCode => {
                let service = pairing::DmPairingService::new();
                let code = service.generate_code();
                println!("{code}");
            }
        },
        Some(Commands::Mcp(mcp_cmd)) => {
            handle_mcp_command(mcp_cmd, &config_path, &cfg)?;
        },
    }

    Ok(())
}

/// Handle MCP subcommands: add, remove, list.
fn handle_mcp_command(
    cmd: McpCommands,
    config_path: &std::path::Path,
    cfg: &config::GatewayConfig,
) -> anyhow::Result<()> {
    match cmd {
        McpCommands::Add {
            name,
            command,
            args,
            env,
            url,
            disabled,
        } => {
            // Determine transport
            let transport = if let Some(cmd_str) = command {
                let env_map: std::collections::HashMap<String, String> = env
                    .iter()
                    .filter_map(|e| {
                        let parts: Vec<&str> = e.splitn(2, '=').collect();
                        if parts.len() == 2 {
                            Some((parts[0].to_string(), parts[1].to_string()))
                        } else {
                            None
                        }
                    })
                    .collect();
                mcp::McpTransport::Stdio {
                    command: cmd_str,
                    args,
                    env: env_map,
                }
            } else if let Some(url_str) = url {
                mcp::McpTransport::Sse { url: url_str }
            } else {
                anyhow::bail!("either --command or --url must be provided");
            };

            let new_server = mcp::McpServerConfig {
                server_id: name.clone(),
                transport,
                auth: None,
                enabled: !disabled,
            };

            // Update config
            let mut updated_cfg = cfg.clone();
            // Remove existing entry with same name if present
            updated_cfg.mcp_servers.retain(|s| s.server_id != name);
            updated_cfg.mcp_servers.push(new_server);

            // Write back
            let output = serde_json::to_string_pretty(&updated_cfg)?;
            std::fs::write(config_path, &output)?;

            println!("Added MCP server '{name}'");
        }
        McpCommands::Remove { name } => {
            let mut updated_cfg = cfg.clone();
            let before = updated_cfg.mcp_servers.len();
            updated_cfg.mcp_servers.retain(|s| s.server_id != name);

            if updated_cfg.mcp_servers.len() == before {
                println!("MCP server '{name}' not found");
            } else {
                let output = serde_json::to_string_pretty(&updated_cfg)?;
                std::fs::write(config_path, &output)?;
                println!("Removed MCP server '{name}'");
            }
        }
        McpCommands::List => {
            if cfg.mcp_servers.is_empty() {
                println!("No MCP servers configured");
            } else {
                println!("{:<20} {:<10} {:<10}", "SERVER ID", "TRANSPORT", "ENABLED");
                println!("{}", "-".repeat(42));
                for server in &cfg.mcp_servers {
                    let transport_type = match &server.transport {
                        mcp::McpTransport::Stdio { .. } => "stdio",
                        mcp::McpTransport::Sse { .. } => "sse",
                    };
                    let enabled = if server.enabled { "yes" } else { "no" };
                    println!("{:<20} {:<10} {:<10}", server.server_id, transport_type, enabled);
                }
            }
        }
    }
    Ok(())
}

/// Kill any process currently listening on the given port (macOS/Linux).
fn kill_port_holder(port: u16) {
    let output = std::process::Command::new("lsof")
        .args(["-ti", &format!(":{}", port)])
        .output();

    if let Ok(out) = output {
        let pids = String::from_utf8_lossy(&out.stdout);
        for pid_str in pids.split_whitespace() {
            if pid_str.parse::<u32>().is_ok() {
                tracing::info!(pid = pid_str, port, "killing existing process on port");
                let _ = std::process::Command::new("kill")
                    .args(["-9", pid_str])
                    .output();
            }
        }
        if !pids.trim().is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    }
}
