//! Property-based tests for configuration hot-reload validation.
//!
//! Feature: gateway-production-maturity, Property 14: Config hot-reload rejects invalid configs
//! **Validates: Requirements R7.2, R7.3**

use adk_gateway::config::*;
use adk_gateway::config_watcher::{validate_config, ConfigDiff};
use proptest::prelude::*;

fn arb_non_empty_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,20}"
}

// Feature: gateway-production-maturity, Property 14: Config hot-reload rejects invalid configs
// **Validates: Requirements R7.2, R7.3**
proptest! {
    /// For any invalid configuration file change detected by ConfigWatcher,
    /// the gateway should log a warning with the validation error and continue
    /// operating with the previous valid configuration.
    /// The gateway state should be unchanged after an invalid reload attempt.
    #[test]
    fn hot_reload_rejects_invalid_configs(
        port in 1..65535u16,
    ) {
        // Valid config should pass validation
        let mut valid_config = GatewayConfig::default();
        valid_config.gateway.port = port;
        prop_assert!(validate_config(&valid_config).is_ok());

        // Zero port should be rejected
        let mut invalid_config = valid_config.clone();
        invalid_config.gateway.port = 0;
        prop_assert!(validate_config(&invalid_config).is_err());
    }

    /// Duplicate cron job IDs should be rejected.
    #[test]
    fn hot_reload_rejects_duplicate_cron_ids(
        id in arb_non_empty_string(),
    ) {
        let mut config = GatewayConfig::default();
        config.cron.jobs = vec![
            CronJob {
                id: id.clone(),
                schedule: "* * * * *".into(),
                message: "a".into(),
                deliver_to: None,
            },
            CronJob {
                id: id.clone(),
                schedule: "0 * * * *".into(),
                message: "b".into(),
                deliver_to: None,
            },
        ];
        prop_assert!(validate_config(&config).is_err());
    }

    /// ConfigDiff should detect changes between configs.
    #[test]
    fn config_diff_detects_changes(
        job_id in arb_non_empty_string(),
    ) {
        let old = GatewayConfig::default();
        let mut new = old.clone();
        new.cron.jobs.push(CronJob {
            id: job_id,
            schedule: "* * * * *".into(),
            message: "test".into(),
            deliver_to: None,
        });

        let diff = ConfigDiff::compute(&old, &new);
        prop_assert!(diff.has_changes());
        prop_assert!(diff.cron_changed);
    }

    /// Identical configs should produce no diff.
    #[test]
    fn config_diff_no_changes_for_identical(
        port in 1..65535u16,
    ) {
        let mut config = GatewayConfig::default();
        config.gateway.port = port;
        let diff = ConfigDiff::compute(&config, &config);
        prop_assert!(!diff.has_changes());
    }
}

// ── Property 15: ConfigDiff detects all field changes ──────────────
// Feature: gateway-full-wiring, Property 15: ConfigDiff detects all field changes
// **Validates: Requirements 9.2**

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 15: For any two GatewayConfig instances that differ in at least
    /// one tracked field, ConfigDiff::compute should return has_changes() == true
    /// and the corresponding *_changed flag == true.
    #[test]
    fn config_diff_detects_all_field_changes(
        field_idx in 0usize..11,
    ) {
        let old = GatewayConfig::default();
        let mut new = old.clone();

        // Mutate exactly one tracked field
        match field_idx {
            0 => {
                // channels_changed
                new.channels.telegram = Some(TelegramConfig::default());
            }
            1 => {
                // routing_changed
                new.routing.bindings.push(RoutingBinding {
                    agent_id: "test_agent".into(),
                    match_rule: RoutingMatch { channel: None, account_id: None, peer: None },
                });
            }
            2 => {
                // session_changed
                new.session.dm_scope = "per-account-channel-peer".into();
            }
            3 => {
                // cron_changed
                new.cron.jobs.push(CronJob {
                    id: "test_job".into(),
                    schedule: "* * * * *".into(),
                    message: "hello".into(),
                    deliver_to: None,
                });
            }
            4 => {
                // plugins_changed
                new.plugins.push(PluginConfig {
                    name: "test_plugin".into(),
                    enabled: true,
                    config: serde_json::Value::Null,
                });
            }
            5 => {
                // auth_changed
                new.auth = Some(AuthConfig {
                    mode: AuthMode::Token,
                    token: Some("test".into()),
                    password: None,
                    roles: vec![],
                    user_mappings: vec![],
                    channel_overrides: std::collections::HashMap::new(),
                    audit: None,
                    sso: None,
                });
            }
            6 => {
                // rag_changed
                new.rag = Some(RagConfig {
                    vector_store: VectorStoreBackend::InMemory,
                    connection_string: None,
                    embedding: EmbeddingConfig { provider: "gemini".into(), model: None },
                    chunking: ChunkingStrategy::default(),
                    chunk_size: None,
                    chunk_overlap: None,
                    watch_dirs: vec![],
                    ingest_webhook: None,
                });
            }
            7 => {
                // memory_changed
                new.memory = Some(MemoryConfig {
                    backend: MemoryBackend::InMemory,
                    connection_string: None,
                    embedding: EmbeddingConfig { provider: "gemini".into(), model: None },
                    max_observations: 50,
                    summary_observations: 10,
                    protocol_path: std::path::PathBuf::from("memory.md"),
                    context_dir: std::path::PathBuf::from("context"),
                });
            }
            8 => {
                // telemetry_changed
                new.telemetry.log_format = LogFormat::Json;
            }
            9 => {
                // conventions_changed
                new.conventions.enabled = !old.conventions.enabled;
            }
            _ => {
                // hooks_changed
                new.hooks.enabled = !old.hooks.enabled;
            }
        }

        let diff = ConfigDiff::compute(&old, &new);

        // has_changes() must be true
        prop_assert!(
            diff.has_changes(),
            "ConfigDiff should detect changes for field_idx={}", field_idx
        );

        // The specific *_changed flag must be true
        let flag = match field_idx {
            0 => diff.channels_changed,
            1 => diff.routing_changed,
            2 => diff.session_changed,
            3 => diff.cron_changed,
            4 => diff.plugins_changed,
            5 => diff.auth_changed,
            6 => diff.rag_changed,
            7 => diff.memory_changed,
            8 => diff.telemetry_changed,
            9 => diff.conventions_changed,
            _ => diff.hooks_changed,
        };
        prop_assert!(
            flag,
            "specific *_changed flag should be true for field_idx={}", field_idx
        );
    }
}
