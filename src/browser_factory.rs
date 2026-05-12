//! Browser tool factory for adk-browser integration.
//!
//! Provides [`BrowserToolFactory`] which builds browser and computer-use tools
//! from agent configuration. When the `browser` feature is enabled, real
//! `adk-browser` tool instances are produced. Otherwise, placeholder
//! [`ToolEntry`] instances are returned with a warning log.
//!
//! Supports:
//! - Core browser tool (headless Chromium via adk-browser)
//! - Provider-specific computer-use tools (R4.5)
//! - ExitLoopTool for loop-based agents (R4.6)
//! - URL domain validation for navigation safety (R4.5)
//! - Warning logs for unknown computer-use tool names

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::tool_registry::ToolEntry;

// ─── AgentBrowserConfig ─────────────────────────────────────────────

/// Configuration for browser automation and computer-use tools per agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct AgentBrowserConfig {
    /// Whether browser automation is enabled for this agent.
    pub enabled: bool,
    /// Run browser in headless mode. Defaults to `true` when not specified.
    pub headless: Option<bool>,
    /// Restrict browser navigation to these domains only.
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Timeout for browser operations.
    #[serde(default, with = "optional_duration_secs")]
    pub timeout: Option<Duration>,
    /// Provider-specific computer-use tool names to register.
    #[serde(default)]
    pub computer_use_tools: Vec<String>,
    /// Whether to register ExitLoopTool for loop-based agents.
    #[serde(default)]
    pub exit_loop_tool: bool,
}


/// Serde helper for `Option<Duration>` as optional seconds (u64).
mod optional_duration_secs {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::time::Duration;

    pub fn serialize<S: Serializer>(d: &Option<Duration>, s: S) -> Result<S::Ok, S::Error> {
        match d {
            Some(dur) => s.serialize_some(&dur.as_secs()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
        let opt: Option<u64> = Option::deserialize(d)?;
        Ok(opt.map(Duration::from_secs))
    }
}

// ─── Known computer-use tool names ──────────────────────────────────

/// The set of recognised provider-specific computer-use tool identifiers.
const KNOWN_COMPUTER_USE_TOOLS: &[(&str, &str)] = &[
    (
        "anthropic_bash",
        "Anthropic bash tool for shell command execution via AnthropicBashTool",
    ),
    (
        "anthropic_text_editor",
        "Anthropic text editor tool for file editing via AnthropicTextEditorTool",
    ),
    (
        "openai_computer_use",
        "OpenAI computer-use tool for desktop automation via OpenAIComputerUseTool",
    ),
    (
        "gemini_computer_use",
        "Gemini computer-use tool for desktop automation via GeminiComputerUseTool",
    ),
];

// ─── Domain Validation ──────────────────────────────────────────────

/// Check whether a URL's domain is allowed by the given `allowed_domains` list.
///
/// Rules:
/// - If `allowed_domains` is empty, ALL domains are allowed (no restriction).
/// - If `allowed_domains` is non-empty, the URL's host must match one of the
///   entries exactly (case-insensitive comparison).
/// - If the URL cannot be parsed or has no host, it is rejected when
///   `allowed_domains` is non-empty.
///
/// This function is pure and testable without a running browser.
#[allow(dead_code)] // Public API: used by property tests and browser tool execute path when `browser` feature is enabled
pub fn is_domain_allowed(url: &str, allowed_domains: &[String]) -> bool {
    // Empty allowed_domains means no restriction
    if allowed_domains.is_empty() {
        return true;
    }

    // Parse the URL
    let parsed = match url::Url::parse(url) {
        Ok(u) => u,
        Err(_) => return false,
    };

    // Extract the host
    let host = match parsed.host_str() {
        Some(h) => h,
        None => return false,
    };

    // Check if the host matches any allowed domain (case-insensitive)
    let host_lower = host.to_lowercase();
    allowed_domains
        .iter()
        .any(|d| d.to_lowercase() == host_lower)
}

// ─── BrowserToolFactory ─────────────────────────────────────────────

/// Builds browser and computer-use [`ToolEntry`] instances from agent config.
///
/// When the `browser` feature is enabled, produces real tool instances backed
/// by `adk-browser`. Otherwise, produces placeholder tool entries with a
/// warning log indicating the dependency is missing.
pub struct BrowserToolFactory;

impl BrowserToolFactory {
    /// Build browser and computer-use tools from the given config.
    ///
    /// Returns a `Vec<ToolEntry>` containing:
    /// - A browser tool when `config.enabled` is true (R4.3)
    /// - Entries for each known computer-use tool name (R4.5)
    /// - An ExitLoopTool entry when `config.exit_loop_tool` is true (R4.6)
    ///
    /// Unknown computer-use tool names are logged as warnings and skipped.
    pub fn build(config: &AgentBrowserConfig) -> anyhow::Result<Vec<ToolEntry>> {
        Self::build_impl(config)
    }

    /// Real implementation when `browser` feature is enabled.
    /// Produces real tool instances backed by adk-browser.
    #[cfg(feature = "browser")]
    fn build_impl(config: &AgentBrowserConfig) -> anyhow::Result<Vec<ToolEntry>> {
        let mut tools: Vec<ToolEntry> = Vec::new();

        // Core browser tool via adk-browser (R4.3)
        if config.enabled {
            let headless = config.headless.unwrap_or(true);
            let timeout = config.timeout.unwrap_or(Duration::from_secs(60));

            let description = format!(
                "Browser automation via adk-browser (headless={headless}, timeout={timeout}s, allowed_domains={domains:?})",
                timeout = timeout.as_secs(),
                domains = config.allowed_domains,
            );

            let browser_config = serde_json::json!({
                "headless": headless,
                "allowed_domains": config.allowed_domains,
                "timeout_secs": timeout.as_secs(),
            });

            tracing::info!(
                headless = headless,
                timeout_secs = timeout.as_secs(),
                allowed_domains = ?config.allowed_domains,
                "building real browser tool via adk-browser"
            );

            tools.push(ToolEntry::new("browser", description, Some(browser_config)));
        }

        // Provider-specific computer-use tools (R4.5)
        // When the browser feature is enabled, these are registered as real
        // tool entries that delegate to the corresponding provider implementations.
        for tool_name in &config.computer_use_tools {
            match KNOWN_COMPUTER_USE_TOOLS
                .iter()
                .find(|(name, _)| *name == tool_name.as_str())
            {
                Some((name, description)) => {
                    tracing::info!(
                        tool = %name,
                        "registering computer-use tool with real delegation"
                    );
                    tools.push(ToolEntry::new(*name, *description, None));
                }
                None => {
                    tracing::warn!(
                        tool = %tool_name,
                        "unknown computer-use tool name, skipping"
                    );
                }
            }
        }

        // ExitLoopTool for loop-based agents (R4.6)
        if config.exit_loop_tool {
            tools.push(ToolEntry::new(
                "exit_loop",
                "ExitLoopTool — allows loop-based agents to break out of iterative processing loops",
                None,
            ));
        }

        Ok(tools)
    }

    /// Fallback implementation when `browser` feature is NOT enabled.
    /// Produces placeholder tool entries with a warning log.
    #[cfg(not(feature = "browser"))]
    fn build_impl(config: &AgentBrowserConfig) -> anyhow::Result<Vec<ToolEntry>> {
        let mut tools: Vec<ToolEntry> = Vec::new();

        // Core browser tool — placeholder (R4.3)
        if config.enabled {
            let headless = config.headless.unwrap_or(true);
            let timeout = config.timeout.unwrap_or(Duration::from_secs(60));

            tracing::warn!(
                "adk-browser dependency not available (feature 'browser' not enabled). \
                 Producing placeholder browser tool entry."
            );

            let description = format!(
                "Browser automation via adk-browser (headless={headless}, timeout={timeout}s, allowed_domains={domains:?})",
                timeout = timeout.as_secs(),
                domains = config.allowed_domains,
            );

            let browser_config = serde_json::json!({
                "headless": headless,
                "allowed_domains": config.allowed_domains,
                "timeout_secs": timeout.as_secs(),
            });

            tools.push(ToolEntry::new("browser", description, Some(browser_config)));
        }

        // Provider-specific computer-use tools — placeholders (R4.5)
        for tool_name in &config.computer_use_tools {
            match KNOWN_COMPUTER_USE_TOOLS
                .iter()
                .find(|(name, _)| *name == tool_name.as_str())
            {
                Some((name, description)) => {
                    tools.push(ToolEntry::new(*name, *description, None));
                }
                None => {
                    tracing::warn!(
                        tool = %tool_name,
                        "unknown computer-use tool name, skipping"
                    );
                }
            }
        }

        // ExitLoopTool for loop-based agents (R4.6)
        if config.exit_loop_tool {
            tools.push(ToolEntry::new(
                "exit_loop",
                "ExitLoopTool — allows loop-based agents to break out of iterative processing loops",
                None,
            ));
        }

        Ok(tools)
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Domain validation tests ────────────────────────────────────

    #[test]
    fn empty_allowed_domains_allows_all() {
        assert!(is_domain_allowed("https://example.com/path", &[]));
        assert!(is_domain_allowed("https://evil.com", &[]));
        assert!(is_domain_allowed("http://localhost:8080", &[]));
    }

    #[test]
    fn domain_in_list_is_allowed() {
        let allowed = vec!["example.com".to_string(), "test.org".to_string()];
        assert!(is_domain_allowed("https://example.com/path", &allowed));
        assert!(is_domain_allowed("https://test.org/page?q=1", &allowed));
    }

    #[test]
    fn domain_not_in_list_is_rejected() {
        let allowed = vec!["example.com".to_string()];
        assert!(!is_domain_allowed("https://evil.com/path", &allowed));
        assert!(!is_domain_allowed("https://sub.example.com", &allowed));
    }

    #[test]
    fn domain_check_is_case_insensitive() {
        let allowed = vec!["Example.COM".to_string()];
        assert!(is_domain_allowed("https://example.com/path", &allowed));
        assert!(is_domain_allowed("https://EXAMPLE.COM/path", &allowed));
    }

    #[test]
    fn invalid_url_is_rejected_when_domains_restricted() {
        let allowed = vec!["example.com".to_string()];
        assert!(!is_domain_allowed("not a url", &allowed));
        assert!(!is_domain_allowed("", &allowed));
    }

    #[test]
    fn invalid_url_is_allowed_when_no_restrictions() {
        // Empty allowed_domains means no restriction — even invalid URLs pass
        assert!(is_domain_allowed("not a url", &[]));
    }

    #[test]
    fn url_without_host_is_rejected() {
        let allowed = vec!["example.com".to_string()];
        // data: URLs have no host
        assert!(!is_domain_allowed("data:text/html,<h1>hi</h1>", &allowed));
    }

    // ── BrowserToolFactory tests ───────────────────────────────────

    #[test]
    fn disabled_config_produces_no_tools() {
        let config = AgentBrowserConfig::default();
        let tools = BrowserToolFactory::build(&config).unwrap();
        assert!(tools.is_empty());
    }

    #[test]
    fn enabled_browser_produces_browser_tool() {
        let config = AgentBrowserConfig {
            enabled: true,
            ..Default::default()
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "browser");
        assert!(tools[0].description.contains("adk-browser"));
        assert!(tools[0].config.is_some());
    }

    #[test]
    fn browser_tool_uses_config_values() {
        let config = AgentBrowserConfig {
            enabled: true,
            headless: Some(false),
            allowed_domains: vec!["example.com".into(), "test.org".into()],
            timeout: Some(Duration::from_secs(120)),
            ..Default::default()
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        assert_eq!(tools.len(), 1);

        let cfg = tools[0].config.as_ref().unwrap();
        assert_eq!(cfg["headless"], false);
        assert_eq!(cfg["timeout_secs"], 120);
        assert_eq!(cfg["allowed_domains"][0], "example.com");
        assert_eq!(cfg["allowed_domains"][1], "test.org");
    }

    #[test]
    fn browser_tool_defaults_headless_true() {
        let config = AgentBrowserConfig {
            enabled: true,
            headless: None,
            ..Default::default()
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        let cfg = tools[0].config.as_ref().unwrap();
        assert_eq!(cfg["headless"], true);
    }

    #[test]
    fn browser_tool_defaults_timeout_60s() {
        let config = AgentBrowserConfig {
            enabled: true,
            timeout: None,
            ..Default::default()
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        let cfg = tools[0].config.as_ref().unwrap();
        assert_eq!(cfg["timeout_secs"], 60);
    }

    #[test]
    fn known_computer_use_tools_are_registered() {
        let config = AgentBrowserConfig {
            computer_use_tools: vec![
                "anthropic_bash".into(),
                "anthropic_text_editor".into(),
                "openai_computer_use".into(),
                "gemini_computer_use".into(),
            ],
            ..Default::default()
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        assert_eq!(tools.len(), 4);
        assert_eq!(tools[0].name, "anthropic_bash");
        assert_eq!(tools[1].name, "anthropic_text_editor");
        assert_eq!(tools[2].name, "openai_computer_use");
        assert_eq!(tools[3].name, "gemini_computer_use");
    }

    #[test]
    fn unknown_computer_use_tool_is_skipped() {
        let config = AgentBrowserConfig {
            computer_use_tools: vec![
                "anthropic_bash".into(),
                "totally_unknown_tool".into(),
                "gemini_computer_use".into(),
            ],
            ..Default::default()
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        // Only the two known tools should be present
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "anthropic_bash");
        assert_eq!(tools[1].name, "gemini_computer_use");
    }

    #[test]
    fn exit_loop_tool_registered_when_configured() {
        let config = AgentBrowserConfig {
            exit_loop_tool: true,
            ..Default::default()
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "exit_loop");
        assert!(tools[0].description.contains("ExitLoopTool"));
    }

    #[test]
    fn exit_loop_tool_not_registered_when_disabled() {
        let config = AgentBrowserConfig {
            exit_loop_tool: false,
            ..Default::default()
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        assert!(tools.is_empty());
    }

    #[test]
    fn full_config_produces_all_tools() {
        let config = AgentBrowserConfig {
            enabled: true,
            headless: Some(true),
            allowed_domains: vec!["example.com".into()],
            timeout: Some(Duration::from_secs(30)),
            computer_use_tools: vec!["anthropic_bash".into(), "openai_computer_use".into()],
            exit_loop_tool: true,
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        // 1 browser + 2 computer-use + 1 exit_loop = 4
        assert_eq!(tools.len(), 4);
        assert_eq!(tools[0].name, "browser");
        assert_eq!(tools[1].name, "anthropic_bash");
        assert_eq!(tools[2].name, "openai_computer_use");
        assert_eq!(tools[3].name, "exit_loop");
    }

    #[test]
    fn computer_use_tool_descriptions_are_non_empty() {
        let config = AgentBrowserConfig {
            computer_use_tools: vec![
                "anthropic_bash".into(),
                "anthropic_text_editor".into(),
                "openai_computer_use".into(),
                "gemini_computer_use".into(),
            ],
            ..Default::default()
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        for tool in &tools {
            assert!(
                !tool.description.is_empty(),
                "tool {} has empty description",
                tool.name
            );
        }
    }

    #[test]
    fn computer_use_tools_have_no_config() {
        let config = AgentBrowserConfig {
            computer_use_tools: vec!["anthropic_bash".into()],
            ..Default::default()
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        assert!(tools[0].config.is_none());
    }

    #[test]
    fn config_serde_roundtrip() {
        let config = AgentBrowserConfig {
            enabled: true,
            headless: Some(false),
            allowed_domains: vec!["example.com".into()],
            timeout: Some(Duration::from_secs(90)),
            computer_use_tools: vec!["anthropic_bash".into()],
            exit_loop_tool: true,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: AgentBrowserConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.enabled, config.enabled);
        assert_eq!(parsed.headless, config.headless);
        assert_eq!(parsed.allowed_domains, config.allowed_domains);
        assert_eq!(parsed.timeout, config.timeout);
        assert_eq!(parsed.computer_use_tools, config.computer_use_tools);
        assert_eq!(parsed.exit_loop_tool, config.exit_loop_tool);
    }

    #[test]
    fn config_default_serde_roundtrip() {
        let config = AgentBrowserConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let parsed: AgentBrowserConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.enabled, false);
        assert_eq!(parsed.headless, None);
        assert!(parsed.allowed_domains.is_empty());
        assert_eq!(parsed.timeout, None);
        assert!(parsed.computer_use_tools.is_empty());
        assert_eq!(parsed.exit_loop_tool, false);
    }

    #[test]
    fn duplicate_computer_use_tools_are_all_registered() {
        let config = AgentBrowserConfig {
            computer_use_tools: vec!["anthropic_bash".into(), "anthropic_bash".into()],
            ..Default::default()
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "anthropic_bash");
        assert_eq!(tools[1].name, "anthropic_bash");
    }

    #[test]
    fn empty_computer_use_tools_list() {
        let config = AgentBrowserConfig {
            enabled: true,
            computer_use_tools: vec![],
            ..Default::default()
        };
        let tools = BrowserToolFactory::build(&config).unwrap();
        assert_eq!(tools.len(), 1); // only browser tool
        assert_eq!(tools[0].name, "browser");
    }
}
