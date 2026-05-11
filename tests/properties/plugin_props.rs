//! Property-based tests for the plugin system.
//!
//! Feature: gateway-production-maturity
//! - Property 37: Plugin hooks fire in config order
//!   **Validates: Requirements R22.1, R22.11**
//! - Property 38: Plugin initialization failure does not abort gateway
//!   **Validates: Requirements R22.9, R22.10**

use adk_gateway::config::PluginConfig;
use adk_gateway::plugin_manager::{
    HookContext, HookResult, Plugin, PluginError, PluginHook, PluginManager,
};
use proptest::prelude::*;
use serde_json::Value;
use std::sync::{Arc, Mutex};

// ── Mock plugins ───────────────────────────────────────────────────

/// A plugin that records its name into a shared log each time on_hook is called.
struct OrderRecordingPlugin {
    plugin_name: String,
    call_log: Arc<Mutex<Vec<String>>>,
}

impl Plugin for OrderRecordingPlugin {
    fn name(&self) -> &str {
        &self.plugin_name
    }

    fn init(&self, _config: &Value) -> Result<(), PluginError> {
        Ok(())
    }

    fn on_hook(&self, _hook: PluginHook, _context: &HookContext) -> HookResult {
        self.call_log.lock().unwrap().push(self.plugin_name.clone());
        HookResult::Continue
    }
}

/// A plugin that always fails during init.
struct FailingInitPlugin {
    plugin_name: String,
}

impl Plugin for FailingInitPlugin {
    fn name(&self) -> &str {
        &self.plugin_name
    }

    fn init(&self, _config: &Value) -> Result<(), PluginError> {
        Err(PluginError::InitFailed(format!(
            "{} intentional failure",
            self.plugin_name
        )))
    }

    fn on_hook(&self, _hook: PluginHook, _context: &HookContext) -> HookResult {
        HookResult::Continue
    }
}

// ── Strategies ─────────────────────────────────────────────────────

/// Generate a random PluginHook variant.
fn hook_strategy() -> impl Strategy<Value = PluginHook> {
    prop_oneof![
        Just(PluginHook::BeforeRun),
        Just(PluginHook::AfterRun),
        Just(PluginHook::OnUserMessage),
        Just(PluginHook::OnEvent),
        Just(PluginHook::BeforeAgent),
        Just(PluginHook::AfterAgent),
        Just(PluginHook::BeforeModel),
        Just(PluginHook::AfterModel),
        Just(PluginHook::BeforeTool),
        Just(PluginHook::AfterTool),
    ]
}

// ── Property 37 ────────────────────────────────────────────────────

// Feature: gateway-production-maturity, Property 37: Plugin hooks fire in config order
// **Validates: Requirements R22.1, R22.11**
proptest! {
    /// Property 37: For N plugins (1..8) loaded in config order,
    /// invoking any hook calls each plugin's on_hook exactly once
    /// and in the order they appear in the config.
    #[test]
    fn plugin_hooks_fire_in_config_order(
        n in 1usize..=8,
        hook in hook_strategy(),
    ) {
        // Generate deterministic unique names based on n
        let names: Vec<String> = (0..n).map(|i| format!("plugin_{}", i)).collect();
        let call_log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

        // Build plugin configs in order
        let configs: Vec<PluginConfig> = names
            .iter()
            .map(|name| PluginConfig {
                name: name.clone(),
                enabled: true,
                config: Value::Null,
            })
            .collect();

        // Build resolver that creates OrderRecordingPlugin instances
        let log_clone = Arc::clone(&call_log);
        let names_clone = names.clone();
        let manager = PluginManager::load_plugins(&configs, move |name| {
            if names_clone.contains(&name.to_string()) {
                Some(Arc::new(OrderRecordingPlugin {
                    plugin_name: name.to_string(),
                    call_log: Arc::clone(&log_clone),
                }) as Arc<dyn Plugin>)
            } else {
                None
            }
        });

        // All plugins should be loaded
        prop_assert_eq!(
            manager.plugin_count(), n,
            "expected {} plugins loaded, got {}",
            n, manager.plugin_count()
        );

        // Invoke the hook
        let mut ctx = HookContext::default();
        let result = manager.invoke_hook(hook, &mut ctx);
        prop_assert_eq!(result, HookResult::Continue);

        // Verify call log matches config order exactly
        let log = call_log.lock().unwrap();
        prop_assert_eq!(
            log.len(), n,
            "expected {} hook calls, got {}",
            n, log.len()
        );
        for (i, called_name) in log.iter().enumerate() {
            prop_assert_eq!(
                called_name, &names[i],
                "hook call {} expected plugin '{}', got '{}'",
                i, names[i], called_name
            );
        }
    }
}

// ── Property 38 ────────────────────────────────────────────────────

/// Strategy to generate a Vec of booleans (true = succeeds init,
/// false = fails init) for 1..=8 plugins, ensuring at least one
/// failure exists.
fn plugin_init_outcomes_strategy() -> impl Strategy<Value = Vec<bool>> {
    proptest::collection::vec(any::<bool>(), 2..=8)
        .prop_filter("need at least one failure and one success", |outcomes| {
            outcomes.iter().any(|&ok| !ok) && outcomes.iter().any(|&ok| ok)
        })
}

// Feature: gateway-production-maturity, Property 38: Plugin initialization failure does not abort gateway
// **Validates: Requirements R22.9, R22.10**
proptest! {
    /// Property 38: For a mix of plugins where some fail init,
    /// the PluginManager still loads the successful ones and the
    /// gateway doesn't abort. Failed plugins are excluded.
    #[test]
    fn plugin_init_failure_does_not_abort(outcomes in plugin_init_outcomes_strategy()) {
        let n = outcomes.len();
        let expected_successes = outcomes.iter().filter(|&&ok| ok).count();

        // Build configs
        let configs: Vec<PluginConfig> = (0..n)
            .map(|i| PluginConfig {
                name: format!("plugin_{}", i),
                enabled: true,
                config: Value::Null,
            })
            .collect();

        let outcomes_clone = outcomes.clone();
        let manager = PluginManager::load_plugins(&configs, move |name| {
            // Extract index from name "plugin_N"
            let idx: usize = name
                .strip_prefix("plugin_")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);

            if outcomes_clone[idx] {
                // Succeeds init
                Some(Arc::new(OrderRecordingPlugin {
                    plugin_name: name.to_string(),
                    call_log: Arc::new(Mutex::new(Vec::new())),
                }) as Arc<dyn Plugin>)
            } else {
                // Fails init
                Some(Arc::new(FailingInitPlugin {
                    plugin_name: name.to_string(),
                }) as Arc<dyn Plugin>)
            }
        });

        // The manager should NOT have aborted — it loaded successfully
        prop_assert_eq!(
            manager.plugin_count(),
            expected_successes,
            "expected {} successful plugins, got {} (total={}, failures={})",
            expected_successes,
            manager.plugin_count(),
            n,
            n - expected_successes
        );

        // Verify only successful plugins are in the names list
        let loaded_names = manager.plugin_names();
        prop_assert_eq!(
            loaded_names.len(),
            expected_successes,
            "loaded names count mismatch"
        );

        for (i, &ok) in outcomes.iter().enumerate() {
            let name = format!("plugin_{}", i);
            if ok {
                prop_assert!(
                    loaded_names.contains(&name.as_str()),
                    "successful plugin '{}' should be loaded",
                    name
                );
            } else {
                prop_assert!(
                    !loaded_names.contains(&name.as_str()),
                    "failed plugin '{}' should NOT be loaded",
                    name
                );
            }
        }

        // Invoking a hook should only call successful plugins
        let mut ctx = HookContext::default();
        let result = manager.invoke_hook(PluginHook::BeforeRun, &mut ctx);
        prop_assert_eq!(result, HookResult::Continue);
    }
}
