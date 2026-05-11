//! Property-based tests for configuration types.
//!
//! Feature: gateway-production-maturity, Property 27: Configuration round-trip
//! Validates: Requirements 15.4

use adk_gateway::config::*;
use proptest::prelude::*;
use std::path::PathBuf;

// ── Leaf strategies ────────────────────────────────────────────────

fn arb_non_empty_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,20}"
}

fn arb_opt_string() -> impl Strategy<Value = Option<String>> {
    prop::option::of(arb_non_empty_string())
}

fn arb_pathbuf() -> impl Strategy<Value = PathBuf> {
    arb_non_empty_string().prop_map(PathBuf::from)
}

fn arb_json_value() -> impl Strategy<Value = serde_json::Value> {
    // Keep JSON values simple to avoid round-trip issues with arbitrary floats
    prop_oneof![
        Just(serde_json::Value::Null),
        any::<bool>().prop_map(serde_json::Value::Bool),
        any::<i32>().prop_map(|n| serde_json::Value::Number(n.into())),
        arb_non_empty_string().prop_map(serde_json::Value::String),
    ]
}

// JSON value that is never null — used inside Option<Value> fields
// because Some(Value::Null) serializes to JSON `null` which deserializes as None.
fn arb_json_value_non_null() -> impl Strategy<Value = serde_json::Value> {
    prop_oneof![
        any::<bool>().prop_map(serde_json::Value::Bool),
        any::<i32>().prop_map(|n| serde_json::Value::Number(n.into())),
        arb_non_empty_string().prop_map(serde_json::Value::String),
    ]
}

// ── Enum strategies ────────────────────────────────────────────────

fn arb_dm_policy() -> impl Strategy<Value = DmPolicy> {
    prop_oneof![
        Just(DmPolicy::Pairing),
        Just(DmPolicy::Allowlist),
        Just(DmPolicy::Open),
        Just(DmPolicy::Disabled),
    ]
}

fn arb_auth_mode() -> impl Strategy<Value = AuthMode> {
    prop_oneof![
        Just(AuthMode::Token),
        Just(AuthMode::Password),
        Just(AuthMode::None),
    ]
}

fn arb_bind_mode() -> impl Strategy<Value = BindMode> {
    prop_oneof![
        Just(BindMode::Loopback),
        Just(BindMode::Lan),
        Just(BindMode::Tailnet),
        arb_non_empty_string().prop_map(BindMode::Custom),
    ]
}

fn arb_audit_sink_type() -> impl Strategy<Value = AuditSinkType> {
    prop_oneof![Just(AuditSinkType::File), Just(AuditSinkType::Custom),]
}

fn arb_memory_backend() -> impl Strategy<Value = MemoryBackend> {
    prop_oneof![
        Just(MemoryBackend::InMemory),
        Just(MemoryBackend::Sqlite),
        Just(MemoryBackend::Postgres),
        Just(MemoryBackend::Neo4j),
        Just(MemoryBackend::SqlRite),
    ]
}

fn arb_vector_store_backend() -> impl Strategy<Value = VectorStoreBackend> {
    prop_oneof![
        Just(VectorStoreBackend::InMemory),
        Just(VectorStoreBackend::Qdrant),
        Just(VectorStoreBackend::LanceDb),
        Just(VectorStoreBackend::PgVector),
        Just(VectorStoreBackend::SurrealDb),
        Just(VectorStoreBackend::SqlRite),
    ]
}

fn arb_chunking_strategy() -> impl Strategy<Value = ChunkingStrategy> {
    prop_oneof![
        Just(ChunkingStrategy::FixedSize),
        Just(ChunkingStrategy::Markdown),
        Just(ChunkingStrategy::Recursive),
    ]
}

fn arb_log_format() -> impl Strategy<Value = LogFormat> {
    prop_oneof![Just(LogFormat::Text), Just(LogFormat::Json),]
}

#[allow(dead_code)]
fn arb_graph_node_type() -> impl Strategy<Value = GraphNodeType> {
    prop_oneof![
        Just(GraphNodeType::Agent),
        Just(GraphNodeType::Action),
        Just(GraphNodeType::Tool),
    ]
}

#[allow(dead_code)]
fn arb_reducer_type() -> impl Strategy<Value = ReducerType> {
    prop_oneof![
        Just(ReducerType::Overwrite),
        Just(ReducerType::Append),
        Just(ReducerType::Sum),
        arb_non_empty_string().prop_map(ReducerType::Custom),
    ]
}

#[allow(dead_code)]
fn arb_graph_stream_mode() -> impl Strategy<Value = GraphStreamMode> {
    prop_oneof![
        Just(GraphStreamMode::Values),
        Just(GraphStreamMode::Updates),
        Just(GraphStreamMode::Messages),
        Just(GraphStreamMode::Debug),
    ]
}

#[allow(dead_code)]
fn arb_action_type() -> impl Strategy<Value = ActionType> {
    prop_oneof![
        Just(ActionType::Http),
        Just(ActionType::Database),
        Just(ActionType::File),
        Just(ActionType::Transform),
        Just(ActionType::Set),
        Just(ActionType::Switch),
        Just(ActionType::Loop),
        Just(ActionType::Merge),
        Just(ActionType::Wait),
        Just(ActionType::Code),
        Just(ActionType::Email),
        Just(ActionType::Notification),
        Just(ActionType::Rss),
        Just(ActionType::Trigger),
    ]
}

// ── Struct strategies ──────────────────────────────────────────────

fn arb_model_spec() -> impl Strategy<Value = ModelSpec> {
    prop_oneof![
        arb_non_empty_string().prop_map(ModelSpec::Simple),
        (
            arb_non_empty_string(),
            prop::collection::vec(arb_non_empty_string(), 0..3)
        )
            .prop_map(|(primary, fallbacks)| ModelSpec::WithFallbacks { primary, fallbacks }),
    ]
}

fn arb_opt_vec_string() -> impl Strategy<Value = Option<Vec<String>>> {
    prop::option::of(prop::collection::vec(arb_non_empty_string(), 1..4))
}

fn arb_category_config() -> impl Strategy<Value = CategoryConfig> {
    (
        arb_model_spec(),
        arb_opt_vec_string(),
        arb_opt_vec_string(),
        arb_opt_vec_string(),
        arb_opt_vec_string(),
        arb_opt_vec_string(),
        arb_opt_vec_string(),
        arb_opt_vec_string(),
        arb_opt_vec_string(),
        arb_opt_vec_string(),
    )
        .prop_map(
            |(
                primary,
                vision,
                omni,
                image_generation,
                tts,
                stt,
                code,
                embedding,
                search,
                music,
            )| {
                CategoryConfig {
                    primary,
                    vision,
                    omni,
                    image_generation,
                    tts,
                    stt,
                    code,
                    embedding,
                    search,
                    music,
                }
            },
        )
}

fn arb_agent_config() -> impl Strategy<Value = AgentConfig> {
    arb_category_config().prop_map(|model| AgentConfig { model })
}

fn arb_agent_defaults() -> impl Strategy<Value = AgentDefaults> {
    (arb_non_empty_string(), arb_opt_string(), arb_opt_string()).prop_map(
        |(workspace, model, thinking_level)| AgentDefaults {
            workspace,
            model,
            thinking_level,
        },
    )
}

fn arb_agent_entry() -> impl Strategy<Value = AgentEntry> {
    (
        arb_non_empty_string(),
        any::<bool>(),
        arb_opt_string(),
        arb_opt_string(),
        prop::collection::vec(arb_non_empty_string(), 0..3),
    )
        .prop_map(|(id, default, workspace, model, skills)| AgentEntry {
            id,
            default,
            workspace,
            model,
            skills,
            browser: None,
            tools: vec![],
        })
}

fn arb_agents_config() -> impl Strategy<Value = AgentsConfig> {
    (
        arb_agent_defaults(),
        prop::collection::vec(arb_agent_entry(), 0..3),
    )
        .prop_map(|(defaults, list)| AgentsConfig { defaults, list })
}

fn arb_role_config() -> impl Strategy<Value = RoleConfig> {
    (
        arb_non_empty_string(),
        prop::collection::vec(arb_non_empty_string(), 0..3),
        prop::collection::vec(arb_non_empty_string(), 0..3),
    )
        .prop_map(|(name, permissions, scopes)| RoleConfig {
            name,
            permissions,
            scopes,
        })
}

fn arb_user_role_mapping() -> impl Strategy<Value = UserRoleMapping> {
    (arb_non_empty_string(), arb_non_empty_string())
        .prop_map(|(user_id, role)| UserRoleMapping { user_id, role })
}

fn arb_audit_config() -> impl Strategy<Value = AuditConfig> {
    (
        any::<bool>(),
        arb_audit_sink_type(),
        prop::option::of(arb_pathbuf()),
    )
        .prop_map(|(enabled, sink, path)| AuditConfig {
            enabled,
            sink,
            path,
        })
}

fn arb_sso_config() -> impl Strategy<Value = SsoConfig> {
    (
        arb_non_empty_string(),
        arb_non_empty_string(),
        arb_non_empty_string(),
    )
        .prop_map(|(jwks_url, issuer, audience)| SsoConfig {
            jwks_url,
            issuer,
            audience,
        })
}

fn arb_channel_auth_override() -> impl Strategy<Value = ChannelAuthOverride> {
    (
        prop::option::of(arb_dm_policy()),
        prop::collection::vec(arb_role_config(), 0..2),
    )
        .prop_map(|(dm_policy, roles)| ChannelAuthOverride { dm_policy, roles })
}

fn arb_auth_config() -> impl Strategy<Value = AuthConfig> {
    (
        arb_auth_mode(),
        arb_opt_string(),
        arb_opt_string(),
        prop::collection::vec(arb_role_config(), 0..2),
        prop::collection::vec(arb_user_role_mapping(), 0..2),
        prop::collection::hash_map(arb_non_empty_string(), arb_channel_auth_override(), 0..2),
        prop::option::of(arb_audit_config()),
        prop::option::of(arb_sso_config()),
    )
        .prop_map(
            |(mode, token, password, roles, user_mappings, channel_overrides, audit, sso)| {
                AuthConfig {
                    mode,
                    token,
                    password,
                    roles,
                    user_mappings,
                    channel_overrides,
                    audit,
                    sso,
                }
            },
        )
}

fn arb_server_settings() -> impl Strategy<Value = ServerSettings> {
    (
        any::<u16>(),
        arb_bind_mode(),
        prop::option::of(arb_auth_config()),
    )
        .prop_map(|(port, bind, auth)| ServerSettings {
            port,
            bind,
            auth,
            drain_timeout_secs: 30,
        })
}

fn arb_group_rule() -> impl Strategy<Value = GroupRule> {
    prop::option::of(any::<bool>()).prop_map(|require_mention| GroupRule { require_mention })
}

fn arb_groups_config() -> impl Strategy<Value = GroupsConfig> {
    prop::collection::hash_map(arb_non_empty_string(), arb_group_rule(), 0..3)
        .prop_map(|rules| GroupsConfig { rules })
}

fn arb_telegram_config() -> impl Strategy<Value = TelegramConfig> {
    (
        any::<bool>(),
        arb_non_empty_string(),
        arb_dm_policy(),
        prop::collection::vec(arb_non_empty_string(), 0..3),
        arb_groups_config(),
        arb_opt_string(),
    )
        .prop_map(
            |(enabled, bot_token, dm_policy, allow_from, groups, stream_mode)| TelegramConfig {
                enabled,
                bot_token,
                dm_policy,
                allow_from,
                groups,
                stream_mode,
                account_id: "default".into(),
            },
        )
}

fn arb_slack_config() -> impl Strategy<Value = SlackConfig> {
    (
        any::<bool>(),
        arb_non_empty_string(),
        arb_non_empty_string(),
        arb_dm_policy(),
        prop::collection::vec(arb_non_empty_string(), 0..3),
    )
        .prop_map(
            |(enabled, bot_token, app_token, dm_policy, allow_from)| SlackConfig {
                enabled,
                bot_token,
                app_token,
                dm_policy,
                allow_from,
                account_id: "default".into(),
            },
        )
}

fn arb_channels_config() -> impl Strategy<Value = ChannelsConfig> {
    (
        prop::option::of(arb_telegram_config()),
        prop::option::of(arb_slack_config()),
    )
        .prop_map(|(telegram, slack)| ChannelsConfig {
            telegram,
            slack,
            telegram_accounts: vec![],
            slack_accounts: vec![],
            // Phase 2 channels: use None to keep round-trip clean
            whatsapp: None,
            discord: None,
            matrix: None,
            signal: None,
            imessage: None,
        })
}

fn arb_routing_match() -> impl Strategy<Value = RoutingMatch> {
    (
        arb_opt_string(),
        arb_opt_string(),
        prop::option::of(arb_json_value_non_null()),
    )
        .prop_map(|(channel, account_id, peer)| RoutingMatch {
            channel,
            account_id,
            peer,
        })
}

fn arb_routing_binding() -> impl Strategy<Value = RoutingBinding> {
    (arb_non_empty_string(), arb_routing_match()).prop_map(|(agent_id, match_rule)| {
        RoutingBinding {
            agent_id,
            match_rule,
        }
    })
}

fn arb_routing_config() -> impl Strategy<Value = RoutingConfig> {
    prop::collection::vec(arb_routing_binding(), 0..3)
        .prop_map(|bindings| RoutingConfig { bindings })
}

fn arb_session_reset_config() -> impl Strategy<Value = SessionResetConfig> {
    (
        arb_non_empty_string(),
        prop::option::of(0..24u8),
        prop::option::of(1..1440u64),
    )
        .prop_map(|(mode, at_hour, idle_minutes)| SessionResetConfig {
            mode,
            at_hour,
            idle_minutes,
        })
}

fn arb_session_backend_type() -> impl Strategy<Value = SessionBackendType> {
    prop_oneof![
        Just(SessionBackendType::InMemory),
        Just(SessionBackendType::Sqlite),
        Just(SessionBackendType::Postgres),
        Just(SessionBackendType::Redis),
        Just(SessionBackendType::Firestore),
    ]
}

fn arb_session_config() -> impl Strategy<Value = SessionConfig> {
    (
        arb_non_empty_string(),
        arb_session_reset_config(),
        arb_session_backend_type(),
        arb_opt_string(),
    )
        .prop_map(
            |(dm_scope, reset, backend, connection_string)| SessionConfig {
                dm_scope,
                reset,
                backend,
                connection_string,
            },
        )
}

fn arb_hooks_config() -> impl Strategy<Value = HooksConfig> {
    (any::<bool>(), arb_opt_string(), arb_opt_string()).prop_map(|(enabled, token, path)| {
        HooksConfig {
            enabled,
            token,
            path,
        }
    })
}

fn arb_cron_delivery() -> impl Strategy<Value = CronDelivery> {
    (arb_non_empty_string(), arb_non_empty_string())
        .prop_map(|(channel, target)| CronDelivery { channel, target })
}

fn arb_cron_job() -> impl Strategy<Value = CronJob> {
    (
        arb_non_empty_string(),
        arb_non_empty_string(),
        arb_non_empty_string(),
        prop::option::of(arb_cron_delivery()),
    )
        .prop_map(|(id, schedule, message, deliver_to)| CronJob {
            id,
            schedule,
            message,
            deliver_to,
        })
}

fn arb_cron_config() -> impl Strategy<Value = CronConfig> {
    prop::collection::vec(arb_cron_job(), 0..3).prop_map(|jobs| CronConfig { jobs })
}

fn arb_embedding_config() -> impl Strategy<Value = EmbeddingConfig> {
    (arb_non_empty_string(), arb_opt_string())
        .prop_map(|(provider, model)| EmbeddingConfig { provider, model })
}

fn arb_memory_config() -> impl Strategy<Value = MemoryConfig> {
    (
        arb_memory_backend(),
        arb_opt_string(),
        arb_embedding_config(),
    )
        .prop_map(|(backend, connection_string, embedding)| MemoryConfig {
            backend,
            connection_string,
            embedding,
            max_observations: 50,
            summary_observations: 10,
            protocol_path: std::path::PathBuf::from("memory.md"),
            context_dir: std::path::PathBuf::from("context"),
        })
}

fn arb_rag_config() -> impl Strategy<Value = RagConfig> {
    (
        arb_vector_store_backend(),
        arb_opt_string(),
        arb_embedding_config(),
        arb_chunking_strategy(),
        prop::option::of(1..10000usize),
        prop::option::of(0..1000usize),
        prop::collection::vec(arb_pathbuf(), 0..2),
        prop::option::of(any::<bool>()),
    )
        .prop_map(
            |(
                vector_store,
                connection_string,
                embedding,
                chunking,
                chunk_size,
                chunk_overlap,
                watch_dirs,
                ingest_webhook,
            )| {
                RagConfig {
                    vector_store,
                    connection_string,
                    embedding,
                    chunking,
                    chunk_size,
                    chunk_overlap,
                    watch_dirs,
                    ingest_webhook,
                }
            },
        )
}

fn arb_plugin_config() -> impl Strategy<Value = PluginConfig> {
    (arb_non_empty_string(), any::<bool>(), arb_json_value()).prop_map(|(name, enabled, config)| {
        PluginConfig {
            name,
            enabled,
            config,
        }
    })
}

fn arb_convention_config() -> impl Strategy<Value = ConventionConfig> {
    (
        any::<bool>(),
        prop::collection::vec(arb_non_empty_string(), 0..3),
        prop::option::of(arb_pathbuf()),
    )
        .prop_map(
            |(enabled, extra_patterns, workspace_dir)| ConventionConfig {
                enabled,
                extra_patterns,
                workspace_dir,
            },
        )
}

fn arb_telemetry_config() -> impl Strategy<Value = TelemetryConfig> {
    (arb_log_format(), arb_opt_string(), any::<bool>()).prop_map(
        |(log_format, otel_endpoint, metrics_enabled)| TelemetryConfig {
            log_format,
            otel_endpoint,
            metrics_enabled,
        },
    )
}

// ── Top-level GatewayConfig strategy ───────────────────────────────

fn arb_gateway_config() -> impl Strategy<Value = GatewayConfig> {
    // proptest supports tuples up to 12 elements, so we split into two groups
    let group1 = (
        arb_agent_config(),
        arb_agents_config(),
        arb_server_settings(),
        arb_channels_config(),
        arb_routing_config(),
        arb_session_config(),
        arb_hooks_config(),
        arb_cron_config(),
        prop::option::of(arb_memory_config()),
        prop::option::of(arb_rag_config()),
    );
    let group2 = (
        prop::option::of(arb_auth_config()),
        prop::collection::vec(arb_plugin_config(), 0..3),
        arb_convention_config(),
        arb_telemetry_config(),
    );
    (group1, group2).prop_map(
        |(
            (agent, agents, gateway, channels, routing, session, hooks, cron, memory, rag),
            (auth, plugins, conventions, telemetry),
        )| {
            GatewayConfig {
                agent,
                agents,
                gateway,
                channels,
                routing,
                session,
                hooks,
                cron,
                memory,
                rag,
                auth,
                plugins,
                conventions,
                telemetry,
                graph_workflow: None,
                mcp_servers: vec![],
                awp: adk_gateway::awp::AwpConfig::default(),
            }
        },
    )
}

// ── Property test ──────────────────────────────────────────────────

// Feature: gateway-production-maturity, Property 27: Configuration round-trip
// **Validates: Requirements 15.4**
proptest! {
    #[test]
    fn config_round_trip(config in arb_gateway_config()) {
        let json = serde_json::to_string(&config).expect("serialization should succeed");
        let parsed: GatewayConfig = serde_json::from_str(&json).expect("deserialization should succeed");
        prop_assert_eq!(config, parsed);
    }
}

// ── Property 28: Environment variable expansion ────────────────────
// Feature: gateway-production-maturity, Property 28: Environment variable expansion
// **Validates: Requirements R15.5, R15.6**

/// Strategy for valid environment variable names: starts with letter or underscore,
/// followed by alphanumeric or underscore characters.
fn arb_env_var_name() -> impl Strategy<Value = String> {
    "[A-Za-z_][A-Za-z0-9_]{0,19}"
}

/// Strategy for arbitrary env var values (printable ASCII, no `${` to avoid nested expansion).
fn arb_env_var_value() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9 _./:@#!%^&*()-]{0,50}"
}

proptest! {
    /// R15.5: When an env var IS set, `${VAR_NAME}` is replaced with its value.
    #[test]
    fn env_var_expansion_replaces_set_vars(
        name in arb_env_var_name(),
        value in arb_env_var_value(),
    ) {
        // Set the env var for this test
        let prefixed_name = format!("PROPTEST_{}", name);
        unsafe { std::env::set_var(&prefixed_name, &value) };

        let input = format!("prefix_${{{}}}__suffix", prefixed_name);
        let result = adk_gateway::config::expand_env_vars(&input);

        let expected = format!("prefix_{}__suffix", value);
        prop_assert_eq!(result, expected);

        // Clean up
        unsafe { std::env::remove_var(&prefixed_name) };
    }

    /// R15.6: When an env var is NOT set, `${VAR_NAME}` pattern remains unchanged.
    #[test]
    fn env_var_expansion_preserves_unset_vars(
        name in arb_env_var_name(),
    ) {
        let prefixed_name = format!("PROPTEST_UNSET_{}", name);
        // Ensure the var is not set
        unsafe { std::env::remove_var(&prefixed_name) };

        let pattern = format!("${{{}}}", prefixed_name);
        let input = format!("before_{}__after", pattern);
        let result = adk_gateway::config::expand_env_vars(&input);

        // Pattern should remain unchanged
        prop_assert_eq!(result, input);
    }
}

// ── Property 13: Secret redaction removes all known patterns ───────
// Feature: gateway-full-wiring, Property 13: Secret redaction removes all known patterns
// **Validates: Requirements 7.2**

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 13: For any string containing a Telegram bot token, Bearer token,
    /// Slack token, or JWT token, SecretRedactor::redact should return a string
    /// that does not contain the original secret.
    #[test]
    fn secret_redaction_removes_known_patterns(
        variant in 0u8..4,
        prefix in "[a-zA-Z ]{0,20}",
        suffix in "[a-zA-Z ]{0,20}",
    ) {
        use adk_gateway::telemetry::SecretRedactor;

        let redactor = SecretRedactor::new();

        // Generate a secret based on variant
        let (input, secret_substring) = match variant {
            0 => {
                // Telegram bot token: digits:alphanumeric
                let token = "1234567890:ABCdefGHIjklMNOpqrSTUvwxYZ1234567";
                (format!("{prefix} {token} {suffix}"), token.to_string())
            }
            1 => {
                // Bearer token
                let bearer = "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
                (format!("{prefix} {bearer} {suffix}"), bearer.to_string())
            }
            2 => {
                // Slack token
                let slack = "xoxb-1234567890-abcdefghij";
                (format!("{prefix} {slack} {suffix}"), slack.to_string())
            }
            _ => {
                // JWT token
                let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ0ZXN0In0.abc123def456ghi789";
                (format!("{prefix} {jwt} {suffix}"), jwt.to_string())
            }
        };

        let result = redactor.redact(&input);

        // The redacted output should not contain the original secret
        prop_assert!(
            !result.contains(&secret_substring),
            "redacted output should not contain the secret '{}', got: '{}'",
            secret_substring, result
        );

        // The redacted output should contain the replacement marker
        prop_assert!(
            result.contains("***REDACTED***"),
            "redacted output should contain replacement marker, got: '{}'",
            result
        );
    }
}

// ── Property 14: Config redaction removes sensitive fields ─────────
// Feature: gateway-full-wiring, Property 14: Config redaction removes sensitive fields
// **Validates: Requirements 7.3, 19.2**

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 14: For any JSON object with keys from SENSITIVE_FIELDS with
    /// non-empty string values, redact_config should replace those values with
    /// "***" while preserving other values.
    #[test]
    fn config_redaction_removes_sensitive_fields(
        sensitive_key_idx in 0usize..10,
        secret_value in "[a-zA-Z0-9]{1,30}",
        safe_key in "[a-zA-Z]{1,10}",
        safe_value in "[a-zA-Z0-9]{1,20}",
    ) {
        use adk_gateway::telemetry::redact_config;

        let sensitive_fields = [
            "bot_token", "botToken", "app_token", "appToken", "token",
            "password", "secret", "api_key", "apiKey", "connection_string",
        ];
        let sensitive_key = sensitive_fields[sensitive_key_idx % sensitive_fields.len()];

        // Build a JSON object with one sensitive and one safe key
        let safe_key_clone = safe_key.clone();
        let config = serde_json::json!({
            sensitive_key: secret_value.clone(),
            safe_key: safe_value.clone(),
        });

        let redacted = redact_config(&config);

        // Sensitive field should be replaced with "***"
        prop_assert_eq!(
            &redacted[sensitive_key], &serde_json::json!("***"),
            "sensitive field '{}' should be redacted to '***', got: {:?}",
            sensitive_key, redacted[sensitive_key]
        );

        // Safe field should be preserved
        prop_assert_eq!(
            &redacted[safe_key_clone.as_str()], &serde_json::json!(safe_value),
            "safe field '{}' should be preserved, got: {:?}",
            safe_key_clone, redacted[safe_key_clone.as_str()]
        );
    }
}
