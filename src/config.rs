//! Gateway JSON5 configuration.
//!
//! Reads `~/.adk-gateway/gateway.json` by default.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::browser_factory::AgentBrowserConfig;
use crate::coding_agent::config::{CodingAgentsConfig, validate_backend_definition};
use crate::mcp::McpServerConfig;

// ── Config Errors ──────────────────────────────────────────────────

/// Errors that can occur during configuration validation.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ConfigError {
    #[error("max_iterations must be between 1 and 1000, got: {0}")]
    InvalidMaxIterations(u32),
}

/// Top-level gateway configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct GatewayConfig {
    /// Single-agent shorthand
    pub agent: AgentConfig,

    /// Multi-agent setup
    pub agents: AgentsConfig,

    /// Gateway server settings
    pub gateway: ServerSettings,

    /// Messaging channel configs
    pub channels: ChannelsConfig,

    /// Multi-agent routing bindings
    pub routing: RoutingConfig,

    /// Session management
    pub session: SessionConfig,

    /// Webhook / hooks
    pub hooks: HooksConfig,

    /// Cron jobs
    pub cron: CronConfig,

    /// Knowledge graph memory configuration
    pub memory: Option<MemoryConfig>,

    /// RAG pipeline configuration
    pub rag: Option<RagConfig>,

    /// Top-level auth configuration (expanded)
    pub auth: Option<AuthConfig>,

    /// Plugin configurations
    #[serde(default)]
    pub plugins: Vec<PluginConfig>,

    /// Convention file discovery settings
    pub conventions: ConventionConfig,

    /// Telemetry and observability settings
    pub telemetry: TelemetryConfig,

    /// Graph workflow configuration
    #[serde(default, rename = "graphWorkflow")]
    pub graph_workflow: Option<GraphWorkflowConfig>,

    /// MCP server configurations
    #[serde(default, rename = "mcpServers")]
    pub mcp_servers: Vec<McpServerConfig>,

    /// AWP (Agentic Web Protocol) configuration
    #[serde(default)]
    pub awp: crate::awp::AwpConfig,

    /// Runner configuration (max_iterations, etc.)
    #[serde(default)]
    pub runner: GatewayRunnerConfig,

    /// Rate limiter configuration for tool invocation throttling.
    #[serde(default, rename = "rateLimiter")]
    pub rate_limiter: RateLimitConfig,

    /// Tool approval configuration for dangerous tool calls.
    #[serde(default, rename = "toolApproval")]
    pub tool_approval: ApprovalConfig,

    /// Stale context detection configuration.
    #[serde(default, rename = "staleContext")]
    pub stale_context: StaleContextConfig,

    /// Heartbeat V2 configuration (session-integrated heartbeat).
    #[serde(default, rename = "heartbeatV2")]
    pub heartbeat_v2: HeartbeatV2Config,

    /// Multi-user support configuration.
    #[serde(default, rename = "multiUser")]
    pub multi_user: MultiUserConfig,

    /// Health monitor configuration for periodic component health checks.
    #[serde(default, rename = "healthMonitor")]
    pub health_monitor: HealthMonitorConfig,

    /// Coding agent subsystem configuration.
    #[serde(default, rename = "codingAgents")]
    pub coding_agents: CodingAgentsConfig,
}


// ── Agent ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Model configuration — supports simple string, WithFallbacks, or full category object.
    pub model: CategoryConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: CategoryConfig {
                primary: ModelSpec::WithFallbacks {
                    primary: "openai/gpt-5.4-mini".into(),
                    fallbacks: vec![
                        "openai/gpt-5.4-nano".into(),
                    ],
                },
                vision: None,
                omni: None,
                image_generation: None,
                tts: None,
                stt: None,
                code: Some(vec!["openai/codex-mini-latest".into()]),
                embedding: None,
                search: None,
                music: None,
            },
        }
    }
}

/// Model can be a simple string or an object with primary + fallbacks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModelSpec {
    Simple(String),
    WithFallbacks {
        primary: String,
        #[serde(default)]
        fallbacks: Vec<String>,
    },
}

impl ModelSpec {
    pub fn primary(&self) -> &str {
        match self {
            ModelSpec::Simple(s) => s,
            ModelSpec::WithFallbacks { primary, .. } => primary,
        }
    }
}

/// Category-based model configuration with fallback chains.
///
/// Each category field is `Option<Vec<String>>` — an ordered list of model IDs.
/// The first element is the primary model; subsequent elements are fallbacks.
///
/// Deserializes from multiple formats for backward compatibility:
/// 1. Simple string: `"anthropic/claude-sonnet-4"` → primary only
/// 2. WithFallbacks: `{ "primary": "...", "fallbacks": [...] }` → primary with fallbacks
/// 3. Full category object with string values: `{ "primary": "model", "vision": "model" }`
/// 4. Full category object with array values: `{ "primary": ["m1", "m2"], "vision": ["m3"] }`
/// 5. Mixed: `{ "primary": "model", "vision": ["m1", "m2"] }`
#[derive(Debug, Clone, PartialEq)]
pub struct CategoryConfig {
    pub primary: ModelSpec,
    pub vision: Option<Vec<String>>,
    pub omni: Option<Vec<String>>,
    pub image_generation: Option<Vec<String>>,
    pub tts: Option<Vec<String>>,
    pub stt: Option<Vec<String>>,
    pub code: Option<Vec<String>>,
    pub embedding: Option<Vec<String>>,
    pub search: Option<Vec<String>>,
    pub music: Option<Vec<String>>,
}

impl CategoryConfig {
    /// Returns the primary model identifier (backward-compatible).
    pub fn primary(&self) -> &str {
        self.primary.primary()
    }

    /// Resolve the effective model for a given category (first element of the chain).
    /// Falls back to omni for vision/tts/stt when not explicitly set.
    /// Returns None for unset categories with no fallback.
    pub fn resolve(&self, category: &str) -> Option<&str> {
        match category {
            "primary" => Some(self.primary()),
            "vision" => first_or(&self.vision).or_else(|| first_or(&self.omni)),
            "tts" => first_or(&self.tts).or_else(|| first_or(&self.omni)),
            "stt" => first_or(&self.stt).or_else(|| first_or(&self.omni)),
            "omni" => first_or(&self.omni),
            "image_generation" => first_or(&self.image_generation),
            "code" => first_or(&self.code),
            "embedding" => first_or(&self.embedding),
            "search" => first_or(&self.search),
            "music" => first_or(&self.music),
            _ => None,
        }
    }

    /// Resolve the full fallback chain for a given category.
    /// Returns None if the category is unset.
    pub fn resolve_chain(&self, category: &str) -> Option<&[String]> {
        let chain = match category {
            "primary" => match &self.primary {
                ModelSpec::Simple(_) => return None, // use primary() for simple
                ModelSpec::WithFallbacks { .. } => return None, // use primary() + fallbacks
            },
            "vision" => self.vision.as_deref().or(self.omni.as_deref()),
            "tts" => self.tts.as_deref().or(self.omni.as_deref()),
            "stt" => self.stt.as_deref().or(self.omni.as_deref()),
            "omni" => self.omni.as_deref(),
            "image_generation" => self.image_generation.as_deref(),
            "code" => self.code.as_deref(),
            "embedding" => self.embedding.as_deref(),
            "search" => self.search.as_deref(),
            "music" => self.music.as_deref(),
            _ => None,
        };
        chain.filter(|c| !c.is_empty())
    }
}

/// Helper: get first element of an optional vec as &str.
fn first_or(opt: &Option<Vec<String>>) -> Option<&str> {
    opt.as_ref().and_then(|v| v.first().map(|s| s.as_str()))
}

impl Serialize for CategoryConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        // Count non-None fields
        let mut count = 1; // primary always present
        if self.vision.as_ref().is_some_and(|v| !v.is_empty()) {
            count += 1;
        }
        if self.omni.as_ref().is_some_and(|v| !v.is_empty()) {
            count += 1;
        }
        if self
            .image_generation
            .as_ref()
            .is_some_and(|v| !v.is_empty())
        {
            count += 1;
        }
        if self.tts.as_ref().is_some_and(|v| !v.is_empty()) {
            count += 1;
        }
        if self.stt.as_ref().is_some_and(|v| !v.is_empty()) {
            count += 1;
        }
        if self.code.as_ref().is_some_and(|v| !v.is_empty()) {
            count += 1;
        }
        if self.embedding.as_ref().is_some_and(|v| !v.is_empty()) {
            count += 1;
        }
        if self.search.as_ref().is_some_and(|v| !v.is_empty()) {
            count += 1;
        }
        if self.music.as_ref().is_some_and(|v| !v.is_empty()) {
            count += 1;
        }

        let mut map = serializer.serialize_map(Some(count))?;
        map.serialize_entry("primary", &self.primary)?;

        fn serialize_vec<S: serde::ser::SerializeMap>(
            map: &mut S,
            key: &str,
            val: &Option<Vec<String>>,
        ) -> Result<(), S::Error> {
            if let Some(v) = val {
                if !v.is_empty() {
                    map.serialize_entry(key, v)?;
                }
            }
            Ok(())
        }

        serialize_vec(&mut map, "vision", &self.vision)?;
        serialize_vec(&mut map, "omni", &self.omni)?;
        serialize_vec(&mut map, "image_generation", &self.image_generation)?;
        serialize_vec(&mut map, "tts", &self.tts)?;
        serialize_vec(&mut map, "stt", &self.stt)?;
        serialize_vec(&mut map, "code", &self.code)?;
        serialize_vec(&mut map, "embedding", &self.embedding)?;
        serialize_vec(&mut map, "search", &self.search)?;
        serialize_vec(&mut map, "music", &self.music)?;

        map.end()
    }
}

impl<'de> Deserialize<'de> for CategoryConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        match &value {
            // Format 1: Simple string → primary only
            serde_json::Value::String(s) => Ok(CategoryConfig {
                primary: ModelSpec::Simple(s.clone()),
                vision: None,
                omni: None,
                image_generation: None,
                tts: None,
                stt: None,
                code: None,
                embedding: None,
                search: None,
                music: None,
            }),
            // Format 2, 3, 4, or 5: Object
            serde_json::Value::Object(map) => {
                if map.contains_key("fallbacks") {
                    // Format 2: WithFallbacks (legacy)
                    let primary = map
                        .get("primary")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| serde::de::Error::missing_field("primary"))?
                        .to_string();
                    let fallbacks: Vec<String> = map
                        .get("fallbacks")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    Ok(CategoryConfig {
                        primary: ModelSpec::WithFallbacks { primary, fallbacks },
                        vision: None,
                        omni: None,
                        image_generation: None,
                        tts: None,
                        stt: None,
                        code: None,
                        embedding: None,
                        search: None,
                        music: None,
                    })
                } else {
                    // Format 3/4/5: Full category object
                    let primary_val = map
                        .get("primary")
                        .ok_or_else(|| serde::de::Error::missing_field("primary"))?;
                    let primary = match primary_val {
                        serde_json::Value::String(s) => ModelSpec::Simple(s.clone()),
                        serde_json::Value::Array(arr) => {
                            // primary as array: first is primary, rest are fallbacks
                            let strings: Vec<String> = arr
                                .iter()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect();
                            if strings.is_empty() {
                                return Err(serde::de::Error::custom(
                                    "primary array must not be empty",
                                ));
                            }
                            if strings.len() == 1 {
                                ModelSpec::Simple(strings.into_iter().next().unwrap())
                            } else {
                                let primary = strings[0].clone();
                                let fallbacks = strings[1..].to_vec();
                                ModelSpec::WithFallbacks { primary, fallbacks }
                            }
                        }
                        serde_json::Value::Object(_) => {
                            serde_json::from_value::<ModelSpec>(primary_val.clone())
                                .map_err(serde::de::Error::custom)?
                        }
                        _ => {
                            return Err(serde::de::Error::custom(
                                "primary must be a string, array, or object",
                            ))
                        }
                    };

                    /// Parse a category value: string → vec!["model"], array → vec, null → None
                    fn opt_vec(
                        map: &serde_json::Map<String, serde_json::Value>,
                        key: &str,
                    ) -> Option<Vec<String>> {
                        match map.get(key) {
                            None | Some(serde_json::Value::Null) => None,
                            Some(serde_json::Value::String(s)) => {
                                if s.is_empty() {
                                    None
                                } else {
                                    Some(vec![s.clone()])
                                }
                            }
                            Some(serde_json::Value::Array(arr)) => {
                                let v: Vec<String> = arr
                                    .iter()
                                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                    .collect();
                                if v.is_empty() {
                                    None
                                } else {
                                    Some(v)
                                }
                            }
                            _ => None,
                        }
                    }

                    Ok(CategoryConfig {
                        primary,
                        vision: opt_vec(map, "vision"),
                        omni: opt_vec(map, "omni"),
                        image_generation: opt_vec(map, "image_generation"),
                        tts: opt_vec(map, "tts"),
                        stt: opt_vec(map, "stt"),
                        code: opt_vec(map, "code"),
                        embedding: opt_vec(map, "embedding"),
                        search: opt_vec(map, "search"),
                        music: opt_vec(map, "music"),
                    })
                }
            }
            _ => Err(serde::de::Error::custom(
                "expected string or object for model config",
            )),
        }
    }
}

// ── Multi-Agent ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AgentsConfig {
    pub defaults: AgentDefaults,
    #[serde(default)]
    pub list: Vec<AgentEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentDefaults {
    pub workspace: String,
    pub model: Option<String>,
    #[serde(rename = "thinkingLevel")]
    pub thinking_level: Option<String>,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: "~/.adk-gateway/workspace".into(),
            model: None,
            thinking_level: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEntry {
    pub id: String,
    #[serde(default)]
    pub default: bool,
    pub workspace: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub skills: Vec<String>,
    /// Browser automation and computer-use tool configuration for this agent.
    #[serde(default)]
    pub browser: Option<AgentBrowserConfig>,
    /// Custom tool entries for this agent (R5.2).
    #[serde(default)]
    pub tools: Vec<CustomToolConfig>,
    /// Per-agent max_iterations override. When set, this takes precedence over
    /// the gateway-level `runner.maxIterations` for requests handled by this agent.
    /// Must be within [1, 1000] if specified.
    #[serde(default, rename = "maxIterations")]
    pub max_iterations: Option<u32>,
    /// Per-agent ACP (Agent Communication Protocol) configuration.
    /// When the `acp` feature is enabled, this configures task delegation to
    /// external coding agents (Claude Code, Codex). Ignored when `acp` feature
    /// is not enabled.
    #[serde(default)]
    pub acp: Option<serde_json::Value>,
}

/// Configuration for a custom tool entry defined in agent configuration (R5.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomToolConfig {
    /// Canonical tool name.
    pub name: String,
    /// Human-readable description of the tool.
    #[serde(default)]
    pub description: String,
    /// Optional tool-specific configuration.
    #[serde(default)]
    pub config: Option<Value>,
}

// ── Gateway Server ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerSettings {
    pub port: u16,
    pub bind: BindMode,
    pub auth: Option<AuthConfig>,
    /// Graceful shutdown drain timeout in seconds (default: 30).
    #[serde(default = "default_drain_timeout_secs", rename = "drainTimeoutSecs")]
    pub drain_timeout_secs: u64,
    /// Encryption configuration for sensitive config values.
    #[serde(default)]
    pub encryption: Option<EncryptionConfig>,
}

/// Configuration for config value encryption.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EncryptionConfig {
    /// Path to the encryption key file (32 bytes raw or base64-encoded).
    #[serde(rename = "keyFile")]
    pub key_file: PathBuf,
}

fn default_drain_timeout_secs() -> u64 {
    30
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            port: 18789,
            bind: BindMode::Loopback,
            auth: None,
            drain_timeout_secs: default_drain_timeout_secs(),
            encryption: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum BindMode {
    #[default]
    Loopback,
    Lan,
    Tailnet,
    Custom(String),
}


impl BindMode {
    pub fn to_addr(&self) -> &str {
        match self {
            BindMode::Loopback => "127.0.0.1",
            BindMode::Lan => "0.0.0.0",
            BindMode::Tailnet => "0.0.0.0", // Tailscale handles routing
            BindMode::Custom(addr) => addr,
        }
    }
}

// ── Rate Limit Configuration ───────────────────────────────────────

/// Configuration for the sliding-window rate limiter.
///
/// Controls how aggressively tool invocations are throttled to prevent
/// runaway agent loops. Each agent can have its own rate limit configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitConfig {
    /// Maximum tool calls allowed within the sliding window (default: 10).
    #[serde(rename = "maxCalls")]
    pub max_calls: u32,
    /// Sliding window duration in seconds (default: 5).
    #[serde(rename = "windowSecs")]
    pub window_secs: u64,
    /// Cooldown pause duration in seconds when rate is exceeded (default: 3).
    #[serde(rename = "cooldownSecs")]
    pub cooldown_secs: u64,
    /// Maximum number of rate-limit triggers before termination (default: 3).
    #[serde(rename = "maxTriggers")]
    pub max_triggers: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_calls: 100,      // 100 calls before first pause (was 10)
            window_secs: 30,     // 30 second window (was 5)
            cooldown_secs: 5,    // 5 second pause (was 3)
            max_triggers: 10,    // 10 pauses before terminate (was 3)
        }
    }
}

// ── Tool Approval Configuration ────────────────────────────────────

/// Configuration for the tool approval flow.
///
/// Controls which tools require explicit user approval before execution.
/// When `require_approval` is non-empty, it takes complete precedence over
/// the default dangerous tool categories.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApprovalConfig {
    /// Tool name patterns that require approval.
    /// Supports glob-style patterns: `"fs_*"`, `"*_exec"`, `"*delete*"`.
    /// When non-empty, these rules completely override the defaults.
    /// When empty, defaults are used: `fs_write`, `fs_delete`, `shell_exec`, `run_command`.
    #[serde(rename = "requireApproval")]
    pub require_approval: Vec<String>,
    /// Timeout in seconds before auto-reject (default: 120).
    #[serde(rename = "timeoutSecs")]
    pub timeout_secs: u64,
}

impl Default for ApprovalConfig {
    fn default() -> Self {
        Self {
            require_approval: vec![],
            timeout_secs: 120,
        }
    }
}

// ── Stale Context Configuration ────────────────────────────────────

/// Configuration for the stale context detector.
///
/// Controls when the system considers a user's session "stale" and triggers
/// a welcome-back message summarizing pending items and alerts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StaleContextConfig {
    /// Idle threshold in seconds before a session is considered stale.
    /// Default: 14400 (4 hours).
    #[serde(rename = "idleThresholdSecs")]
    pub idle_threshold_secs: u64,
}

impl Default for StaleContextConfig {
    fn default() -> Self {
        Self {
            idle_threshold_secs: 14400,
        }
    }
}

// ── Heartbeat V2 Configuration ─────────────────────────────────────

/// Configuration for the session-integrated Heartbeat V2 system.
///
/// Replaces the cron-based heartbeat with a per-user, session-aware heartbeat
/// that runs within the user's active session context. Each paired user gets
/// an independent heartbeat schedule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HeartbeatV2Config {
    /// Whether the heartbeat V2 system is enabled (default: true).
    pub enabled: bool,
    /// Default heartbeat interval in seconds per user (default: 3600 = 1 hour).
    #[serde(rename = "defaultIntervalSecs")]
    pub default_interval_secs: u64,
    /// The heartbeat prompt injected into the session.
    /// Should instruct the agent to reply "HEARTBEAT_OK" if nothing needs attention.
    pub prompt: String,
}

impl Default for HeartbeatV2Config {
    fn default() -> Self {
        Self {
            enabled: true,
            default_interval_secs: 3600,
            prompt: "System heartbeat: Check conversation history for any pending or incomplete work. If there is unfinished work, continue it autonomously using available tools — do not ask for permission. Report progress concisely. If all work is complete, reply with exactly HEARTBEAT_OK.".to_string(),
        }
    }
}

// ── Multi-User Configuration ───────────────────────────────────────

/// Configuration for multi-user support.
///
/// Controls how multiple users are managed per channel, including
/// per-group agent routing and default agent assignment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MultiUserConfig {
    /// Whether multi-user support is enabled (default: true).
    pub enabled: bool,
    /// The default agent ID for messages without a matching routing rule.
    #[serde(rename = "defaultAgent")]
    pub default_agent: String,
    /// Per-group agent routing rules.
    #[serde(default, rename = "routingRules")]
    pub routing_rules: Vec<GroupRoutingRule>,
    /// Default heartbeat interval in seconds for new users (default: 3600 = 1 hour).
    #[serde(default = "default_heartbeat_interval_secs", rename = "defaultHeartbeatIntervalSecs")]
    pub default_heartbeat_interval_secs: u64,
}

fn default_heartbeat_interval_secs() -> u64 {
    3600
}

impl Default for MultiUserConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_agent: "default".to_string(),
            routing_rules: Vec::new(),
            default_heartbeat_interval_secs: default_heartbeat_interval_secs(),
        }
    }
}

/// A routing rule that maps a group (and optionally a thread) to a specific agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupRoutingRule {
    /// The group ID this rule applies to.
    #[serde(rename = "groupId")]
    pub group_id: String,
    /// Optional thread ID for more specific routing.
    #[serde(rename = "threadId")]
    pub thread_id: Option<String>,
    /// The agent ID to route messages to.
    #[serde(rename = "agentId")]
    pub agent_id: String,
}

// ── Health Monitor Configuration ───────────────────────────────────

/// Configuration for the health monitor subsystem.
///
/// Controls periodic health checks for gateway components (channel connectivity,
/// model reachability, session store availability) and alerting behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HealthMonitorConfig {
    /// Interval between health checks in seconds (default: 60).
    #[serde(rename = "checkIntervalSecs")]
    pub check_interval_secs: u64,
    /// Number of consecutive failures before emitting an alert (default: 3).
    #[serde(rename = "failureThreshold")]
    pub failure_threshold: u32,
    /// Optional webhook URL for alert delivery (POST JSON payload).
    #[serde(rename = "alertWebhookUrl")]
    pub alert_webhook_url: Option<String>,
    /// Optional Telegram admin chat ID for alert delivery.
    #[serde(rename = "alertTelegramAdmin")]
    pub alert_telegram_admin: Option<String>,
}

impl Default for HealthMonitorConfig {
    fn default() -> Self {
        Self {
            check_interval_secs: 60,
            failure_threshold: 3,
            alert_webhook_url: None,
            alert_telegram_admin: None,
        }
    }
}

// ── Runner Configuration ───────────────────────────────────────────

/// Gateway-level runner configuration.
///
/// Controls how the adk-runner behaves when processing requests through the gateway.
/// The gateway default of 25 iterations is more conservative than the Runner's
/// internal default of 100, providing a safety net for gateway-hosted agents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayRunnerConfig {
    /// Maximum iterations per request (default: 25 for gateway, Runner default: 100).
    #[serde(rename = "maxIterations")]
    pub max_iterations: u32,
}

impl Default for GatewayRunnerConfig {
    fn default() -> Self {
        Self {
            max_iterations: 100,
        }
    }
}

impl GatewayRunnerConfig {
    /// Validate that `max_iterations` is within the allowed range [1, 1000].
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.max_iterations < 1 || self.max_iterations > 1000 {
            return Err(ConfigError::InvalidMaxIterations(self.max_iterations));
        }
        Ok(())
    }

    /// Resolve the effective `max_iterations` for a given agent.
    ///
    /// If the agent has a per-agent `maxIterations` override configured, that value
    /// takes precedence. Otherwise, the gateway-level default (`runner.maxIterations`)
    /// is used. This enables per-request differentiation when different agents need
    /// different iteration budgets.
    pub fn resolve_max_iterations(&self, agent_entry: Option<&AgentEntry>) -> u32 {
        agent_entry
            .and_then(|entry| entry.max_iterations)
            .unwrap_or(self.max_iterations)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthConfig {
    pub mode: AuthMode,
    pub token: Option<String>,
    pub password: Option<String>,
    #[serde(default)]
    pub roles: Vec<RoleConfig>,
    #[serde(default, rename = "userMappings")]
    pub user_mappings: Vec<UserRoleMapping>,
    #[serde(default, rename = "channelOverrides")]
    pub channel_overrides: HashMap<String, ChannelAuthOverride>,
    pub audit: Option<AuditConfig>,
    pub sso: Option<SsoConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoleConfig {
    pub name: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserRoleMapping {
    #[serde(rename = "userId")]
    pub user_id: String,
    pub role: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChannelAuthOverride {
    #[serde(rename = "dmPolicy")]
    pub dm_policy: Option<DmPolicy>,
    #[serde(default)]
    pub roles: Vec<RoleConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditConfig {
    #[serde(default)]
    pub enabled: bool,
    pub sink: AuditSinkType,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditSinkType {
    File,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SsoConfig {
    #[serde(rename = "jwksUrl")]
    pub jwks_url: String,
    pub issuer: String,
    pub audience: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthMode {
    Token,
    Password,
    None,
}

// ── Channels ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ChannelsConfig {
    pub telegram: Option<TelegramConfig>,
    pub slack: Option<SlackConfig>,
    /// Additional Telegram accounts for multi-account support (R12).
    #[serde(default, rename = "telegramAccounts")]
    pub telegram_accounts: Vec<TelegramConfig>,
    /// Additional Slack accounts for multi-account support (R12).
    #[serde(default, rename = "slackAccounts")]
    pub slack_accounts: Vec<SlackConfig>,
    // Phase 2 channels
    pub whatsapp: Option<WhatsAppConfig>,
    pub discord: Option<DiscordConfig>,
    pub matrix: Option<MatrixConfig>,
    pub signal: Option<serde_json::Value>,
    pub imessage: Option<serde_json::Value>,
}

/// WhatsApp Cloud API channel configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WhatsAppConfig {
    pub enabled: bool,
    #[serde(default = "default_account_id", rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "phoneNumberId")]
    pub phone_number_id: String,
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "verifyToken")]
    pub verify_token: String,
    #[serde(rename = "webhookPath")]
    pub webhook_path: String,
}

impl Default for WhatsAppConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            account_id: default_account_id(),
            phone_number_id: String::new(),
            access_token: String::new(),
            verify_token: String::new(),
            webhook_path: "/webhook/whatsapp".to_string(),
        }
    }
}

/// Discord bot channel configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordConfig {
    pub enabled: bool,
    #[serde(default = "default_account_id", rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "botToken")]
    pub bot_token: String,
    #[serde(rename = "applicationId")]
    pub application_id: String,
    #[serde(default, rename = "guildIds")]
    pub guild_ids: Vec<String>,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            account_id: default_account_id(),
            bot_token: String::new(),
            application_id: String::new(),
            guild_ids: vec![],
        }
    }
}

/// Matrix channel configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MatrixConfig {
    pub enabled: bool,
    #[serde(default = "default_account_id", rename = "accountId")]
    pub account_id: String,
    #[serde(rename = "homeserverUrl")]
    pub homeserver_url: String,
    #[serde(rename = "accessToken")]
    pub access_token: String,
    #[serde(rename = "userId")]
    pub user_id: String,
    #[serde(default, rename = "roomIds")]
    pub room_ids: Vec<String>,
}

impl Default for MatrixConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            account_id: default_account_id(),
            homeserver_url: String::new(),
            access_token: String::new(),
            user_id: String::new(),
            room_ids: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramConfig {
    pub enabled: bool,
    #[serde(rename = "botToken")]
    pub bot_token: String,
    #[serde(rename = "dmPolicy")]
    pub dm_policy: DmPolicy,
    #[serde(rename = "allowFrom")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub groups: GroupsConfig,
    /// Streaming mode: "partial" sends edits, "complete" waits for full response
    #[serde(rename = "streamMode")]
    pub stream_mode: Option<String>,
    /// Account identifier for multi-account support (R12).
    #[serde(default = "default_account_id", rename = "accountId")]
    pub account_id: String,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bot_token: String::new(),
            dm_policy: DmPolicy::Pairing,
            allow_from: vec![],
            groups: GroupsConfig::default(),
            stream_mode: None,
            account_id: default_account_id(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SlackConfig {
    pub enabled: bool,
    #[serde(rename = "botToken")]
    pub bot_token: String,
    #[serde(rename = "appToken")]
    pub app_token: String,
    #[serde(rename = "dmPolicy")]
    pub dm_policy: DmPolicy,
    #[serde(rename = "allowFrom")]
    pub allow_from: Vec<String>,
    /// Account identifier for multi-account support (R12).
    #[serde(default = "default_account_id", rename = "accountId")]
    pub account_id: String,
}

impl Default for SlackConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bot_token: String::new(),
            app_token: String::new(),
            dm_policy: DmPolicy::Pairing,
            allow_from: vec![],
            account_id: default_account_id(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum DmPolicy {
    #[default]
    Pairing,
    Allowlist,
    Open,
    Disabled,
}


#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct GroupsConfig {
    #[serde(flatten)]
    pub rules: HashMap<String, GroupRule>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupRule {
    #[serde(rename = "requireMention")]
    pub require_mention: Option<bool>,
}

// ── Routing ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RoutingConfig {
    #[serde(default)]
    pub bindings: Vec<RoutingBinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingBinding {
    #[serde(rename = "agentId")]
    pub agent_id: String,
    #[serde(rename = "match")]
    pub match_rule: RoutingMatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingMatch {
    pub channel: Option<String>,
    #[serde(rename = "accountId")]
    pub account_id: Option<String>,
    pub peer: Option<serde_json::Value>,
}

// ── Session ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    #[serde(rename = "dmScope")]
    pub dm_scope: String,
    pub reset: SessionResetConfig,
    /// Session storage backend type (defaults to InMemory)
    pub backend: SessionBackendType,
    /// Connection string for persistent backends (SQLite path, Postgres URL, Redis URL, etc.)
    #[serde(rename = "connectionString")]
    pub connection_string: Option<String>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            dm_scope: "per-channel-peer".into(),
            reset: SessionResetConfig::default(),
            backend: SessionBackendType::default(),
            connection_string: None,
        }
    }
}

/// Supported session storage backends.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionBackendType {
    #[default]
    #[serde(rename = "inmemory")]
    InMemory,
    Sqlite,
    Postgres,
    Redis,
    Firestore,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionResetConfig {
    pub mode: String,
    #[serde(rename = "atHour")]
    pub at_hour: Option<u8>,
    #[serde(rename = "idleMinutes")]
    pub idle_minutes: Option<u64>,
}

impl Default for SessionResetConfig {
    fn default() -> Self {
        Self {
            mode: "daily".into(),
            at_hour: Some(4),
            idle_minutes: Some(120),
        }
    }
}

// ── Hooks ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HooksConfig {
    pub enabled: bool,
    pub token: Option<String>,
    pub path: Option<String>,
}

// ── Cron ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CronConfig {
    #[serde(default)]
    pub jobs: Vec<CronJob>,
}

/// The target type for a cron job execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CronJobTarget {
    /// Route through the standard agent pipeline (default behavior).
    SystemAgent,
    /// Route to a specific custom agent by ID.
    CustomAgent { agent_id: String },
    /// Delegate to a registered coding agent.
    CodingAgent { agent_id: String },
}

impl Default for CronJobTarget {
    fn default() -> Self {
        Self::SystemAgent
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub schedule: String,
    pub message: String,
    #[serde(rename = "deliverTo")]
    pub deliver_to: Option<CronDelivery>,
    /// If the agent's response contains only this keyword, suppress delivery.
    /// Used for heartbeat-style tasks where "nothing to report" shouldn't ping the user.
    #[serde(default, rename = "suppressKeyword", skip_serializing_if = "Option::is_none")]
    pub suppress_keyword: Option<String>,
    /// The target for this cron job. Defaults to `SystemAgent` (standard pipeline).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<CronJobTarget>,
    /// Optional workspace override for coding agent targets.
    /// Specifies which workspace the coding agent should operate in for this job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CronDelivery {
    pub channel: String,
    pub target: String,
}

// ── Memory ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub backend: MemoryBackend,
    #[serde(rename = "connectionString")]
    pub connection_string: Option<String>,
    pub embedding: EmbeddingConfig,
    /// Maximum observations to keep per entity (oldest trimmed). Defaults to 50.
    #[serde(default = "default_max_observations")]
    pub max_observations: usize,
    /// Maximum observations shown per entity in the summary. Defaults to 10.
    #[serde(default = "default_summary_observations")]
    pub summary_observations: usize,
    /// Path to the memory protocol markdown file. Defaults to "memory.md".
    /// This file teaches the agent how to use the knowledge graph intelligently.
    #[serde(default = "default_protocol_path", rename = "protocolPath")]
    pub protocol_path: PathBuf,
    /// Directory containing persistent context files (PROFILE.md, USER.md, etc.).
    /// Defaults to "context" relative to the config file.
    #[serde(default = "default_context_dir", rename = "contextDir")]
    pub context_dir: PathBuf,
}

fn default_max_observations() -> usize {
    50
}
fn default_summary_observations() -> usize {
    10
}
fn default_protocol_path() -> PathBuf {
    PathBuf::from("context/MEMORY.md")
}
fn default_context_dir() -> PathBuf {
    PathBuf::from("context")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryBackend {
    #[serde(rename = "inmemory")]
    InMemory,
    Sqlite,
    Postgres,
    Neo4j,
    #[serde(rename = "sqlrite")]
    SqlRite,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    pub provider: String,
    pub model: Option<String>,
}

// ── RAG ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RagConfig {
    #[serde(rename = "vectorStore")]
    pub vector_store: VectorStoreBackend,
    #[serde(rename = "connectionString")]
    pub connection_string: Option<String>,
    pub embedding: EmbeddingConfig,
    #[serde(default)]
    pub chunking: ChunkingStrategy,
    #[serde(rename = "chunkSize")]
    pub chunk_size: Option<usize>,
    #[serde(rename = "chunkOverlap")]
    pub chunk_overlap: Option<usize>,
    #[serde(default, rename = "watchDirs")]
    pub watch_dirs: Vec<PathBuf>,
    #[serde(rename = "ingestWebhook")]
    pub ingest_webhook: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VectorStoreBackend {
    #[serde(rename = "inmemory")]
    InMemory,
    Qdrant,
    #[serde(rename = "lancedb")]
    LanceDb,
    #[serde(rename = "pgvector")]
    PgVector,
    #[serde(rename = "surrealdb")]
    SurrealDb,
    #[serde(rename = "sqlrite")]
    SqlRite,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChunkingStrategy {
    #[default]
    FixedSize,
    Markdown,
    Recursive,
}

// ── Plugins ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginConfig {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub config: Value,
}

fn default_true() -> bool {
    true
}

fn default_account_id() -> String {
    "default".to_string()
}

// ── Conventions ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConventionConfig {
    pub enabled: bool,
    #[serde(default, rename = "extraPatterns")]
    pub extra_patterns: Vec<String>,
    #[serde(rename = "workspaceDir")]
    pub workspace_dir: Option<PathBuf>,
}

impl Default for ConventionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_patterns: vec![],
            workspace_dir: None,
        }
    }
}

// ── Telemetry ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    #[serde(rename = "logFormat")]
    pub log_format: LogFormat,
    #[serde(rename = "otelEndpoint")]
    pub otel_endpoint: Option<String>,
    #[serde(default, rename = "metricsEnabled")]
    pub metrics_enabled: bool,
    /// Directory for persistent daily log files. If set, logs are written to
    /// `{log_dir}/adk-gateway.YYYY-MM-DD.log` with daily rotation.
    #[serde(default, rename = "logDir")]
    pub log_dir: Option<String>,
    /// Log rotation and retention configuration.
    #[serde(default, rename = "logRotation")]
    pub log_rotation: LogRotationConfig,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            log_format: LogFormat::Text,
            otel_endpoint: None,
            metrics_enabled: false,
            log_dir: None,
            log_rotation: LogRotationConfig::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
    Pretty,
}

// ── Log Rotation Configuration ─────────────────────────────────────

/// Configuration for log rotation and retention.
///
/// Controls how log files are rotated (daily, hourly, or by size) and how long
/// they are retained before automatic deletion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogRotationConfig {
    /// Rotation policy: daily, hourly, or size-based (default: daily).
    pub rotation: RotationPolicy,
    /// Number of days to retain log files before deletion (default: 7).
    #[serde(rename = "retentionDays")]
    pub retention_days: u32,
    /// Maximum file size in megabytes before rotation (default: 100).
    #[serde(rename = "maxFileSizeMb")]
    pub max_file_size_mb: u64,
    /// Log output format override. When set, takes precedence over the top-level `logFormat`.
    /// Supports `json` (structured JSON) or `pretty` (human-readable).
    pub format: Option<LogFormat>,
}

impl Default for LogRotationConfig {
    fn default() -> Self {
        Self {
            rotation: RotationPolicy::Daily,
            retention_days: 7,
            max_file_size_mb: 100,
            format: None,
        }
    }
}

impl LogRotationConfig {
    /// Determine which log files should be deleted based on retention policy.
    ///
    /// Returns paths of files whose creation date is more than `retention_days`
    /// days before `now`. Files within the retention window are never returned.
    pub fn files_to_delete(&self, files: &[LogFileInfo], now: chrono::DateTime<chrono::Utc>) -> Vec<std::path::PathBuf> {
        let retention_duration = chrono::Duration::days(self.retention_days as i64);
        let cutoff = now - retention_duration;

        files
            .iter()
            .filter(|f| f.created_at < cutoff)
            .map(|f| f.path.clone())
            .collect()
    }
}

/// Rotation policy for log files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RotationPolicy {
    /// Rotate log files daily (default).
    #[default]
    Daily,
    /// Rotate log files hourly.
    Hourly,
    /// Rotate log files when they exceed a size threshold.
    Size {
        /// Maximum file size in bytes before rotation.
        max_bytes: u64,
    },
}

/// Metadata about a log file used for retention decisions.
#[derive(Debug, Clone, PartialEq)]
pub struct LogFileInfo {
    /// Path to the log file.
    pub path: std::path::PathBuf,
    /// When the log file was created.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Size of the log file in bytes.
    pub size_bytes: u64,
}

// ── Graph Workflows ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphWorkflowConfig {
    #[serde(default)]
    pub nodes: Vec<GraphNodeConfig>,
    #[serde(default)]
    pub edges: Vec<GraphEdgeConfig>,
    #[serde(default, rename = "stateReducers")]
    pub state_reducers: HashMap<String, ReducerType>,
    pub checkpoint: Option<CheckpointConfig>,
    #[serde(rename = "streamMode")]
    pub stream_mode: Option<GraphStreamMode>,
    #[serde(rename = "maxIterations")]
    pub max_iterations: Option<u32>,
    pub interrupts: Option<InterruptConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNodeConfig {
    pub id: String,
    #[serde(rename = "nodeType")]
    pub node_type: GraphNodeType,
    #[serde(default)]
    pub config: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphNodeType {
    Agent,
    Action,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphEdgeConfig {
    pub from: String,
    pub to: String,
    pub condition: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReducerType {
    Overwrite,
    Append,
    Sum,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphStreamMode {
    Values,
    Updates,
    Messages,
    Debug,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckpointConfig {
    pub backend: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterruptConfig {
    #[serde(default)]
    pub before: Vec<String>,
    #[serde(default)]
    pub after: Vec<String>,
}

// ── Action Nodes ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionNodeConfig {
    #[serde(rename = "actionType")]
    pub action_type: ActionType,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionType {
    Http,
    Database,
    File,
    Transform,
    Set,
    Switch,
    Loop,
    Merge,
    Wait,
    Code,
    Email,
    Notification,
    Rss,
    Trigger,
}

// ── Loading ────────────────────────────────────────────────────────

/// Default config path: `~/.adk-gateway/gateway.json`
pub fn default_config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let new_path = home.join(".adk-gateway").join("gateway.json");
    if new_path.exists() {
        return new_path;
    }
    // Path doesn't exist yet — use it anyway (will be created on first run)
    new_path
}

/// Load and parse JSON5 config with env var substitution.
/// If the config file doesn't exist, creates it with sensible defaults.
pub fn load_config(path: &Path) -> anyhow::Result<GatewayConfig> {
    if !path.exists() {
        tracing::info!(?path, "config file not found, creating with defaults");
        // Create the directory and a minimal config file
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let default_config = serde_json::json!({
            "agent": { "model": { "primary": "openai/gpt-5.4-mini", "fallbacks": ["openai/gpt-5.4-nano"] } },
            "gateway": { "port": 18789, "bind": "loopback" },
            "telemetry": { "logDir": "logs" }
        });
        let json = serde_json::to_string_pretty(&default_config).unwrap_or_default();
        if let Err(e) = std::fs::write(path, &json) {
            tracing::warn!(?path, error = %e, "failed to create default config file");
        }
        return Ok(GatewayConfig::default());
    }

    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;
    let expanded = expand_env_vars(&raw);
    let config: GatewayConfig = json5::from_str(&expanded)
        .with_context(|| format!("failed to parse config file: {}", path.display()))?;

    // Validate runner configuration (max_iterations bounds, etc.)
    config.runner.validate().with_context(|| {
        format!("invalid runner configuration in {}", path.display())
    })?;

    // Validate coding agent backend definitions
    for backend in &config.coding_agents.backends {
        let errors = validate_backend_definition(backend);
        if !errors.is_empty() {
            tracing::warn!(
                agent_type = %backend.agent_type,
                errors = ?errors,
                "invalid coding agent backend definition, skipping"
            );
        }
    }

    tracing::info!(?path, "loaded configuration");
    Ok(config)
}

/// Expand `${VAR_NAME}` patterns in config text with environment variables.
/// Values are JSON-escaped to prevent injection attacks.
pub fn expand_env_vars(input: &str) -> String {
    let mut result = input.to_string();
    // Match ${VAR_NAME} patterns
    let re = regex::Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap();
    for cap in re.captures_iter(input) {
        let full_match = &cap[0];
        let var_name = &cap[1];
        if let Ok(val) = std::env::var(var_name) {
            // Escape the value so it can't break JSON/JSON5 structure
            let escaped = val
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t");
            result = result.replace(full_match, &escaped);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_config() {
        let json = r#"{ "agent": { "model": "anthropic/claude-sonnet-4" } }"#;
        let cfg: GatewayConfig = json5::from_str(json).unwrap();
        assert_eq!(cfg.agent.model.primary(), "anthropic/claude-sonnet-4");
        assert_eq!(cfg.gateway.port, 18789);
    }

    #[test]
    fn test_telegram_config() {
        let json = r#"{
            "channels": {
                "telegram": {
                    "botToken": "123:ABC",
                    "allowFrom": ["@user1"],
                    "dmPolicy": "open"
                }
            }
        }"#;
        let cfg: GatewayConfig = json5::from_str(json).unwrap();
        let tg = cfg.channels.telegram.unwrap();
        assert_eq!(tg.bot_token, "123:ABC");
        assert!(matches!(tg.dm_policy, DmPolicy::Open));
    }

    #[test]
    fn test_env_var_expansion() {
        // SAFETY: test runs single-threaded, no concurrent env access
        unsafe { std::env::set_var("TEST_TOKEN_XYZ", "secret123") };
        let input = r#"{"token": "${TEST_TOKEN_XYZ}"}"#;
        let expanded = expand_env_vars(input);
        assert!(expanded.contains("secret123"));
        unsafe { std::env::remove_var("TEST_TOKEN_XYZ") };
    }

    #[test]
    fn test_runner_config_default() {
        let cfg = GatewayRunnerConfig::default();
        assert_eq!(cfg.max_iterations, 100);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_runner_config_validation_boundaries() {
        // Valid boundaries
        let cfg = GatewayRunnerConfig { max_iterations: 1 };
        assert!(cfg.validate().is_ok());

        let cfg = GatewayRunnerConfig { max_iterations: 1000 };
        assert!(cfg.validate().is_ok());

        // Invalid: below minimum
        let cfg = GatewayRunnerConfig { max_iterations: 0 };
        assert!(matches!(cfg.validate(), Err(ConfigError::InvalidMaxIterations(0))));

        // Invalid: above maximum
        let cfg = GatewayRunnerConfig { max_iterations: 1001 };
        assert!(matches!(cfg.validate(), Err(ConfigError::InvalidMaxIterations(1001))));
    }

    #[test]
    fn test_runner_resolve_max_iterations_no_agent() {
        let cfg = GatewayRunnerConfig { max_iterations: 25 };
        // No agent entry → uses gateway default
        assert_eq!(cfg.resolve_max_iterations(None), 25);
    }

    #[test]
    fn test_runner_resolve_max_iterations_agent_without_override() {
        let cfg = GatewayRunnerConfig { max_iterations: 25 };
        let agent = AgentEntry {
            id: "test-agent".into(),
            default: false,
            workspace: None,
            model: None,
            skills: vec![],
            browser: None,
            tools: vec![],
            max_iterations: None, // No per-agent override
            acp: None,
        };
        // Agent without override → uses gateway default
        assert_eq!(cfg.resolve_max_iterations(Some(&agent)), 25);
    }

    #[test]
    fn test_runner_resolve_max_iterations_agent_with_override() {
        let cfg = GatewayRunnerConfig { max_iterations: 25 };
        let agent = AgentEntry {
            id: "heavy-agent".into(),
            default: false,
            workspace: None,
            model: None,
            skills: vec![],
            browser: None,
            tools: vec![],
            max_iterations: Some(50), // Per-agent override
            acp: None,
        };
        // Agent with override → uses agent's value
        assert_eq!(cfg.resolve_max_iterations(Some(&agent)), 50);
    }

    #[test]
    fn test_runner_config_deserialization() {
        let json = r#"{ "runner": { "maxIterations": 42 } }"#;
        let cfg: GatewayConfig = json5::from_str(json).unwrap();
        assert_eq!(cfg.runner.max_iterations, 42);
    }

    #[test]
    fn test_agent_entry_max_iterations_deserialization() {
        let json = r#"{
            "agents": {
                "list": [{
                    "id": "research",
                    "maxIterations": 100
                }]
            }
        }"#;
        let cfg: GatewayConfig = json5::from_str(json).unwrap();
        assert_eq!(cfg.agents.list[0].max_iterations, Some(100));
    }

    #[test]
    fn test_agent_entry_no_max_iterations_deserialization() {
        let json = r#"{
            "agents": {
                "list": [{
                    "id": "simple"
                }]
            }
        }"#;
        let cfg: GatewayConfig = json5::from_str(json).unwrap();
        assert_eq!(cfg.agents.list[0].max_iterations, None);
    }
}
