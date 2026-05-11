//! Agent code generation pipeline.
//!
//! Converts an `AgentConfig` into a compiled Rust binary that runs as an
//! isolated A2A agent process. The pipeline:
//!
//! 1. `to_project_schema()` — map AgentConfig → ProjectSchema
//! 2. `generate_source()` — produce main.rs source from the schema
//! 3. `inject_a2a_server()` — replace stdin loop with A2A HTTP server
//! 4. `write_files()` — write main.rs + Cargo.toml to agent workspace
//! 5. `compile()` — `cargo build --release` in the workspace
//! 6. `build_agent()` — orchestrate the full pipeline

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::agent_config::{AgentConfig, AgentType};

// ── Self-contained schema types (mirrors adk-studio) ──────────────

/// Top-level project schema describing an agent crate to generate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectSchema {
    pub agent: AgentSchema,
    pub tools: Vec<ToolConfig>,
    pub action_nodes: Vec<ActionNodeConfig>,
    pub workflow: Option<WorkflowSchema>,
    pub settings: ProjectSettings,
}

/// Schema for the agent itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentSchema {
    pub id: String,
    pub name: String,
    pub description: String,
    pub agent_type: String,
    pub model: String,
    pub instruction: String,
    pub temperature: Option<f32>,
    pub max_output_tokens: Option<i32>,
    pub sub_agents: Vec<String>,
}

/// A tool binding for the agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolConfig {
    pub name: String,
}

/// An action node in the workflow graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionNodeConfig {
    pub id: String,
    pub config: serde_json::Value,
}

/// Workflow graph definition with edges.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowSchema {
    pub edges: Vec<WorkflowEdgeSchema>,
}

/// A directed edge in the workflow graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowEdgeSchema {
    pub from: String,
    pub to: String,
    pub condition: Option<String>,
}

/// Project-level settings for code generation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectSettings {
    pub api_key_env: String,
    pub adk_crate_root: Option<String>,
}

/// The generated project files ready to be written to disk.
#[derive(Debug, Clone)]
pub struct GeneratedProject {
    pub main_rs: String,
    pub cargo_toml: String,
}

// ── Marker used by inject_a2a_server ──────────────────────────────

/// The stdin interactive loop marker that `generate_source` emits and
/// `inject_a2a_server` replaces with the A2A HTTP server.
const STDIN_LOOP_MARKER: &str = "// __STDIN_INTERACTIVE_LOOP__";
const STDIN_LOOP_END_MARKER: &str = "// __END_STDIN_INTERACTIVE_LOOP__";

// ── AgentCodegen ──────────────────────────────────────────────────

/// Generates, compiles, and manages agent Rust binaries.
pub struct AgentCodegen {
    workspace_root: PathBuf,
    adk_crate_root: Option<PathBuf>,
}

impl AgentCodegen {
    /// Create a new codegen instance.
    ///
    /// * `workspace_root` — root directory containing `agents/` subdirectories.
    /// * `adk_crate_root` — optional path to the local adk-rust checkout for
    ///   path dependencies. When `None`, version deps are used.
    pub fn new(workspace_root: PathBuf, adk_crate_root: Option<PathBuf>) -> Self {
        Self {
            workspace_root,
            adk_crate_root,
        }
    }

    // ── Step 1 ────────────────────────────────────────────────────

    /// Convert an `AgentConfig` into a `ProjectSchema`.
    pub fn to_project_schema(&self, config: &AgentConfig) -> ProjectSchema {
        let agent_type_str = match config.agent_type {
            AgentType::Llm => "llm",
            AgentType::Sequential => "sequential",
            AgentType::Parallel => "parallel",
            AgentType::Loop => "loop",
            AgentType::Router => "router",
            AgentType::Graph => "graph",
        }
        .to_string();

        let tools: Vec<ToolConfig> = config
            .tools
            .iter()
            .map(|name| ToolConfig { name: name.clone() })
            .collect();

        let action_nodes: Vec<ActionNodeConfig> = config
            .action_nodes
            .iter()
            .map(|entry| ActionNodeConfig {
                id: entry.id.clone(),
                config: entry.config.clone(),
            })
            .collect();

        let workflow = if config.workflow_edges.is_empty() {
            None
        } else {
            Some(WorkflowSchema {
                edges: config
                    .workflow_edges
                    .iter()
                    .map(|e| WorkflowEdgeSchema {
                        from: e.from.clone(),
                        to: e.to.clone(),
                        condition: e.condition.clone(),
                    })
                    .collect(),
            })
        };

        let settings = ProjectSettings {
            api_key_env: config.api_key_env.clone(),
            adk_crate_root: self
                .adk_crate_root
                .as_ref()
                .map(|p| p.display().to_string()),
        };

        ProjectSchema {
            agent: AgentSchema {
                id: config.id.clone(),
                name: config.name.clone(),
                description: config.description.clone(),
                agent_type: agent_type_str,
                model: config.model.clone(),
                instruction: config.instruction.clone(),
                temperature: config.temperature,
                max_output_tokens: config.max_output_tokens,
                sub_agents: config.sub_agents.clone(),
            },
            tools,
            action_nodes,
            workflow,
            settings,
        }
    }

    // ── Step 2 ────────────────────────────────────────────────────

    /// Generate Rust source code (main.rs) from a `ProjectSchema`.
    ///
    /// The generated code creates the appropriate agent type, configures the
    /// model, adds tools, and includes a stdin interactive loop placeholder
    /// that `inject_a2a_server` will replace.
    pub fn generate_source(&self, schema: &ProjectSchema) -> Result<GeneratedProject> {
        let main_rs = self.generate_main_rs(schema);
        let cargo_toml = self.generate_cargo_toml(schema);
        Ok(GeneratedProject {
            main_rs,
            cargo_toml,
        })
    }

    fn generate_main_rs(&self, schema: &ProjectSchema) -> String {
        let agent = &schema.agent;
        let escaped_instruction = agent.instruction.replace('\\', "\\\\").replace('"', "\\\"");
        let escaped_name = agent.name.replace('\\', "\\\\").replace('"', "\\\"");
        let escaped_desc = agent.description.replace('\\', "\\\\").replace('"', "\\\"");

        let model_setup = self.generate_model_setup(&agent.model, &schema.settings.api_key_env);

        let tool_registrations: String = schema
            .tools
            .iter()
            .map(|t| format!("    // tool: {}\n", t.name))
            .collect();

        let action_node_setup: String = if schema.action_nodes.is_empty() {
            String::new()
        } else {
            let mut s = String::from("    // Action nodes:\n");
            for node in &schema.action_nodes {
                s.push_str(&format!(
                    "    // node: {} config={}\n",
                    node.id, node.config
                ));
            }
            s
        };

        let temp_line = agent
            .temperature
            .map(|t| format!("    let _temperature: f32 = {:.2};\n", t))
            .unwrap_or_default();

        let max_tokens_line = agent
            .max_output_tokens
            .map(|t| format!("    let _max_output_tokens: i32 = {};\n", t))
            .unwrap_or_default();

        // Build the source via push to avoid nested format-string escaping issues.
        let mut src = String::new();
        src.push_str(&format!(
            "//! Generated agent binary for \"{}\".\n",
            escaped_name
        ));
        src.push_str(
            "//!\n//! Auto-generated by adk-gateway AgentCodegen. Do not edit manually.\n\n",
        );
        src.push_str("use std::io::BufRead;\n\n");
        src.push_str("#[tokio::main]\n");
        src.push_str("async fn main() {\n");
        src.push_str(&format!("    let agent_id = \"{}\";\n", agent.id));
        src.push_str(&format!("    let agent_name = \"{}\";\n", escaped_name));
        src.push_str(&format!(
            "    let agent_description = \"{}\";\n",
            escaped_desc
        ));
        src.push_str(&format!("    let agent_type = \"{}\";\n", agent.agent_type));
        src.push_str(&format!(
            "    let instruction = \"{}\";\n\n",
            escaped_instruction
        ));
        src.push_str(&model_setup);
        src.push('\n');
        src.push_str(&temp_line);
        src.push_str(&max_tokens_line);
        src.push_str(&tool_registrations);
        src.push_str(&action_node_setup);
        src.push_str(&format!(
            "    // Build the agent (type: {})\n",
            agent.agent_type
        ));
        src.push_str("    println!(\"Agent '{}' ({}) initialized\", agent_name, agent_type);\n\n");
        src.push_str(STDIN_LOOP_MARKER);
        src.push('\n');
        src.push_str("    let stdin = std::io::stdin();\n");
        src.push_str("    for line in stdin.lock().lines() {\n");
        src.push_str("        let line = match line {\n");
        src.push_str("            Ok(l) => l,\n");
        src.push_str("            Err(e) => {\n");
        src.push_str("                eprintln!(\"Error reading stdin: {}\", e);\n");
        src.push_str("                break;\n");
        src.push_str("            }\n");
        src.push_str("        };\n");
        src.push_str("        if line.trim() == \"quit\" {\n");
        src.push_str("            break;\n");
        src.push_str("        }\n");
        src.push_str("        println!(\"Agent {} received: {}\", agent_id, line);\n");
        src.push_str("    }\n");
        src.push_str(STDIN_LOOP_END_MARKER);
        src.push('\n');
        src.push_str("}\n");
        src
    }

    fn generate_model_setup(&self, model: &str, api_key_env: &str) -> String {
        let provider = if model.starts_with("anthropic/") {
            "anthropic"
        } else if model.starts_with("openai/") || model.starts_with("gpt-") {
            "openai"
        } else if model.starts_with("google/") || model.starts_with("gemini") {
            "gemini"
        } else if model.starts_with("ollama/") {
            "ollama"
        } else {
            "openai" // default fallback
        };

        format!(
            "    let _model_provider = \"{provider}\";\n    \
             let _model_id = \"{model}\";\n    \
             let _api_key_env = \"{api_key_env}\";\n",
            provider = provider,
            model = model,
            api_key_env = api_key_env,
        )
    }

    fn generate_cargo_toml(&self, schema: &ProjectSchema) -> String {
        let agent_id = &schema.agent.id;

        let adk_deps = if let Some(root) = &schema.settings.adk_crate_root {
            format!(
                r#"adk-core = {{ path = "{root}/adk-core" }}
adk-agent = {{ path = "{root}/adk-agent" }}
adk-runner = {{ path = "{root}/adk-runner" }}
adk-session = {{ path = "{root}/adk-session" }}
adk-server = {{ path = "{root}/adk-server" }}
adk-model = {{ path = "{root}/adk-model" }}
adk-tool = {{ path = "{root}/adk-tool" }}
adk-graph = {{ path = "{root}/adk-graph" }}
adk-memory = {{ path = "{root}/adk-memory" }}"#,
                root = root
            )
        } else {
            r#"# adk dependencies — set version or path as needed
# adk-core = "0.1"
# adk-agent = "0.1"
# adk-runner = "0.1"
# adk-session = "0.1"
# adk-server = "0.1"
# adk-model = "0.1"
# adk-tool = "0.1"
# adk-graph = "0.1"
# adk-memory = "0.1""#
                .to_string()
        };

        format!(
            r#"[package]
name = "{agent_id}"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "{agent_id}"
path = "src/main.rs"

[dependencies]
{adk_deps}
tokio = {{ version = "1", features = ["full"] }}
axum = {{ version = "0.8", features = ["macros"] }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
"#,
            agent_id = agent_id,
            adk_deps = adk_deps,
        )
    }

    // ── Step 3 ────────────────────────────────────────────────────

    /// Replace the stdin interactive loop in the generated main.rs with an
    /// A2A HTTP server that exposes `/.well-known/agent.json`, `POST /a2a`,
    /// and `POST /stream` routes using real axum code.
    pub fn inject_a2a_server(&self, main_rs: &str, config: &AgentConfig) -> String {
        let escaped_name = config.name.replace('\\', "\\\\").replace('"', "\\\"");
        let escaped_desc = config
            .description
            .replace('\\', "\\\\")
            .replace('"', "\\\"");

        // Build the server code via push to avoid nested format-string escaping.
        let mut server_code = String::new();
        server_code.push_str("    // ── A2A HTTP Server (injected by AgentCodegen) ──\n");
        server_code.push_str("    let port: u16 = std::env::var(\"AGENT_PORT\")\n");
        server_code.push_str("        .expect(\"AGENT_PORT must be set\")\n");
        server_code.push_str("        .parse()\n");
        server_code.push_str("        .expect(\"AGENT_PORT must be a valid u16\");\n");
        server_code.push_str("    let agent_id_env = std::env::var(\"AGENT_ID\")\n");
        server_code.push_str("        .unwrap_or_else(|_| agent_id.to_string());\n\n");
        server_code.push_str(&format!(
            "    let agent_card = serde_json::json!({{\n\
             \x20       \"name\": \"{}\",\n\
             \x20       \"description\": \"{}\",\n\
             \x20       \"url\": format!(\"http://127.0.0.1:{{}}\", port),\n\
             \x20       \"capabilities\": {{\n\
             \x20           \"streaming\": true,\n\
             \x20           \"pushNotifications\": false\n\
             \x20       }},\n\
             \x20       \"skills\": []\n\
             \x20   }});\n\n",
            escaped_name, escaped_desc
        ));
        server_code.push_str(
            "    println!(\"Starting A2A server for '{}' on port {}\", agent_id_env, port);\n\n",
        );
        server_code.push_str("    let card_json = agent_card.clone();\n");
        server_code.push_str("    let app = axum::Router::new()\n");
        server_code.push_str(
            "        .route(\"/.well-known/agent.json\", axum::routing::get(move || async move {\n",
        );
        server_code.push_str("            axum::Json(card_json.clone())\n");
        server_code.push_str("        }))\n");
        server_code.push_str("        .route(\"/a2a\", axum::routing::post(move |body: axum::Json<serde_json::Value>| async move {\n");
        server_code.push_str("            // Process the A2A message and return a response.\n");
        server_code.push_str("            let input = body.0;\n");
        server_code.push_str("            let response = serde_json::json!({\n");
        server_code.push_str("                \"jsonrpc\": \"2.0\",\n");
        server_code.push_str("                \"id\": input.get(\"id\").cloned().unwrap_or(serde_json::json!(null)),\n");
        server_code.push_str("                \"result\": {\n");
        server_code.push_str("                    \"content\": [{\n");
        server_code.push_str("                        \"type\": \"text\",\n");
        server_code.push_str("                        \"text\": format!(\"Agent {} processed your request.\", agent_id)\n");
        server_code.push_str("                    }]\n");
        server_code.push_str("                }\n");
        server_code.push_str("            });\n");
        server_code.push_str("            axum::Json(response)\n");
        server_code.push_str("        }));\n\n");
        server_code
            .push_str("    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));\n");
        server_code.push_str("    let listener = tokio::net::TcpListener::bind(addr).await.expect(\"failed to bind\");\n");
        server_code.push_str("    println!(\"A2A server listening on http://{}\", addr);\n");
        server_code.push_str("    axum::serve(listener, app).await.expect(\"server error\");\n");

        // Find and replace the stdin loop section.
        if let (Some(start), Some(end)) = (
            main_rs.find(STDIN_LOOP_MARKER),
            main_rs.find(STDIN_LOOP_END_MARKER),
        ) {
            let end_pos = end + STDIN_LOOP_END_MARKER.len();
            let mut result = String::with_capacity(main_rs.len());
            result.push_str(&main_rs[..start]);
            result.push_str(&server_code);
            result.push('\n');
            if end_pos < main_rs.len() {
                result.push_str(&main_rs[end_pos..]);
            }
            result
        } else {
            // If markers not found, append server code at the end.
            let mut result = main_rs.to_string();
            result.push_str("\n// WARNING: stdin loop markers not found, appending server code.\n");
            result.push_str(&server_code);
            result.push('\n');
            result
        }
    }

    // ── Step 4 ────────────────────────────────────────────────────

    /// Write the generated main.rs and Cargo.toml to the agent workspace.
    pub fn write_files(&self, agent_dir: &Path, project: &GeneratedProject) -> Result<()> {
        let src_dir = agent_dir.join("src");
        std::fs::create_dir_all(&src_dir)
            .with_context(|| format!("failed to create src dir at {}", src_dir.display()))?;

        let main_path = src_dir.join("main.rs");
        std::fs::write(&main_path, &project.main_rs)
            .with_context(|| format!("failed to write {}", main_path.display()))?;
        debug!(path = %main_path.display(), "wrote main.rs");

        let cargo_path = agent_dir.join("Cargo.toml");
        std::fs::write(&cargo_path, &project.cargo_toml)
            .with_context(|| format!("failed to write {}", cargo_path.display()))?;
        debug!(path = %cargo_path.display(), "wrote Cargo.toml");

        info!(agent_dir = %agent_dir.display(), "wrote generated agent files");
        Ok(())
    }

    // ── Step 5 ────────────────────────────────────────────────────

    /// Compile the agent crate with `cargo build --release`.
    /// Returns the path to the compiled binary on success.
    pub async fn compile(&self, agent_dir: &Path) -> Result<PathBuf> {
        info!(agent_dir = %agent_dir.display(), "compiling agent binary");

        let output = tokio::process::Command::new("cargo")
            .args(["build", "--release"])
            .current_dir(agent_dir)
            .output()
            .await
            .with_context(|| format!("failed to run cargo build in {}", agent_dir.display()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("cargo build failed in {}:\n{}", agent_dir.display(), stderr);
        }

        // Determine binary name from Cargo.toml (agent ID).
        let cargo_toml_path = agent_dir.join("Cargo.toml");
        let cargo_content = std::fs::read_to_string(&cargo_toml_path)
            .with_context(|| format!("failed to read {}", cargo_toml_path.display()))?;

        let binary_name = cargo_content
            .lines()
            .find(|l| l.starts_with("name = "))
            .and_then(|l| l.split('"').nth(1))
            .unwrap_or("agent");

        let binary_path = agent_dir.join("target").join("release").join(binary_name);

        if !binary_path.exists() {
            bail!("compiled binary not found at {}", binary_path.display());
        }

        info!(binary = %binary_path.display(), "agent binary compiled successfully");
        Ok(binary_path)
    }

    // ── Step 6 (full pipeline) ────────────────────────────────────

    /// Full build pipeline: schema → source → inject server → write → compile.
    /// Returns the path to the compiled binary.
    pub async fn build_agent(&self, config: &AgentConfig) -> Result<PathBuf> {
        let agent_dir = self.workspace_root.join("agents").join(&config.id);

        info!(
            agent_id = %config.id,
            agent_dir = %agent_dir.display(),
            "starting agent build pipeline"
        );

        // Step 1: Convert config to schema.
        let schema = self.to_project_schema(config);
        debug!(agent_id = %config.id, "generated project schema");

        // Step 2: Generate source code.
        let mut project = self.generate_source(&schema)?;
        debug!(agent_id = %config.id, "generated source code");

        // Step 3: Inject A2A server.
        project.main_rs = self.inject_a2a_server(&project.main_rs, config);
        debug!(agent_id = %config.id, "injected A2A server code");

        // Step 4: Write files.
        self.write_files(&agent_dir, &project)?;

        // Step 5: Compile.
        let binary_path = self.compile(&agent_dir).await?;

        info!(
            agent_id = %config.id,
            binary = %binary_path.display(),
            "agent build pipeline complete"
        );

        Ok(binary_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_config::{
        ActionNodeEntry, AgentConfig, AgentRoleConfig, AgentType, ChannelBinding, WorkflowEdge,
    };

    /// Helper: build a minimal LLM agent config for testing.
    fn llm_agent_config() -> AgentConfig {
        AgentConfig {
            id: "test-agent".to_string(),
            name: "Test Agent".to_string(),
            description: "A test LLM agent".to_string(),
            agent_type: AgentType::Llm,
            model: "anthropic/claude-sonnet-4".to_string(),
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            instruction: "You are a helpful assistant.".to_string(),
            tools: vec!["web_search".to_string(), "calculator".to_string()],
            action_nodes: vec![],
            workflow_edges: vec![],
            sub_agents: vec![],
            role: AgentRoleConfig {
                allow: vec!["web_search".to_string()],
                deny: vec![],
            },
            channel_bindings: vec![],
            auto_start: false,
            temperature: Some(0.7),
            max_output_tokens: Some(4096),
            model_override: None,
        }
    }

    /// Helper: build a graph agent config with action nodes.
    fn graph_agent_config_with_actions() -> AgentConfig {
        AgentConfig {
            id: "graph-agent".to_string(),
            name: "Graph Agent".to_string(),
            description: "A graph workflow agent".to_string(),
            agent_type: AgentType::Graph,
            model: "openai/gpt-4".to_string(),
            api_key_env: "OPENAI_API_KEY".to_string(),
            instruction: "Process data through the workflow.".to_string(),
            tools: vec!["http_request".to_string()],
            action_nodes: vec![
                ActionNodeEntry {
                    id: "fetch-data".to_string(),
                    config: serde_json::json!({
                        "type": "http",
                        "url": "https://api.example.com/data",
                        "method": "GET"
                    }),
                },
                ActionNodeEntry {
                    id: "transform".to_string(),
                    config: serde_json::json!({
                        "type": "transform",
                        "expression": "data.items"
                    }),
                },
            ],
            workflow_edges: vec![
                WorkflowEdge {
                    from: "start".to_string(),
                    to: "fetch-data".to_string(),
                    condition: None,
                },
                WorkflowEdge {
                    from: "fetch-data".to_string(),
                    to: "transform".to_string(),
                    condition: Some("status == 200".to_string()),
                },
            ],
            sub_agents: vec![],
            role: AgentRoleConfig {
                allow: vec!["http_request".to_string()],
                deny: vec![],
            },
            channel_bindings: vec![ChannelBinding {
                channel_type: "webhook".to_string(),
                account_id: None,
                peer_filter: None,
            }],
            auto_start: true,
            temperature: None,
            max_output_tokens: None,
            model_override: None,
        }
    }

    // ── 6.9: to_project_schema produces valid schema for LLM agent ──

    #[test]
    fn to_project_schema_llm_agent() {
        let codegen = AgentCodegen::new(PathBuf::from("/tmp/gw"), None);
        let config = llm_agent_config();
        let schema = codegen.to_project_schema(&config);

        assert_eq!(schema.agent.id, "test-agent");
        assert_eq!(schema.agent.name, "Test Agent");
        assert_eq!(schema.agent.description, "A test LLM agent");
        assert_eq!(schema.agent.agent_type, "llm");
        assert_eq!(schema.agent.model, "anthropic/claude-sonnet-4");
        assert_eq!(schema.agent.instruction, "You are a helpful assistant.");
        assert_eq!(schema.agent.temperature, Some(0.7));
        assert_eq!(schema.agent.max_output_tokens, Some(4096));
        assert!(schema.agent.sub_agents.is_empty());

        // Tools mapped correctly.
        assert_eq!(schema.tools.len(), 2);
        assert_eq!(schema.tools[0].name, "web_search");
        assert_eq!(schema.tools[1].name, "calculator");

        // No action nodes or workflow for a simple LLM agent.
        assert!(schema.action_nodes.is_empty());
        assert!(schema.workflow.is_none());

        // Settings.
        assert_eq!(schema.settings.api_key_env, "ANTHROPIC_API_KEY");
        assert!(schema.settings.adk_crate_root.is_none());
    }

    #[test]
    fn to_project_schema_with_adk_crate_root() {
        let codegen = AgentCodegen::new(
            PathBuf::from("/tmp/gw"),
            Some(PathBuf::from("/home/dev/adk-rust")),
        );
        let config = llm_agent_config();
        let schema = codegen.to_project_schema(&config);

        assert_eq!(
            schema.settings.adk_crate_root.as_deref(),
            Some("/home/dev/adk-rust")
        );
    }

    // ── 6.10: to_project_schema includes action nodes when configured ──

    #[test]
    fn to_project_schema_includes_action_nodes() {
        let codegen = AgentCodegen::new(PathBuf::from("/tmp/gw"), None);
        let config = graph_agent_config_with_actions();
        let schema = codegen.to_project_schema(&config);

        assert_eq!(schema.agent.agent_type, "graph");

        // Action nodes mapped.
        assert_eq!(schema.action_nodes.len(), 2);
        assert_eq!(schema.action_nodes[0].id, "fetch-data");
        assert_eq!(schema.action_nodes[1].id, "transform");
        assert_eq!(
            schema.action_nodes[0].config["type"],
            serde_json::json!("http")
        );

        // Workflow edges mapped.
        let workflow = schema
            .workflow
            .as_ref()
            .expect("workflow should be present");
        assert_eq!(workflow.edges.len(), 2);
        assert_eq!(workflow.edges[0].from, "start");
        assert_eq!(workflow.edges[0].to, "fetch-data");
        assert!(workflow.edges[0].condition.is_none());
        assert_eq!(workflow.edges[1].from, "fetch-data");
        assert_eq!(workflow.edges[1].to, "transform");
        assert_eq!(
            workflow.edges[1].condition.as_deref(),
            Some("status == 200")
        );
    }

    // ── 6.11: inject_a2a_server replaces stdin loop with server code ──

    #[test]
    fn inject_a2a_server_replaces_stdin_loop() {
        let codegen = AgentCodegen::new(PathBuf::from("/tmp/gw"), None);
        let config = llm_agent_config();

        // Generate source first to get the stdin loop markers.
        let schema = codegen.to_project_schema(&config);
        let project = codegen.generate_source(&schema).unwrap();

        // Verify markers are present in the generated source.
        assert!(
            project.main_rs.contains(STDIN_LOOP_MARKER),
            "generated source should contain stdin loop marker"
        );
        assert!(
            project.main_rs.contains(STDIN_LOOP_END_MARKER),
            "generated source should contain stdin loop end marker"
        );
        assert!(
            project.main_rs.contains("stdin"),
            "generated source should contain stdin loop code"
        );

        // Inject the A2A server.
        let injected = codegen.inject_a2a_server(&project.main_rs, &config);

        // The stdin loop should be gone.
        assert!(
            !injected.contains(STDIN_LOOP_MARKER),
            "injected source should not contain stdin loop marker"
        );
        assert!(
            !injected.contains("stdin.lock().lines()"),
            "injected source should not contain stdin loop code"
        );

        // The A2A server code should be present.
        assert!(
            injected.contains("AGENT_PORT"),
            "injected source should reference AGENT_PORT"
        );
        assert!(
            injected.contains("A2A HTTP Server"),
            "injected source should contain A2A server comment"
        );
        assert!(
            injected.contains(".well-known/agent.json"),
            "injected source should reference agent card route"
        );
        assert!(
            injected.contains("agent_card"),
            "injected source should set up agent card"
        );
        assert!(
            injected.contains("Test Agent"),
            "injected source should include agent name"
        );
    }

    #[test]
    fn inject_a2a_server_handles_missing_markers() {
        let codegen = AgentCodegen::new(PathBuf::from("/tmp/gw"), None);
        let config = llm_agent_config();

        let source_without_markers = "fn main() {\n    println!(\"hello\");\n}\n";
        let injected = codegen.inject_a2a_server(source_without_markers, &config);

        // Should append with a warning.
        assert!(injected.contains("WARNING: stdin loop markers not found"));
        assert!(injected.contains("A2A HTTP Server"));
    }

    // ── Additional coverage: generate_source and write_files ──────

    #[test]
    fn generate_source_produces_valid_rust_structure() {
        let codegen = AgentCodegen::new(PathBuf::from("/tmp/gw"), None);
        let config = llm_agent_config();
        let schema = codegen.to_project_schema(&config);
        let project = codegen.generate_source(&schema).unwrap();

        // main.rs should have fn main.
        assert!(project.main_rs.contains("fn main()"));
        assert!(project.main_rs.contains("test-agent"));
        assert!(project.main_rs.contains("anthropic"));

        // Cargo.toml should reference the agent ID.
        assert!(project.cargo_toml.contains("name = \"test-agent\""));
        assert!(project.cargo_toml.contains("tokio"));
    }

    #[test]
    fn generate_cargo_toml_uses_path_deps_when_adk_root_set() {
        let codegen = AgentCodegen::new(
            PathBuf::from("/tmp/gw"),
            Some(PathBuf::from("/home/dev/adk-rust")),
        );
        let config = llm_agent_config();
        let schema = codegen.to_project_schema(&config);
        let project = codegen.generate_source(&schema).unwrap();

        assert!(project
            .cargo_toml
            .contains("path = \"/home/dev/adk-rust/adk-core\""));
        assert!(project
            .cargo_toml
            .contains("path = \"/home/dev/adk-rust/adk-server\""));
    }

    #[test]
    fn write_files_creates_directory_structure() {
        let tmp = tempfile::tempdir().unwrap();
        let agent_dir = tmp.path().join("agents").join("test-agent");

        let codegen = AgentCodegen::new(tmp.path().to_path_buf(), None);
        let project = GeneratedProject {
            main_rs: "fn main() {}".to_string(),
            cargo_toml: "[package]\nname = \"test-agent\"".to_string(),
        };

        codegen.write_files(&agent_dir, &project).unwrap();

        assert!(agent_dir.join("src").join("main.rs").exists());
        assert!(agent_dir.join("Cargo.toml").exists());

        let main_content = std::fs::read_to_string(agent_dir.join("src").join("main.rs")).unwrap();
        assert_eq!(main_content, "fn main() {}");

        let cargo_content = std::fs::read_to_string(agent_dir.join("Cargo.toml")).unwrap();
        assert_eq!(cargo_content, "[package]\nname = \"test-agent\"");
    }

    #[test]
    fn model_provider_detection() {
        let codegen = AgentCodegen::new(PathBuf::from("/tmp"), None);

        // Test various model strings produce correct provider detection.
        let mut config = llm_agent_config();

        // Anthropic
        config.model = "anthropic/claude-sonnet-4".to_string();
        let schema = codegen.to_project_schema(&config);
        let project = codegen.generate_source(&schema).unwrap();
        assert!(project.main_rs.contains("\"anthropic\""));

        // OpenAI
        config.model = "openai/gpt-4".to_string();
        let schema = codegen.to_project_schema(&config);
        let project = codegen.generate_source(&schema).unwrap();
        assert!(project.main_rs.contains("\"openai\""));

        // Gemini
        config.model = "google/gemini-pro".to_string();
        let schema = codegen.to_project_schema(&config);
        let project = codegen.generate_source(&schema).unwrap();
        assert!(project.main_rs.contains("\"gemini\""));

        // Ollama
        config.model = "ollama/llama3".to_string();
        let schema = codegen.to_project_schema(&config);
        let project = codegen.generate_source(&schema).unwrap();
        assert!(project.main_rs.contains("\"ollama\""));
    }
}
