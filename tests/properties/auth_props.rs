//! Property-based tests for authentication and access control.
//!
//! Feature: gateway-production-maturity
//! - Property 43: DM policy maps to correct auth roles
//!   **Validates: Requirements R26.2**
//! - Property 44: Tool access requires correct role and scope
//!   **Validates: Requirements R26.4, R26.5, R26.11**
//! - Property 45: Audit events record all access decisions
//!   **Validates: Requirements R26.9, R26.10**
//! - Property 46: Scope resolver chain order
//!   **Validates: Requirements R26.6**
//! - Property 47: Per-channel access control overrides
//!   **Validates: Requirements R26.7**

use adk_gateway::access_control::{
    AccessControlBridge, AuthDecision, ChainedScopeResolver, ScopeResolver, StaticScopeResolver,
    ToolAccessCheck, ToolAccessDecision,
};
use adk_gateway::audit::{AuditEvent, AuditEventType, AuditOutcome};
use adk_gateway::channel::{ChannelType, InboundMessage, MessageSource};
use adk_gateway::config::*;
use proptest::prelude::*;
use std::collections::{HashMap, HashSet};

// ── Strategies ─────────────────────────────────────────────────────

fn arb_non_empty_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_]{1,20}"
}

fn arb_dm_policy() -> impl Strategy<Value = DmPolicy> {
    prop_oneof![
        Just(DmPolicy::Open),
        Just(DmPolicy::Disabled),
        Just(DmPolicy::Allowlist),
        Just(DmPolicy::Pairing),
    ]
}

fn arb_channel_type() -> impl Strategy<Value = ChannelType> {
    prop_oneof![Just(ChannelType::Telegram), Just(ChannelType::Slack),]
}

fn arb_audit_event_type() -> impl Strategy<Value = AuditEventType> {
    prop_oneof![
        Just(AuditEventType::ToolAccess),
        Just(AuditEventType::AgentAccess),
        Just(AuditEventType::Login),
        Just(AuditEventType::PairingAttempt),
        Just(AuditEventType::PermissionCheck),
    ]
}

fn arb_audit_outcome() -> impl Strategy<Value = AuditOutcome> {
    prop_oneof![
        Just(AuditOutcome::Allowed),
        Just(AuditOutcome::Denied),
        Just(AuditOutcome::Error),
    ]
}

fn arb_audit_event() -> impl Strategy<Value = AuditEvent> {
    (
        arb_non_empty_string(),
        prop::option::of(arb_non_empty_string()),
        prop::option::of(arb_channel_type()),
        arb_audit_event_type(),
        arb_non_empty_string(),
        arb_audit_outcome(),
        prop::option::of(arb_non_empty_string()),
    )
        .prop_map(
            |(user_id, session_id, channel_type, event_type, resource, outcome, details)| {
                AuditEvent {
                    timestamp: chrono::Utc::now(),
                    user_id,
                    session_id,
                    channel_type,
                    event_type,
                    resource,
                    outcome,
                    details,
                }
            },
        )
}

/// Helper: build an InboundMessage for a given channel type and sender.
fn make_msg(channel_type: ChannelType, sender_id: &str) -> InboundMessage {
    InboundMessage {
        channel_type,
        account_id: "default".into(),
        sender_id: sender_id.into(),
        sender_name: None,
        text: "hello".into(),
        is_group: false,
        group_id: None,
        is_mention: false,
        platform_message_id: "m1".into(),
        attachments: vec![],
        metadata: Default::default(),
        source: MessageSource::default(),
        timestamp: chrono::Utc::now(),
    }
}

// ── Property 43: DM policy maps to correct auth roles ──────────────
// Feature: gateway-production-maturity, Property 43: DM policy maps to correct auth roles
// **Validates: Requirements R26.2**
proptest! {
    /// Property 43: For any DM policy and sender, the AccessControlBridge
    /// produces the correct AuthDecision:
    /// - Open → Allowed
    /// - Disabled → Denied
    /// - Allowlist with sender in list → Allowed, not in list → Denied
    /// - Pairing with unpaired user → RequiresPairing, paired → Allowed
    #[test]
    fn dm_policy_maps_to_correct_auth_decision(
        policy in arb_dm_policy(),
        sender_id in arb_non_empty_string(),
        is_paired in any::<bool>(),
        in_allow_list in any::<bool>(),
    ) {
        let allow_from = if in_allow_list {
            vec![sender_id.clone()]
        } else {
            vec!["other_user_not_matching".to_string()]
        };

        let mut cfg = GatewayConfig::default();
        cfg.channels.telegram = Some(TelegramConfig {
            enabled: true,
            bot_token: "tok".into(),
            dm_policy: policy.clone(),
            allow_from,
            groups: GroupsConfig::default(),
            stream_mode: None,
            account_id: "default".into(),
        });

        let mut bridge = AccessControlBridge::new(&cfg);

        if is_paired {
            let canonical = format!("telegram:{}", sender_id);
            bridge.mark_paired(&canonical);
        }

        let msg = make_msg(ChannelType::Telegram, &sender_id);
        let decision = bridge.check_message_access(&msg);

        match policy {
            DmPolicy::Open => {
                prop_assert_eq!(decision, AuthDecision::Allowed,
                    "Open policy should always allow");
            }
            DmPolicy::Disabled => {
                prop_assert!(matches!(decision, AuthDecision::Denied { .. }),
                    "Disabled policy should always deny");
            }
            DmPolicy::Allowlist => {
                if in_allow_list {
                    prop_assert_eq!(decision, AuthDecision::Allowed,
                        "Allowlist with sender in list should allow");
                } else {
                    prop_assert!(matches!(decision, AuthDecision::Denied { .. }),
                        "Allowlist without sender in list should deny");
                }
            }
            DmPolicy::Pairing => {
                if is_paired {
                    prop_assert_eq!(decision, AuthDecision::Allowed,
                        "Pairing with paired user should allow");
                } else {
                    prop_assert_eq!(decision, AuthDecision::RequiresPairing,
                        "Pairing with unpaired user should require pairing");
                }
            }
        }
    }
}

// ── Property 44: Tool access requires correct role and scope ───────
// Feature: gateway-production-maturity, Property 44: Tool access requires correct role and scope
// **Validates: Requirements R26.4, R26.5, R26.11**
proptest! {
    /// Property 44: ToolAccessCheck correctly allows/denies based on
    /// whether the user has the required role and all required scopes.
    ///
    /// - If a required role is specified and the user lacks it → Denied
    /// - If the user has the role but is missing any required scope → Denied
    /// - If the user has the role and all scopes → Allowed
    /// - If no role/scopes required → Allowed
    #[test]
    fn tool_access_requires_correct_role_and_scope(
        user_role in arb_non_empty_string(),
        required_role in arb_non_empty_string(),
        has_role in any::<bool>(),
        _user_scope_a in arb_non_empty_string(),
        _user_scope_b in arb_non_empty_string(),
        required_scope in arb_non_empty_string(),
        has_all_scopes in any::<bool>(),
        tool_name in arb_non_empty_string(),
    ) {
        let user_roles: HashSet<String> = if has_role {
            [required_role.clone()].into_iter().collect()
        } else {
            [user_role].into_iter().collect()
        };

        let required_scopes = vec![required_scope.clone()];
        let user_scopes: HashSet<String> = if has_all_scopes {
            [required_scope.clone()].into_iter().collect()
        } else {
            // Give the user some scope that is NOT the required one
            let different = format!("{}_different", required_scope);
            [different].into_iter().collect()
        };

        // Test with role requirement
        let decision = ToolAccessCheck::check_tool_access(
            &user_roles,
            &user_scopes,
            &tool_name,
            Some(&required_role),
            &required_scopes,
        );

        if !has_role {
            prop_assert!(matches!(decision, ToolAccessDecision::Denied { .. }),
                "should deny when user lacks required role");
        } else if !has_all_scopes {
            prop_assert!(matches!(decision, ToolAccessDecision::Denied { .. }),
                "should deny when user lacks required scopes");
        } else {
            prop_assert_eq!(decision, ToolAccessDecision::Allowed,
                "should allow when user has role and all scopes");
        }

        // Test with no requirements → always allowed
        let no_req_decision = ToolAccessCheck::check_tool_access(
            &user_roles,
            &user_scopes,
            &tool_name,
            None,
            &[],
        );
        prop_assert_eq!(no_req_decision, ToolAccessDecision::Allowed,
            "should always allow when no role or scope requirements");
    }
}

// ── Property 45: Audit events record all access decisions ──────────
// Feature: gateway-production-maturity, Property 45: Audit events record all access decisions
// **Validates: Requirements R26.9, R26.10**
proptest! {
    /// Property 45: Any AuditEvent serializes to valid JSON-line format
    /// and can be round-tripped (serialize → deserialize → compare).
    #[test]
    fn audit_events_round_trip_through_json(event in arb_audit_event()) {
        // Serialize to JSON (one line)
        let json = serde_json::to_string(&event)
            .expect("AuditEvent should serialize to JSON");

        // Must be a single line (JSON-line format)
        prop_assert!(!json.contains('\n'),
            "JSON-line format must not contain newlines");

        // Must be valid JSON
        let parsed_value: serde_json::Value = serde_json::from_str(&json)
            .expect("serialized AuditEvent should be valid JSON");

        // Must contain required fields per R26.10
        prop_assert!(parsed_value.get("timestamp").is_some(),
            "AuditEvent JSON must contain timestamp");
        prop_assert!(parsed_value.get("user_id").is_some(),
            "AuditEvent JSON must contain user_id");
        prop_assert!(parsed_value.get("event_type").is_some(),
            "AuditEvent JSON must contain event_type");
        prop_assert!(parsed_value.get("resource").is_some(),
            "AuditEvent JSON must contain resource");
        prop_assert!(parsed_value.get("outcome").is_some(),
            "AuditEvent JSON must contain outcome");

        // Round-trip: deserialize back and compare fields
        let parsed: AuditEvent = serde_json::from_str(&json)
            .expect("AuditEvent should deserialize from JSON");

        prop_assert_eq!(&parsed.user_id, &event.user_id);
        prop_assert_eq!(&parsed.event_type, &event.event_type);
        prop_assert_eq!(&parsed.resource, &event.resource);
        prop_assert_eq!(&parsed.outcome, &event.outcome);
        prop_assert_eq!(&parsed.session_id, &event.session_id);
        prop_assert_eq!(&parsed.channel_type, &event.channel_type);
        prop_assert_eq!(&parsed.details, &event.details);
    }
}

// ── Property 46: Scope resolver chain order ────────────────────────
// Feature: gateway-production-maturity, Property 46: Scope resolver chain order
// **Validates: Requirements R26.6**
proptest! {
    /// Property 46: Multiple StaticScopeResolvers chained together produce
    /// a merged result containing all scopes from all resolvers.
    ///
    /// For N resolvers each with distinct scope sets, the chained result
    /// is the union of all scope sets.
    #[test]
    fn scope_resolver_chain_merges_all_scopes(
        num_resolvers in 1usize..5,
        scopes_per_resolver in 1usize..5,
        canonical_id in arb_non_empty_string(),
    ) {
        let mut resolvers: Vec<Box<dyn ScopeResolver>> = Vec::new();
        let mut expected_scopes: HashSet<String> = HashSet::new();

        for r in 0..num_resolvers {
            let mut user_scopes = HashSet::new();
            for s in 0..scopes_per_resolver {
                let scope = format!("scope_r{}_s{}", r, s);
                user_scopes.insert(scope.clone());
                expected_scopes.insert(scope);
            }
            let mut map = HashMap::new();
            map.insert(canonical_id.clone(), user_scopes);
            resolvers.push(Box::new(StaticScopeResolver::new(map)));
        }

        let chain = ChainedScopeResolver::new(resolvers);
        let result = chain.resolve_scopes(&canonical_id);

        // The merged result must contain ALL scopes from all resolvers
        prop_assert_eq!(result.len(), expected_scopes.len(),
            "merged scopes count should equal total unique scopes");

        for scope in &expected_scopes {
            prop_assert!(result.contains(scope),
                "merged result should contain scope '{}'", scope);
        }
    }
}

// ── Property 47: Per-channel access control overrides ──────────────
// Feature: gateway-production-maturity, Property 47: Per-channel access control overrides
// **Validates: Requirements R26.7**
proptest! {
    /// Property 47: When a per-channel auth override is configured, the
    /// override policy takes precedence over the channel's default DM policy.
    ///
    /// We configure a Telegram channel with one DM policy, then apply an
    /// auth override with a different policy, and verify the override wins.
    #[test]
    fn per_channel_override_takes_precedence(
        channel_policy in arb_dm_policy(),
        override_policy in arb_dm_policy(),
        sender_id in arb_non_empty_string(),
    ) {
        let mut cfg = GatewayConfig::default();
        cfg.channels.telegram = Some(TelegramConfig {
            enabled: true,
            bot_token: "tok".into(),
            dm_policy: channel_policy,
            allow_from: vec![sender_id.clone()],
            groups: GroupsConfig::default(),
            stream_mode: None,
            account_id: "default".into(),
        });

        // Apply per-channel override
        cfg.auth = Some(AuthConfig {
            mode: AuthMode::None,
            token: None,
            password: None,
            roles: vec![],
            user_mappings: vec![],
            channel_overrides: {
                let mut m = HashMap::new();
                m.insert(
                    "telegram".into(),
                    ChannelAuthOverride {
                        dm_policy: Some(override_policy.clone()),
                        roles: vec![],
                    },
                );
                m
            },
            audit: None,
            sso: None,
        });

        let mut bridge = AccessControlBridge::new(&cfg);

        // Mark user as paired so Pairing policy resolves to Allowed
        let canonical = format!("telegram:{}", sender_id);
        bridge.mark_paired(&canonical);

        let msg = make_msg(ChannelType::Telegram, &sender_id);
        let decision = bridge.check_message_access(&msg);

        // The decision should match the OVERRIDE policy, not the channel default
        match override_policy {
            DmPolicy::Open => {
                prop_assert_eq!(decision, AuthDecision::Allowed,
                    "override Open should allow");
            }
            DmPolicy::Disabled => {
                prop_assert!(matches!(decision, AuthDecision::Denied { .. }),
                    "override Disabled should deny");
            }
            DmPolicy::Allowlist => {
                // Override doesn't carry an allow_list, so user won't be in it → Denied
                prop_assert!(matches!(decision, AuthDecision::Denied { .. }),
                    "override Allowlist with empty allow_list should deny");
            }
            DmPolicy::Pairing => {
                // User is marked as paired, so should be Allowed
                prop_assert_eq!(decision, AuthDecision::Allowed,
                    "override Pairing with paired user should allow");
            }
        }
    }
}

// ── Property 4: JWT validation round-trip ──────────────────────────
// Feature: gateway-full-wiring, Property 4: JWT validation round-trip
// **Validates: Requirements 2.3, 2.4**

use adk_gateway::jwt::{JwksCache, JwtValidator};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use std::sync::Arc;
use std::time::Duration;

/// Load the test RSA key pair from fixtures.
fn test_rsa_keys() -> (EncodingKey, jsonwebtoken::DecodingKey) {
    let rsa_private = include_str!("../fixtures/test_rsa_private.pem");
    let rsa_public = include_str!("../fixtures/test_rsa_public.pem");
    let encoding_key = EncodingKey::from_rsa_pem(rsa_private.as_bytes()).unwrap();
    let decoding_key = jsonwebtoken::DecodingKey::from_rsa_pem(rsa_public.as_bytes()).unwrap();
    (encoding_key, decoding_key)
}

/// Strategy for generating arbitrary role lists (1–5 roles).
fn arb_roles() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec("[a-z_]{1,15}", 0..5)
}

/// Strategy for generating arbitrary scope strings (space-delimited).
fn arb_scopes() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec("[a-z]{1,8}:[a-z]{1,8}", 0..5)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 4: For any valid JWT claims (sub, email, roles, scopes,
    /// correct issuer and audience), signing the claims into a token and
    /// then validating with JwtValidator should produce JwtClaims where
    /// sub, roles, and scopes match the original input.
    #[test]
    fn jwt_validation_round_trip(
        sub in "[a-zA-Z0-9_]{1,30}",
        email in "[a-z]{1,10}@[a-z]{1,10}\\.[a-z]{2,4}",
        roles in arb_roles(),
        scopes in arb_scopes(),
    ) {
        let (encoding_key, decoding_key) = test_rsa_keys();
        let kid = "prop-test-key";
        let issuer = "https://auth.example.com";
        let audience = "my-gateway";

        // Build the scope string (space-delimited, as providers encode it)
        let scope_string = scopes.join(" ");

        let now = chrono::Utc::now().timestamp();
        let claims_json = serde_json::json!({
            "sub": sub,
            "email": email,
            "roles": roles,
            "scope": scope_string,
            "iss": issuer,
            "aud": audience,
            "exp": now + 3600,
            "iat": now,
        });

        // Sign the token
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let token = encode(&header, &claims_json, &encoding_key)
            .expect("encoding should succeed");

        // Set up JwtValidator with the test key in cache
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let cache = Arc::new(JwksCache::new(
                "http://unused".into(),
                Duration::from_secs(3600),
                reqwest::Client::new(),
            ));
            let mut keys = HashMap::new();
            keys.insert(kid.to_string(), decoding_key);
            cache.set_keys(keys).await;

            let validator = JwtValidator::new(
                cache,
                issuer.to_string(),
                audience.to_string(),
            );

            let result = validator.validate(&token).await;
            prop_assert!(result.is_ok(), "validation should succeed for valid token, got: {:?}", result.err());

            let jwt_claims = result.unwrap();
            prop_assert_eq!(&jwt_claims.sub, &sub, "sub should match");
            prop_assert_eq!(&jwt_claims.roles, &roles, "roles should match");
            prop_assert_eq!(&jwt_claims.scopes, &scopes, "scopes should match");

            Ok(())
        })?;
    }
}

// ── Property 7: Access control rebuild preserves paired users ──────
// Feature: gateway-full-wiring, Property 7: rebuild preserves paired users
// **Validates: Requirements 4.1**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 7: For any set of paired user IDs and any new GatewayConfig,
    /// calling AccessControlBridge::rebuild should preserve all previously
    /// paired users while updating roles and channel policies.
    #[test]
    fn rebuild_preserves_paired_users(
        paired_ids in prop::collection::hash_set("[a-z]{1,8}:[a-z0-9]{1,10}", 1..10),
        new_dm_policy in arb_dm_policy(),
        new_role_names in prop::collection::vec("[a-z_]{1,12}", 0..3),
    ) {
        // Build initial config with Pairing policy
        let initial_cfg = GatewayConfig::default();
        let mut bridge = AccessControlBridge::new(&initial_cfg);

        // Mark all users as paired
        for id in &paired_ids {
            bridge.mark_paired(id);
        }

        // Verify all are paired before rebuild
        for id in &paired_ids {
            prop_assert!(bridge.is_paired(id),
                "user '{}' should be paired before rebuild", id);
        }

        // Build a new config with different roles and channel policies
        let mut new_cfg = GatewayConfig::default();
        new_cfg.channels.telegram = Some(TelegramConfig {
            enabled: true,
            bot_token: "new_tok".into(),
            dm_policy: new_dm_policy,
            allow_from: vec![],
            groups: GroupsConfig::default(),
            stream_mode: None,
            account_id: "default".into(),
        });

        let new_roles: Vec<RoleConfig> = new_role_names.iter().map(|name| {
            RoleConfig {
                name: name.clone(),
                permissions: vec![format!("perm_{}", name)],
                scopes: vec![format!("scope:{}", name)],
            }
        }).collect();

        if !new_roles.is_empty() {
            new_cfg.auth = Some(AuthConfig {
                mode: AuthMode::None,
                token: None,
                password: None,
                roles: new_roles.clone(),
                user_mappings: vec![],
                channel_overrides: HashMap::new(),
                audit: None,
                sso: None,
            });
        }

        // Rebuild with new config
        bridge.rebuild(&new_cfg);

        // All previously paired users must still be paired
        for id in &paired_ids {
            prop_assert!(bridge.is_paired(id),
                "user '{}' should still be paired after rebuild", id);
        }

        // Paired users should retain the "paired" role
        for id in &paired_ids {
            let roles = bridge.user_role_names(id);
            prop_assert!(roles.contains("paired"),
                "user '{}' should have 'paired' role after rebuild", id);
        }

        // New roles from config should be available
        for role_cfg in &new_roles {
            let role = bridge.get_role(&role_cfg.name);
            prop_assert!(role.is_some(),
                "role '{}' from new config should exist after rebuild", role_cfg.name);
        }
    }
}
