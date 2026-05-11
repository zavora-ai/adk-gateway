//! Plugin system with lifecycle hooks for extending gateway behavior.
//!
//! Provides a `PluginManager` that loads plugins from config in order and
//! invokes lifecycle hooks at defined points in the message pipeline.
//! Supports short-circuit responses where a plugin can skip remaining
//! processing and provide a final result.
//!
//! Design: PluginManager [R22.1–R22.11]

use serde_json::Value;
use std::fmt;
use std::sync::Arc;

use crate::config::PluginConfig;

// ── Hook types ─────────────────────────────────────────────────────

/// All lifecycle hook points in the gateway pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PluginHook {
    BeforeRun,
    AfterRun,
    OnUserMessage,
    #[allow(dead_code)] // Available for event-driven plugin hooks
    OnEvent,
    BeforeAgent,
    AfterAgent,
    BeforeModel,
    AfterModel,
    BeforeTool,
    AfterTool,
}

impl fmt::Display for PluginHook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BeforeRun => write!(f, "before_run"),
            Self::AfterRun => write!(f, "after_run"),
            Self::OnUserMessage => write!(f, "on_user_message"),
            Self::OnEvent => write!(f, "on_event"),
            Self::BeforeAgent => write!(f, "before_agent"),
            Self::AfterAgent => write!(f, "after_agent"),
            Self::BeforeModel => write!(f, "before_model"),
            Self::AfterModel => write!(f, "after_model"),
            Self::BeforeTool => write!(f, "before_tool"),
            Self::AfterTool => write!(f, "after_tool"),
        }
    }
}

/// Result of invoking a plugin hook.
#[derive(Debug, Clone, PartialEq)]
pub enum HookResult {
    /// Proceed normally — no changes.
    Continue,
    /// Skip remaining processing; use this response as the final result (R22.9).
    #[allow(dead_code)] // Used in tests; returned by plugins to short-circuit pipeline
    ShortCircuit { response: String },
    /// Proceed with modified data.
    #[allow(dead_code)] // Used in tests; returned by plugins to modify pipeline data
    Modified { data: Value },
}

/// Context passed to plugin hooks with relevant pipeline data.
#[derive(Debug, Clone, Default)]
pub struct HookContext {
    /// The user message text (for OnUserMessage, BeforeRun).
    pub user_message: Option<String>,
    /// The agent being invoked (for BeforeAgent, AfterAgent).
    pub agent_name: Option<String>,
    /// The tool being called (for BeforeTool, AfterTool).
    pub tool_name: Option<String>,
    /// The model being called (for BeforeModel, AfterModel).
    pub model_name: Option<String>,
    /// Event data (for OnEvent).
    #[allow(dead_code)] // Available for OnEvent plugin hook context
    pub event_data: Option<Value>,
    /// Response text (for AfterRun).
    pub response_text: Option<String>,
    /// Arbitrary metadata.
    pub metadata: Option<Value>,
}

// ── Plugin trait ───────────────────────────────────────────────────

/// Trait that all plugins must implement.
pub trait Plugin: Send + Sync {
    /// Human-readable plugin name.
    #[allow(dead_code)] // Part of Plugin trait contract; used by plugin implementations
    fn name(&self) -> &str;

    /// Initialize the plugin. Called once at load time.
    /// Return `Err` to signal init failure (plugin will be skipped).
    fn init(&self, config: &Value) -> Result<(), PluginError>;

    /// Called when a lifecycle hook fires. Return a `HookResult` to
    /// continue, short-circuit, or modify data.
    fn on_hook(&self, hook: PluginHook, context: &HookContext) -> HookResult;
}

// ── Error type ─────────────────────────────────────────────────────

/// Errors that can occur during plugin operations.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("plugin init failed: {0}")]
    #[allow(dead_code)] // Returned by Plugin::init implementations
    InitFailed(String),
    #[error("plugin hook error: {0}")]
    #[allow(dead_code)] // Available for plugin hook error reporting
    HookError(String),
}

// ── LoadedPlugin ───────────────────────────────────────────────────

/// A successfully loaded and initialized plugin.
pub struct LoadedPlugin {
    pub name: String,
    pub plugin: Arc<dyn Plugin>,
    pub enabled: bool,
}

impl fmt::Debug for LoadedPlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadedPlugin")
            .field("name", &self.name)
            .field("enabled", &self.enabled)
            .finish()
    }
}

// ── PluginManager ──────────────────────────────────────────────────

/// Manages loaded plugins and invokes lifecycle hooks in config order.
pub struct PluginManager {
    plugins: Vec<LoadedPlugin>,
}

impl fmt::Debug for PluginManager {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginManager")
            .field("plugins", &self.plugins)
            .finish()
    }
}

impl PluginManager {
    /// Create an empty PluginManager with no plugins.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }

    /// Load plugins from config entries in listed order (R22.1).
    ///
    /// Uses the provided `resolver` to map plugin names to trait objects.
    /// On init failure: logs error, continues without that plugin (R22.10).
    pub fn load_plugins<F>(configs: &[PluginConfig], resolver: F) -> Self
    where
        F: Fn(&str) -> Option<Arc<dyn Plugin>>,
    {
        let mut plugins = Vec::new();

        for cfg in configs {
            if !cfg.enabled {
                tracing::info!(plugin = %cfg.name, "plugin disabled, skipping");
                continue;
            }

            match resolver(&cfg.name) {
                Some(plugin) => {
                    // Attempt init
                    match plugin.init(&cfg.config) {
                        Ok(()) => {
                            tracing::info!(plugin = %cfg.name, "plugin loaded");
                            plugins.push(LoadedPlugin {
                                name: cfg.name.clone(),
                                plugin,
                                enabled: true,
                            });
                        }
                        Err(e) => {
                            // R22.10: log error, continue without plugin
                            tracing::error!(
                                plugin = %cfg.name,
                                error = %e,
                                "plugin init failed, skipping"
                            );
                        }
                    }
                }
                None => {
                    tracing::error!(
                        plugin = %cfg.name,
                        "unknown plugin name, skipping"
                    );
                }
            }
        }

        Self { plugins }
    }

    /// Invoke a hook on all enabled plugins in config order (R22.11).
    ///
    /// If any plugin returns `ShortCircuit`, iteration stops immediately
    /// and the short-circuit response is returned (R22.9).
    ///
    /// If a plugin returns `Modified`, the modified data is passed to
    /// subsequent plugins via the context's `metadata` field.
    pub fn invoke_hook(&self, hook: PluginHook, context: &mut HookContext) -> HookResult {
        for loaded in &self.plugins {
            if !loaded.enabled {
                continue;
            }

            let result = loaded.plugin.on_hook(hook, context);

            match &result {
                HookResult::ShortCircuit { response } => {
                    tracing::debug!(
                        plugin = %loaded.name,
                        hook = %hook,
                        response_len = response.len(),
                        "plugin short-circuited"
                    );
                    return result;
                }
                HookResult::Modified { data } => {
                    tracing::debug!(
                        plugin = %loaded.name,
                        hook = %hook,
                        "plugin modified data"
                    );
                    // Pass modified data forward to next plugin
                    context.metadata = Some(data.clone());
                }
                HookResult::Continue => {}
            }
        }

        HookResult::Continue
    }

    /// Number of loaded (enabled) plugins.
    pub fn plugin_count(&self) -> usize {
        self.plugins.iter().filter(|p| p.enabled).count()
    }

    /// Names of loaded plugins in order.
    pub fn plugin_names(&self) -> Vec<&str> {
        self.plugins
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.name.as_str())
            .collect()
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A test plugin that records hook invocations.
    struct RecordingPlugin {
        plugin_name: String,
        call_count: AtomicUsize,
        hook_result: HookResult,
    }

    impl RecordingPlugin {
        fn new(name: &str, result: HookResult) -> Self {
            Self {
                plugin_name: name.to_string(),
                call_count: AtomicUsize::new(0),
                hook_result: result,
            }
        }

        fn calls(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    impl Plugin for RecordingPlugin {
        fn name(&self) -> &str {
            &self.plugin_name
        }

        fn init(&self, _config: &Value) -> Result<(), PluginError> {
            Ok(())
        }

        fn on_hook(&self, _hook: PluginHook, _context: &HookContext) -> HookResult {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            self.hook_result.clone()
        }
    }

    /// A plugin that always fails init.
    struct FailingPlugin {
        plugin_name: String,
    }

    impl Plugin for FailingPlugin {
        fn name(&self) -> &str {
            &self.plugin_name
        }

        fn init(&self, _config: &Value) -> Result<(), PluginError> {
            Err(PluginError::InitFailed("intentional failure".into()))
        }

        fn on_hook(&self, _hook: PluginHook, _context: &HookContext) -> HookResult {
            HookResult::Continue
        }
    }

    fn make_config(name: &str) -> PluginConfig {
        PluginConfig {
            name: name.to_string(),
            enabled: true,
            config: Value::Null,
        }
    }

    #[test]
    fn test_hooks_invoked_in_config_order() {
        // R22.11: hooks fire in config order
        let p1 = Arc::new(RecordingPlugin::new("first", HookResult::Continue));
        let p2 = Arc::new(RecordingPlugin::new("second", HookResult::Continue));
        let p3 = Arc::new(RecordingPlugin::new("third", HookResult::Continue));

        let p1c = Arc::clone(&p1);
        let p2c = Arc::clone(&p2);
        let p3c = Arc::clone(&p3);

        let configs = vec![
            make_config("first"),
            make_config("second"),
            make_config("third"),
        ];

        let manager = PluginManager::load_plugins(&configs, |name| match name {
            "first" => Some(p1c.clone() as Arc<dyn Plugin>),
            "second" => Some(p2c.clone() as Arc<dyn Plugin>),
            "third" => Some(p3c.clone() as Arc<dyn Plugin>),
            _ => None,
        });

        assert_eq!(manager.plugin_count(), 3);
        assert_eq!(manager.plugin_names(), vec!["first", "second", "third"]);

        let mut ctx = HookContext::default();
        let result = manager.invoke_hook(PluginHook::BeforeRun, &mut ctx);

        assert_eq!(result, HookResult::Continue);
        assert_eq!(p1.calls(), 1);
        assert_eq!(p2.calls(), 1);
        assert_eq!(p3.calls(), 1);
    }

    #[test]
    fn test_short_circuit_stops_subsequent_plugins() {
        // R22.9: short-circuit skips remaining plugins
        let p1 = Arc::new(RecordingPlugin::new(
            "blocker",
            HookResult::ShortCircuit {
                response: "blocked!".into(),
            },
        ));
        let p2 = Arc::new(RecordingPlugin::new("skipped", HookResult::Continue));

        let p1c = Arc::clone(&p1);
        let p2c = Arc::clone(&p2);

        let configs = vec![make_config("blocker"), make_config("skipped")];

        let manager = PluginManager::load_plugins(&configs, |name| match name {
            "blocker" => Some(p1c.clone() as Arc<dyn Plugin>),
            "skipped" => Some(p2c.clone() as Arc<dyn Plugin>),
            _ => None,
        });

        let mut ctx = HookContext::default();
        let result = manager.invoke_hook(PluginHook::OnUserMessage, &mut ctx);

        assert_eq!(
            result,
            HookResult::ShortCircuit {
                response: "blocked!".into()
            }
        );
        assert_eq!(p1.calls(), 1);
        assert_eq!(p2.calls(), 0); // skipped due to short-circuit
    }

    #[test]
    fn test_init_failure_skips_plugin() {
        // R22.10: init failure logs error, continues without plugin
        let good = Arc::new(RecordingPlugin::new("good", HookResult::Continue));
        let good_c = Arc::clone(&good);

        let configs = vec![make_config("bad"), make_config("good")];

        let manager = PluginManager::load_plugins(&configs, |name| match name {
            "bad" => Some(Arc::new(FailingPlugin {
                plugin_name: "bad".into(),
            }) as Arc<dyn Plugin>),
            "good" => Some(good_c.clone() as Arc<dyn Plugin>),
            _ => None,
        });

        // Only the good plugin should be loaded
        assert_eq!(manager.plugin_count(), 1);
        assert_eq!(manager.plugin_names(), vec!["good"]);

        let mut ctx = HookContext::default();
        manager.invoke_hook(PluginHook::BeforeRun, &mut ctx);
        assert_eq!(good.calls(), 1);
    }

    #[test]
    fn test_unknown_plugin_name_skipped() {
        let configs = vec![make_config("nonexistent")];

        let manager = PluginManager::load_plugins(&configs, |_| None);

        assert_eq!(manager.plugin_count(), 0);
    }

    #[test]
    fn test_disabled_plugin_not_loaded() {
        let configs = vec![PluginConfig {
            name: "disabled_one".into(),
            enabled: false,
            config: Value::Null,
        }];

        let manager = PluginManager::load_plugins(&configs, |name| {
            Some(Arc::new(RecordingPlugin::new(name, HookResult::Continue)) as Arc<dyn Plugin>)
        });

        assert_eq!(manager.plugin_count(), 0);
    }

    #[test]
    fn test_modified_data_passed_to_next_plugin() {
        let modifier = Arc::new(RecordingPlugin::new(
            "modifier",
            HookResult::Modified {
                data: serde_json::json!({"key": "value"}),
            },
        ));
        let reader = Arc::new(RecordingPlugin::new("reader", HookResult::Continue));

        let mod_c = Arc::clone(&modifier);
        let read_c = Arc::clone(&reader);

        let configs = vec![make_config("modifier"), make_config("reader")];

        let manager = PluginManager::load_plugins(&configs, |name| match name {
            "modifier" => Some(mod_c.clone() as Arc<dyn Plugin>),
            "reader" => Some(read_c.clone() as Arc<dyn Plugin>),
            _ => None,
        });

        let mut ctx = HookContext::default();
        let result = manager.invoke_hook(PluginHook::BeforeModel, &mut ctx);

        assert_eq!(result, HookResult::Continue);
        // After modifier runs, metadata should be set
        assert_eq!(ctx.metadata, Some(serde_json::json!({"key": "value"})));
        assert_eq!(modifier.calls(), 1);
        assert_eq!(reader.calls(), 1);
    }

    #[test]
    fn test_all_hook_types_can_be_invoked() {
        let p = Arc::new(RecordingPlugin::new("all_hooks", HookResult::Continue));
        let pc = Arc::clone(&p);

        let configs = vec![make_config("all_hooks")];
        let manager =
            PluginManager::load_plugins(&configs, |_| Some(pc.clone() as Arc<dyn Plugin>));

        let hooks = [
            PluginHook::BeforeRun,
            PluginHook::AfterRun,
            PluginHook::OnUserMessage,
            PluginHook::OnEvent,
            PluginHook::BeforeAgent,
            PluginHook::AfterAgent,
            PluginHook::BeforeModel,
            PluginHook::AfterModel,
            PluginHook::BeforeTool,
            PluginHook::AfterTool,
        ];

        for hook in hooks {
            let mut ctx = HookContext::default();
            manager.invoke_hook(hook, &mut ctx);
        }

        assert_eq!(p.calls(), 10);
    }

    #[test]
    fn test_empty_plugin_manager() {
        let manager = PluginManager::new();
        assert_eq!(manager.plugin_count(), 0);

        let mut ctx = HookContext::default();
        let result = manager.invoke_hook(PluginHook::BeforeRun, &mut ctx);
        assert_eq!(result, HookResult::Continue);
    }

    #[test]
    fn test_hook_context_fields() {
        let ctx = HookContext {
            user_message: Some("hello".into()),
            agent_name: Some("agent1".into()),
            tool_name: Some("web_search".into()),
            model_name: Some("gpt-4".into()),
            event_data: Some(serde_json::json!({"type": "partial"})),
            response_text: Some("response".into()),
            metadata: None,
        };

        assert_eq!(ctx.user_message.as_deref(), Some("hello"));
        assert_eq!(ctx.agent_name.as_deref(), Some("agent1"));
        assert_eq!(ctx.tool_name.as_deref(), Some("web_search"));
        assert_eq!(ctx.model_name.as_deref(), Some("gpt-4"));
        assert!(ctx.event_data.is_some());
        assert_eq!(ctx.response_text.as_deref(), Some("response"));
    }
}
