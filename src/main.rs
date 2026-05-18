//! adk-gateway — Multi-channel AI gateway for adk-rust agents
//!
//! OpenClaw-compatible configuration. Connects Telegram, Slack, and more
//! to your adk-rust agents via a single long-running binary.
//!
//! This binary uses the `adk_gateway` library crate for all functionality.
//! CLI parsing and the entry point live here; everything else is in lib.rs.

use adk_gateway::channel;
use adk_gateway::config;
use adk_gateway::config_encryption;
use adk_gateway::gateway;
use adk_gateway::knowledge_graph;
use adk_gateway::mcp;
use adk_gateway::pairing;
use adk_gateway::rag;
use adk_gateway::telemetry;

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

    /// Encrypt sensitive config values in-place
    ConfigEncrypt {
        /// Path to the encryption key file (32 bytes raw or base64-encoded)
        #[arg(long)]
        key_file: PathBuf,
    },
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
    ///
    /// Supports two modes:
    ///   1. Flag-based: --name my-server --command uvx --args "pkg@latest"
    ///   2. JSON input:  --json '{"my-server": {"command": "uvx", "args": ["pkg@latest"], "env": {"KEY": "val"}}}'
    ///
    /// The JSON format matches the standard mcpServers config block.
    Add {
        /// Server name/ID (required unless --json is used)
        #[arg(long, required_unless_present = "json")]
        name: Option<String>,
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
        /// JSON config: '{"name": {"command": "...", "args": [...], "env": {...}}}'
        #[arg(long, conflicts_with_all = ["command", "url", "args", "env"])]
        json: Option<String>,
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

    let telemetry_setup = telemetry::TelemetrySetup::from_config(&cfg.telemetry);
    telemetry_setup.init(filter);

    tracing::info!(telemetry = %telemetry_setup.describe(), "telemetry initialized");

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
        }
        Some(Commands::ConfigEncrypt { key_file }) => {
            match config_encryption::encrypt_config_file(&config_path, &key_file) {
                Ok(count) => {
                    println!(
                        "Encrypted {count} sensitive field(s) in {}",
                        config_path.display()
                    );
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    std::process::exit(1);
                }
            }
        }
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
            json,
        } => {
            let mut updated_cfg = cfg.clone();

            if let Some(json_str) = json {
                let parsed: serde_json::Value = serde_json::from_str(&json_str)
                    .map_err(|e| anyhow::anyhow!("invalid JSON: {e}"))?;

                let obj = parsed
                    .as_object()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "JSON must be an object mapping server names to configs"
                        )
                    })?;

                for (server_name, server_val) in obj {
                    let server_obj = server_val.as_object().ok_or_else(|| {
                        anyhow::anyhow!("config for '{server_name}' must be an object")
                    })?;

                    let transport = if let Some(cmd_val) = server_obj.get("command") {
                        let cmd_str = cmd_val
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("'command' must be a string"))?
                            .to_string();
                        let args_val: Vec<String> = server_obj
                            .get("args")
                            .and_then(|a| a.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let env_map: std::collections::HashMap<String, String> = server_obj
                            .get("env")
                            .and_then(|e| e.as_object())
                            .map(|obj| {
                                obj.iter()
                                    .filter_map(|(k, v)| {
                                        v.as_str().map(|s| (k.clone(), s.to_string()))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        mcp::McpTransport::Stdio {
                            command: cmd_str,
                            args: args_val,
                            env: env_map,
                        }
                    } else if let Some(url_val) = server_obj.get("url") {
                        let url_str = url_val
                            .as_str()
                            .ok_or_else(|| anyhow::anyhow!("'url' must be a string"))?
                            .to_string();
                        mcp::McpTransport::Sse { url: url_str }
                    } else {
                        anyhow::bail!("server '{server_name}' must have 'command' or 'url'");
                    };

                    let is_disabled = server_obj
                        .get("disabled")
                        .and_then(|d| d.as_bool())
                        .unwrap_or(false);

                    let new_server = mcp::McpServerConfig {
                        server_id: server_name.clone(),
                        transport,
                        auth: None,
                        enabled: !is_disabled,
                    };

                    updated_cfg
                        .mcp_servers
                        .retain(|s| s.server_id != *server_name);
                    updated_cfg.mcp_servers.push(new_server);
                    println!("Added MCP server '{server_name}'");
                }
            } else {
                let name = name.expect("name is required when --json is not used");
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

                updated_cfg.mcp_servers.retain(|s| s.server_id != name);
                updated_cfg.mcp_servers.push(new_server);
                println!("Added MCP server '{name}'");
            }

            let output = serde_json::to_string_pretty(&updated_cfg)?;
            std::fs::write(config_path, &output)?;
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
                    println!(
                        "{:<20} {:<10} {:<10}",
                        server.server_id, transport_type, enabled
                    );
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
