//! Tool registry and built-in tool factory for the gateway.
//!
//! Centralizes tool resolution for built-in tools. The [`BuiltInToolFactory`]
//! maps configuration tool names (e.g. `"web_search"`, `"code_execution"`) to
//! constructor functions. The [`ToolRegistry`] uses the factory to resolve
//! tool names from agent configuration into [`ToolEntry`] instances, logging
//! warnings for unknown names and skipping them without failing startup.
//!
//! Since the actual adk-tool implementations may not be available at this
//! stage, constructors produce placeholder [`ToolEntry`] instances that
//! record the tool name, description, and optional config.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ─── ToolExecutionConfig ────────────────────────────────────────────

/// Configuration governing tool execution behaviour within the agent pipeline.
///
/// Tool calls are executed internally by the adk-runner/adk-agent framework
/// when tools are registered with the agent. This config provides safety
/// limits to prevent runaway tool loops and enforce timeouts (R3.2, R3.3, R3.4).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)] // Deserialized from agent config; enforced during tool execution
pub struct ToolExecutionConfig {
    /// Maximum number of tool calls allowed per agent turn.
    /// Prevents runaway loops where the agent keeps calling tools indefinitely.
    /// Default: 20.
    pub max_tool_calls_per_turn: u32,
    /// Timeout for a single tool execution.
    /// If a tool does not complete within this duration, the call is cancelled
    /// and an error result is returned to the agent (R3.4).
    /// Default: 30 seconds.
    #[serde(with = "humantime_serde_compat")]
    pub tool_timeout: Duration,
}

impl Default for ToolExecutionConfig {
    fn default() -> Self {
        Self {
            max_tool_calls_per_turn: 20,
            tool_timeout: Duration::from_secs(30),
        }
    }
}

/// Serde helper for Duration as seconds (u64).
#[allow(dead_code)] // Used by ToolExecutionConfig serde derive
mod humantime_serde_compat {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(d.as_secs())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}

// ─── ToolEntry ─────────────────────────────────────────────────────

/// A resolved tool entry ready for registration with an agent.
#[derive(Debug, Clone)]
pub struct ToolEntry {
    /// Canonical tool name (e.g. `"web_search"`).
    pub name: String,
    /// Human-readable description of the tool.
    #[allow(dead_code)] // Used in tests and tool resolution; part of tool metadata
    pub description: String,
    /// Tool-specific configuration passed from the agent's tool config section.
    #[allow(dead_code)] // Used in tests and tool resolution; passed to tool constructors
    pub config: Option<Value>,
}

impl ToolEntry {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        config: Option<Value>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            config,
        }
    }
}

// ─── ToolConstructor ───────────────────────────────────────────────

/// A constructor function that creates a [`ToolEntry`] from optional config.
type ToolConstructor = Box<dyn Fn(Option<&Value>) -> anyhow::Result<ToolEntry> + Send + Sync>;

// ─── BuiltInToolFactory ────────────────────────────────────────────

/// Maps configuration tool names to their constructors.
///
/// At startup the factory is populated with all known built-in tool names.
/// When a tool name is requested, the factory invokes the corresponding
/// constructor, passing any tool-specific configuration from the agent's
/// config section.
pub struct BuiltInToolFactory {
    registry: HashMap<&'static str, ToolConstructor>,
}

impl BuiltInToolFactory {
    /// Create a new factory pre-populated with all known built-in tools.
    pub fn new() -> Self {
        let mut factory = Self {
            registry: HashMap::new(),
        };

        // ── Search tools ───────────────────────────────────────────
        factory.register("web_search", "Web search via WebSearchTool");
        factory.register("google_search", "Google search via GoogleSearchTool");
        factory.register("google_maps", "Google Maps via GoogleMapsTool");

        // ── Content tools ──────────────────────────────────────────
        factory.register(
            "url_context",
            "Fetch and extract URL content via UrlContextTool",
        );
        factory.register("load_artifacts", "Load artifacts via LoadArtifactsTool");

        // ── Code tools ─────────────────────────────────────────────
        factory.register("code_execution", "Code execution via CodeTool");
        factory.register("python_code", "Python code execution via PythonCodeTool");
        factory.register(
            "javascript_code",
            "JavaScript code execution via JavaScriptCodeTool",
        );
        factory.register(
            "frontend_code",
            "Frontend code execution via FrontendCodeTool",
        );
        factory.register("rust_code", "Rust code execution via RustCodeTool");

        // ── Provider-specific tools ────────────────────────────────
        factory.register(
            "openai_web_search",
            "OpenAI web search via OpenAIWebSearchTool",
        );
        factory.register(
            "openai_file_search",
            "OpenAI file search via OpenAIFileSearchTool",
        );
        factory.register(
            "openai_image_generation",
            "OpenAI image generation via OpenAIImageGenerationTool",
        );
        factory.register(
            "openai_code_interpreter",
            "OpenAI code interpreter via OpenAICodeInterpreterTool",
        );
        factory.register(
            "gemini_code_execution",
            "Gemini code execution via GeminiCodeExecutionTool",
        );
        factory.register(
            "gemini_file_search",
            "Gemini file search via GeminiFileSearchTool",
        );

        factory
    }

    /// Register a built-in tool with a placeholder constructor.
    fn register(&mut self, name: &'static str, description: &'static str) {
        self.registry.insert(
            name,
            Box::new(move |cfg: Option<&Value>| {
                Ok(ToolEntry::new(name, description, cfg.cloned()))
            }),
        );
    }

    /// Try to create a tool entry by name. Returns `None` for unknown names.
    #[allow(dead_code)] // Used by resolve_tools; available for selective tool resolution
    pub fn create(&self, name: &str, config: Option<&Value>) -> Option<anyhow::Result<ToolEntry>> {
        self.registry.get(name).map(|ctor| ctor(config))
    }

    /// Returns the set of all registered tool names.
    pub fn known_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.registry.keys().copied().collect();
        names.sort_unstable();
        names
    }
}

impl Default for BuiltInToolFactory {
    fn default() -> Self {
        Self::new()
    }
}

// ─── AgentToolEntry ─────────────────────────────────────────────────

/// Represents a tool that wraps another agent for inter-agent delegation (R3.8).
///
/// When an agent's configuration specifies an `AgentTool` entry referencing
/// another agent by ID, the gateway creates an `AgentToolEntry` and registers
/// it with the calling agent. The actual agent invocation is wired in later
/// phases; this struct captures the metadata needed for resolution.
#[derive(Debug, Clone)]
pub struct AgentToolEntry {
    /// The ID of the agent being wrapped as a tool.
    #[allow(dead_code)] // Used in tests; identifies the target agent for delegation
    pub agent_id: String,
    /// Human-readable description of what this agent-tool does.
    pub description: String,
}

impl AgentToolEntry {
    pub fn new(agent_id: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
            description: description.into(),
        }
    }
}

// ─── ToolRegistry ──────────────────────────────────────────────────

/// Central registry that resolves tool names to [`ToolEntry`] instances.
///
/// Uses [`BuiltInToolFactory`] for built-in tools, agent tools for
/// inter-agent delegation (R3.8), and custom tools. Unknown names are
/// logged as warnings and skipped without failing (R17.4).
pub struct ToolRegistry {
    builtin_factory: BuiltInToolFactory,
    /// Additional custom tool entries registered at runtime.
    custom_tools: HashMap<String, ToolEntry>,
    /// Agent tools that wrap other agents as callable tools (R3.8).
    agent_tools: HashMap<String, AgentToolEntry>,
}

impl ToolRegistry {
    /// Create a new registry with the default built-in factory.
    pub fn new() -> Self {
        Self {
            builtin_factory: BuiltInToolFactory::new(),
            custom_tools: HashMap::new(),
            agent_tools: HashMap::new(),
        }
    }

    /// Register a custom (non-built-in) tool entry.
    pub fn register_custom(&mut self, entry: ToolEntry) {
        self.custom_tools.insert(entry.name.clone(), entry);
    }

    /// Register an agent tool that wraps another agent as a callable tool (R3.8).
    ///
    /// The tool is registered under the name `"agent:<agent_id>"` so it can be
    /// resolved alongside built-in and custom tools. The actual agent invocation
    /// will be wired in later phases.
    pub fn register_agent_tool(&mut self, agent_id: &str, description: &str) {
        let entry = AgentToolEntry::new(agent_id, description);
        self.agent_tools.insert(agent_id.to_string(), entry);
        tracing::info!(
            agent_id = %agent_id,
            "registered agent tool for inter-agent delegation"
        );
    }

    /// Returns a reference to the registered agent tools map.
    #[allow(dead_code)] // Used in tests; available for agent tool inspection
    pub fn agent_tools(&self) -> &HashMap<String, AgentToolEntry> {
        &self.agent_tools
    }

    /// Resolve a list of tool names to [`ToolEntry`] instances.
    ///
    /// For each name:
    /// 1. Check custom tools first.
    /// 2. Check agent tools (names matching a registered agent ID produce
    ///    a `ToolEntry` wrapping that agent).
    /// 3. Try the built-in factory, passing tool-specific config extracted
    ///    from `tool_config` (keyed by tool name).
    /// 4. If unknown, log a warning and skip (R17.4).
    #[allow(dead_code)] // Available for selective tool resolution by name
    pub fn resolve_tools(&self, names: &[String], tool_config: Option<&Value>) -> Vec<ToolEntry> {
        let mut resolved = Vec::new();

        for name in names {
            // 1. Custom tool?
            if let Some(entry) = self.custom_tools.get(name.as_str()) {
                resolved.push(entry.clone());
                continue;
            }

            // 2. Agent tool? (R3.8)
            if let Some(agent_entry) = self.agent_tools.get(name.as_str()) {
                resolved.push(ToolEntry::new(name.clone(), &agent_entry.description, None));
                continue;
            }

            // 3. Built-in tool?
            let cfg = tool_config.and_then(|v| v.get(name.as_str()));

            match self.builtin_factory.create(name, cfg) {
                Some(Ok(entry)) => {
                    resolved.push(entry);
                }
                Some(Err(e)) => {
                    tracing::warn!(
                        tool = %name,
                        error = %e,
                        "failed to construct built-in tool, skipping"
                    );
                }
                None => {
                    tracing::warn!(
                        tool = %name,
                        "unknown tool name, skipping registration"
                    );
                }
            }
        }

        resolved
    }

    /// Resolve all registered custom and agent tools without requiring explicit names.
    /// This returns every tool that was registered via `register_custom` or `register_agent_tool`.
    pub fn resolve_all(&self) -> Vec<ToolEntry> {
        let mut resolved: Vec<ToolEntry> = self.custom_tools.values().cloned().collect();
        for (name, agent_entry) in &self.agent_tools {
            resolved.push(ToolEntry::new(name.clone(), &agent_entry.description, None));
        }
        resolved
    }

    /// Returns all known tool names across built-in, custom, and agent tools.
    pub fn known_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .builtin_factory
            .known_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        names.extend(self.custom_tools.keys().cloned());
        names.extend(self.agent_tools.keys().map(|k| format!("agent:{k}")));
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Log the list of registered tool names for an agent at info level (R17.5).
    pub fn log_registered_tools(agent_name: &str, tools: &[ToolEntry]) {
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        tracing::info!(
            agent = %agent_name,
            tools = ?names,
            count = names.len(),
            "registered tools for agent"
        );
    }

    /// Build a tool error result suitable for feeding back to the agent (R3.4).
    ///
    /// When a tool execution fails, the error should be returned to the agent
    /// as a tool-level error result rather than a gateway-level error. This
    /// allows the agent to handle the failure gracefully (e.g. retry, use a
    /// different tool, or inform the user).
    ///
    /// The returned JSON value follows the structure expected by adk-runner
    /// for `FunctionResponse` error payloads.
    #[allow(dead_code)] // Used in tests; available for tool error result construction
    pub fn build_tool_error_result(tool_name: &str, error: &str) -> serde_json::Value {
        serde_json::json!({
            "name": tool_name,
            "error": true,
            "content": error,
        })
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_creates_all_known_tools() {
        let factory = BuiltInToolFactory::new();
        let known = factory.known_names();

        // Verify we have all 16 expected built-in tools
        assert_eq!(known.len(), 16);

        // Each known name should produce a valid ToolEntry
        for name in &known {
            let result = factory.create(name, None);
            assert!(result.is_some(), "factory should know about {name}");
            let entry = result.unwrap().expect("constructor should succeed");
            assert_eq!(entry.name, *name);
            assert!(!entry.description.is_empty());
            assert!(entry.config.is_none());
        }
    }

    #[test]
    fn factory_returns_none_for_unknown() {
        let factory = BuiltInToolFactory::new();
        assert!(factory.create("nonexistent_tool", None).is_none());
    }

    #[test]
    fn factory_passes_config_to_entry() {
        let factory = BuiltInToolFactory::new();
        let cfg = serde_json::json!({"api_key": "test123"});
        let entry = factory.create("web_search", Some(&cfg)).unwrap().unwrap();
        assert_eq!(entry.name, "web_search");
        assert_eq!(entry.config.unwrap(), cfg);
    }

    #[test]
    fn registry_resolves_known_tools() {
        let registry = ToolRegistry::new();
        let names = vec![
            "web_search".to_string(),
            "code_execution".to_string(),
            "gemini_code_execution".to_string(),
        ];
        let tools = registry.resolve_tools(&names, None);
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "web_search");
        assert_eq!(tools[1].name, "code_execution");
        assert_eq!(tools[2].name, "gemini_code_execution");
    }

    #[test]
    fn registry_skips_unknown_tools() {
        let registry = ToolRegistry::new();
        let names = vec![
            "web_search".to_string(),
            "totally_fake_tool".to_string(),
            "code_execution".to_string(),
        ];
        let tools = registry.resolve_tools(&names, None);
        // Unknown tool is skipped, only 2 resolved
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "web_search");
        assert_eq!(tools[1].name, "code_execution");
    }

    #[test]
    fn registry_passes_per_tool_config() {
        let registry = ToolRegistry::new();
        let tool_config = serde_json::json!({
            "web_search": {"api_key": "ws_key"},
            "code_execution": {"sandbox": true}
        });
        let names = vec!["web_search".to_string(), "code_execution".to_string()];
        let tools = registry.resolve_tools(&names, Some(&tool_config));
        assert_eq!(tools.len(), 2);
        assert_eq!(
            tools[0].config.as_ref().unwrap(),
            &serde_json::json!({"api_key": "ws_key"})
        );
        assert_eq!(
            tools[1].config.as_ref().unwrap(),
            &serde_json::json!({"sandbox": true})
        );
    }

    #[test]
    fn registry_custom_tools_take_precedence() {
        let mut registry = ToolRegistry::new();
        let custom = ToolEntry::new("web_search", "Custom web search override", None);
        registry.register_custom(custom);

        let names = vec!["web_search".to_string()];
        let tools = registry.resolve_tools(&names, None);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].description, "Custom web search override");
    }

    #[test]
    fn registry_resolves_empty_list() {
        let registry = ToolRegistry::new();
        let tools = registry.resolve_tools(&[], None);
        assert!(tools.is_empty());
    }

    #[test]
    fn registry_handles_all_unknown() {
        let registry = ToolRegistry::new();
        let names = vec!["fake1".to_string(), "fake2".to_string()];
        let tools = registry.resolve_tools(&names, None);
        assert!(tools.is_empty());
    }

    #[test]
    fn known_names_are_sorted() {
        let factory = BuiltInToolFactory::new();
        let names = factory.known_names();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
    }

    // ── Agent tool tests (R3.8) ────────────────────────────────────

    #[test]
    fn register_agent_tool_stores_entry() {
        let mut registry = ToolRegistry::new();
        registry.register_agent_tool(
            "research-agent",
            "Delegates research tasks to the research agent",
        );

        let agent_tools = registry.agent_tools();
        assert_eq!(agent_tools.len(), 1);

        let entry = agent_tools.get("research-agent").unwrap();
        assert_eq!(entry.agent_id, "research-agent");
        assert_eq!(
            entry.description,
            "Delegates research tasks to the research agent"
        );
    }

    #[test]
    fn register_multiple_agent_tools() {
        let mut registry = ToolRegistry::new();
        registry.register_agent_tool("research-agent", "Research delegation");
        registry.register_agent_tool("code-agent", "Code generation delegation");
        registry.register_agent_tool("review-agent", "Code review delegation");

        assert_eq!(registry.agent_tools().len(), 3);
        assert!(registry.agent_tools().contains_key("research-agent"));
        assert!(registry.agent_tools().contains_key("code-agent"));
        assert!(registry.agent_tools().contains_key("review-agent"));
    }

    #[test]
    fn resolve_tools_finds_agent_tools() {
        let mut registry = ToolRegistry::new();
        registry.register_agent_tool("research-agent", "Delegates research tasks");

        let names = vec!["research-agent".to_string()];
        let tools = registry.resolve_tools(&names, None);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "research-agent");
        assert_eq!(tools[0].description, "Delegates research tasks");
        assert!(tools[0].config.is_none());
    }

    #[test]
    fn resolve_tools_mixes_builtin_and_agent_tools() {
        let mut registry = ToolRegistry::new();
        registry.register_agent_tool("research-agent", "Research delegation");

        let names = vec![
            "web_search".to_string(),
            "research-agent".to_string(),
            "code_execution".to_string(),
        ];
        let tools = registry.resolve_tools(&names, None);

        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0].name, "web_search");
        assert_eq!(tools[1].name, "research-agent");
        assert_eq!(tools[2].name, "code_execution");
    }

    #[test]
    fn custom_tools_take_precedence_over_agent_tools() {
        let mut registry = ToolRegistry::new();
        registry.register_agent_tool("my-tool", "Agent version");
        let custom = ToolEntry::new("my-tool", "Custom version", None);
        registry.register_custom(custom);

        let names = vec!["my-tool".to_string()];
        let tools = registry.resolve_tools(&names, None);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].description, "Custom version");
    }

    #[test]
    fn agent_tools_take_precedence_over_builtin() {
        let mut registry = ToolRegistry::new();
        // Register an agent tool with the same name as a built-in
        registry.register_agent_tool("web_search", "Agent-wrapped web search");

        let names = vec!["web_search".to_string()];
        let tools = registry.resolve_tools(&names, None);

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].description, "Agent-wrapped web search");
    }

    #[test]
    fn register_agent_tool_overwrites_same_id() {
        let mut registry = ToolRegistry::new();
        registry.register_agent_tool("research-agent", "Old description");
        registry.register_agent_tool("research-agent", "New description");

        assert_eq!(registry.agent_tools().len(), 1);
        assert_eq!(
            registry
                .agent_tools()
                .get("research-agent")
                .unwrap()
                .description,
            "New description"
        );
    }

    // ── Tool error result tests (R3.4) ─────────────────────────────

    #[test]
    fn build_tool_error_result_contains_name() {
        let result = ToolRegistry::build_tool_error_result("web_search", "API key expired");
        assert_eq!(result["name"], "web_search");
    }

    #[test]
    fn build_tool_error_result_contains_error_flag() {
        let result = ToolRegistry::build_tool_error_result("web_search", "timeout");
        assert_eq!(result["error"], true);
    }

    #[test]
    fn build_tool_error_result_contains_error_message() {
        let msg = "Connection refused: could not reach search API";
        let result = ToolRegistry::build_tool_error_result("google_search", msg);
        assert_eq!(result["content"], msg);
    }

    #[test]
    fn build_tool_error_result_with_empty_error() {
        let result = ToolRegistry::build_tool_error_result("code_execution", "");
        assert_eq!(result["name"], "code_execution");
        assert_eq!(result["error"], true);
        assert_eq!(result["content"], "");
    }

    #[test]
    fn build_tool_error_result_with_special_characters() {
        let msg = r#"Error: unexpected token '<' at line 1, col 1 — "<!DOCTYPE html>""#;
        let result = ToolRegistry::build_tool_error_result("url_context", msg);
        assert_eq!(result["content"], msg);
    }

    // ── ToolExecutionConfig tests ──────────────────────────────────

    #[test]
    fn tool_execution_config_defaults() {
        let config = ToolExecutionConfig::default();
        assert_eq!(config.max_tool_calls_per_turn, 20);
        assert_eq!(config.tool_timeout, std::time::Duration::from_secs(30));
    }

    #[test]
    fn tool_execution_config_serde_roundtrip() {
        let config = ToolExecutionConfig {
            max_tool_calls_per_turn: 50,
            tool_timeout: std::time::Duration::from_secs(60),
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ToolExecutionConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.max_tool_calls_per_turn, 50);
        assert_eq!(parsed.tool_timeout, std::time::Duration::from_secs(60));
    }
}
