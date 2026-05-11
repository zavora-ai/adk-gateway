//! Self-contained access control bridge for the gateway.
//!
//! Maps channel DM policies and user identities to access decisions.
//! Provides tool-level protection via role and scope checks.
//! Designed to be replaced by adk-auth integration when available.
//!
//! Implements requirements R26.1, R26.2, R26.4, R26.5, R26.6, R26.7, R26.8, R26.11.

use crate::channel::{ChannelType, InboundMessage};
use crate::config::{AuthConfig, ChannelAuthOverride, DmPolicy, GatewayConfig};
use std::collections::{HashMap, HashSet};

// ── Auth Decision ──────────────────────────────────────────────────

/// Result of an access control check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthDecision {
    /// Access granted.
    Allowed,
    /// Access denied with a human-readable reason.
    Denied { reason: String },
    /// User must complete the pairing flow before access is granted.
    RequiresPairing,
}

// ── Role & Identity ────────────────────────────────────────────────

/// A role that can be assigned to users.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Role {
    pub name: String,
    pub permissions: Vec<String>,
    pub scopes: Vec<String>,
}

/// A resolved user identity within the access control system.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuthIdentity {
    /// Canonical identity string, e.g. "telegram:12345" or "slack:U0ABC".
    pub canonical_id: String,
    /// The raw platform-specific user ID.
    pub platform_id: String,
    /// Which channel this identity originates from.
    pub channel_type: ChannelType,
}

// ── Tool Access Decision (R26.4, R26.5, R26.11) ───────────────────

/// Result of a tool-level access check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolAccessDecision {
    /// Tool execution is allowed.
    Allowed,
    /// Tool execution is denied with a reason.
    Denied { reason: String },
}

// ── ScopeResolver (R26.6) ──────────────────────────────────────────

/// Resolves the set of scopes available to a user.
///
/// Implementations check different sources (static config, JWT claims,
/// session state) and return whatever scopes they can find.
pub trait ScopeResolver: Send + Sync {
    /// Return the scopes available to the given canonical user ID.
    /// Returns an empty set if this resolver has no information.
    fn resolve_scopes(&self, canonical_id: &str) -> HashSet<String>;
}

/// Returns scopes from a static configuration map (user → scopes).
pub struct StaticScopeResolver {
    /// canonical_id → set of scopes
    user_scopes: HashMap<String, HashSet<String>>,
}

impl StaticScopeResolver {
    pub fn new(user_scopes: HashMap<String, HashSet<String>>) -> Self {
        Self { user_scopes }
    }
}

impl ScopeResolver for StaticScopeResolver {
    fn resolve_scopes(&self, canonical_id: &str) -> HashSet<String> {
        self.user_scopes
            .get(canonical_id)
            .cloned()
            .unwrap_or_default()
    }
}

/// Tries multiple resolvers in order and merges all results.
///
/// The chain checks session state → JWT claims → static config (or
/// whatever resolvers are provided). Results from all resolvers are
/// merged into a single set.
pub struct ChainedScopeResolver {
    resolvers: Vec<Box<dyn ScopeResolver>>,
}

impl ChainedScopeResolver {
    pub fn new(resolvers: Vec<Box<dyn ScopeResolver>>) -> Self {
        Self { resolvers }
    }
}

impl ScopeResolver for ChainedScopeResolver {
    fn resolve_scopes(&self, canonical_id: &str) -> HashSet<String> {
        let mut merged = HashSet::new();
        for resolver in &self.resolvers {
            merged.extend(resolver.resolve_scopes(canonical_id));
        }
        merged
    }
}

// ── ToolAccessCheck (R26.4, R26.5) ─────────────────────────────────

/// Evaluates whether a user may execute a specific tool based on
/// their roles and scopes.
pub struct ToolAccessCheck;

impl ToolAccessCheck {
    /// Check if a user has the required role and scopes to execute a tool.
    ///
    /// - `user_roles`: the set of role names assigned to the user.
    /// - `user_scopes`: the set of scopes resolved for the user.
    /// - `tool_name`: the name of the tool (for error messages).
    /// - `required_role`: if `Some`, the user must hold this role.
    /// - `required_scopes`: the user must hold *all* of these scopes.
    pub fn check_tool_access(
        user_roles: &HashSet<String>,
        user_scopes: &HashSet<String>,
        tool_name: &str,
        required_role: Option<&str>,
        required_scopes: &[String],
    ) -> ToolAccessDecision {
        // Check role requirement first.
        if let Some(role) = required_role {
            if !user_roles.contains(role) {
                return ToolAccessDecision::Denied {
                    reason: format!(
                        "tool '{}' requires role '{}' which the user does not have",
                        tool_name, role
                    ),
                };
            }
        }

        // Check scope requirements — user must have ALL required scopes.
        let missing: Vec<&String> = required_scopes
            .iter()
            .filter(|s| !user_scopes.contains(s.as_str()))
            .collect();

        if !missing.is_empty() {
            let missing_list: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
            return ToolAccessDecision::Denied {
                reason: format!(
                    "tool '{}' requires scopes [{}] which the user is missing",
                    tool_name,
                    missing_list.join(", ")
                ),
            };
        }

        ToolAccessDecision::Allowed
    }
}

// ── Per-Channel Policy ─────────────────────────────────────────────

/// Resolved access policy for a single channel (or the global default).
#[derive(Debug, Clone)]
struct ChannelPolicy {
    dm_policy: DmPolicy,
    /// User IDs allowed when dm_policy == Allowlist.
    allow_list: HashSet<String>,
    /// Additional roles defined at the channel level.
    #[allow(dead_code)] // Populated from config for future per-channel role enforcement
    roles: Vec<Role>,
}

// ── AccessControlBridge ────────────────────────────────────────────

/// Central access control component.
///
/// Stores roles, user-to-role mappings, paired users, and per-channel
/// DM policies. Provides `check_message_access()` as the single entry
/// point for authorization decisions.
pub struct AccessControlBridge {
    /// Global roles from config.
    roles: HashMap<String, Role>,
    /// user canonical_id → set of role names.
    user_roles: HashMap<String, HashSet<String>>,
    /// Users that have completed the pairing flow.
    paired_users: HashSet<String>,
    /// Per-channel policy overrides keyed by channel name (e.g. "telegram").
    channel_policies: HashMap<String, ChannelPolicy>,
    /// Default (global) policy derived from the first channel or config.
    default_policy: ChannelPolicy,
}

impl AccessControlBridge {
    /// Build an `AccessControlBridge` from the gateway configuration.
    ///
    /// Reads roles, user-to-role mappings, and per-channel DM policies
    /// from the `AuthConfig` and `ChannelsConfig` sections.
    pub fn new(config: &GatewayConfig) -> Self {
        let mut roles = HashMap::new();
        let mut user_roles: HashMap<String, HashSet<String>> = HashMap::new();
        let mut channel_policies = HashMap::new();

        // ── Load roles and user mappings from auth config ──────────
        if let Some(ref auth) = config.auth {
            Self::load_auth_config(auth, &mut roles, &mut user_roles);

            // Load per-channel overrides from auth config.
            for (channel_name, override_cfg) in &auth.channel_overrides {
                let policy = Self::build_channel_policy_from_override(override_cfg, config);
                channel_policies.insert(channel_name.clone(), policy);
            }
        }

        // ── Derive channel policies from channel DM settings ───────
        if let Some(ref tg) = config.channels.telegram {
            channel_policies
                .entry("telegram".to_string())
                .or_insert_with(|| ChannelPolicy {
                    dm_policy: tg.dm_policy.clone(),
                    allow_list: tg.allow_from.iter().cloned().collect(),
                    roles: vec![],
                });
        }

        if let Some(ref sl) = config.channels.slack {
            channel_policies
                .entry("slack".to_string())
                .or_insert_with(|| ChannelPolicy {
                    dm_policy: sl.dm_policy.clone(),
                    allow_list: sl.allow_from.iter().cloned().collect(),
                    roles: vec![],
                });
        }

        let default_policy = ChannelPolicy {
            dm_policy: DmPolicy::Pairing,
            allow_list: HashSet::new(),
            roles: vec![],
        };

        Self {
            roles,
            user_roles,
            paired_users: HashSet::new(),
            channel_policies,
            default_policy,
        }
    }

    // ── Identity mapping (R26.8) ───────────────────────────────────

    /// Map a channel-specific user identity to a canonical `AuthIdentity`.
    ///
    /// The canonical ID format is `"<channel_type>:<platform_id>"`, e.g.
    /// `"telegram:12345"` or `"slack:U0ABC"`.
    pub fn map_identity(&self, msg: &InboundMessage) -> AuthIdentity {
        AuthIdentity {
            canonical_id: format!("{}:{}", msg.channel_type, msg.sender_id),
            platform_id: msg.sender_id.clone(),
            channel_type: msg.channel_type,
        }
    }

    // ── Access check (R26.1, R26.2, R26.7) ─────────────────────────

    /// Check whether the sender of `msg` is allowed to interact with the
    /// gateway. Returns an `AuthDecision`.
    ///
    /// The decision is based on the DM policy for the message's channel
    /// (with per-channel overrides from config), the allow-list, and the
    /// user's pairing state.
    pub fn check_message_access(&self, msg: &InboundMessage) -> AuthDecision {
        let identity = self.map_identity(msg);
        let channel_name = msg.channel_type.to_string();
        let policy = self
            .channel_policies
            .get(&channel_name)
            .unwrap_or(&self.default_policy);

        Self::evaluate_policy(policy, &identity, &self.paired_users)
    }

    // ── Pairing management ─────────────────────────────────────────

    /// Mark a user as paired, granting them the "paired" role.
    pub fn mark_paired(&mut self, canonical_id: &str) {
        self.paired_users.insert(canonical_id.to_string());
        self.user_roles
            .entry(canonical_id.to_string())
            .or_default()
            .insert("paired".to_string());
    }

    /// Check whether a user has completed pairing.
    pub fn is_paired(&self, canonical_id: &str) -> bool {
        self.paired_users.contains(canonical_id)
    }

    // ── Hot-reload (R26.15) ────────────────────────────────────────

    /// Rebuild the access control state from a new config without
    /// dropping paired-user state.
    pub fn rebuild(&mut self, config: &GatewayConfig) {
        let mut roles = HashMap::new();
        let mut user_roles: HashMap<String, HashSet<String>> = HashMap::new();
        let mut channel_policies = HashMap::new();

        if let Some(ref auth) = config.auth {
            Self::load_auth_config(auth, &mut roles, &mut user_roles);

            for (channel_name, override_cfg) in &auth.channel_overrides {
                let policy = Self::build_channel_policy_from_override(override_cfg, config);
                channel_policies.insert(channel_name.clone(), policy);
            }
        }

        if let Some(ref tg) = config.channels.telegram {
            channel_policies
                .entry("telegram".to_string())
                .or_insert_with(|| ChannelPolicy {
                    dm_policy: tg.dm_policy.clone(),
                    allow_list: tg.allow_from.iter().cloned().collect(),
                    roles: vec![],
                });
        }

        if let Some(ref sl) = config.channels.slack {
            channel_policies
                .entry("slack".to_string())
                .or_insert_with(|| ChannelPolicy {
                    dm_policy: sl.dm_policy.clone(),
                    allow_list: sl.allow_from.iter().cloned().collect(),
                    roles: vec![],
                });
        }

        // Preserve paired users across rebuild.
        for canonical_id in &self.paired_users {
            user_roles
                .entry(canonical_id.clone())
                .or_default()
                .insert("paired".to_string());
        }

        self.roles = roles;
        self.user_roles = user_roles;
        self.channel_policies = channel_policies;
    }

    // ── Role queries ───────────────────────────────────────────────

    /// Get the set of role names assigned to a user.
    pub fn user_role_names(&self, canonical_id: &str) -> HashSet<String> {
        self.user_roles
            .get(canonical_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Look up a role definition by name.
    pub fn get_role(&self, name: &str) -> Option<&Role> {
        self.roles.get(name)
    }

    /// Return all configured role names.
    pub fn role_names(&self) -> Vec<String> {
        // Uses get_role internally to verify each role exists
        self.roles
            .keys()
            .filter(|name| self.get_role(name).is_some())
            .cloned()
            .collect()
    }

    /// Return all paired user IDs (for diagnostics).
    pub fn paired_user_ids(&self) -> Vec<String> {
        // Uses is_paired internally to verify each user
        self.paired_users
            .iter()
            .filter(|id| self.is_paired(id))
            .cloned()
            .collect()
    }

    // ── Tool protection (R26.4, R26.5, R26.6, R26.11) ─────────────

    /// Check whether a user may execute a tool, considering both role
    /// and scope requirements.
    ///
    /// Uses the provided `ScopeResolver` to determine the user's scopes,
    /// then delegates to `ToolAccessCheck`.
    pub fn wrap_tool_check(
        &self,
        canonical_id: &str,
        tool_name: &str,
        required_role: Option<&str>,
        required_scopes: &[String],
        scope_resolver: &dyn ScopeResolver,
    ) -> ToolAccessDecision {
        let user_roles = self.user_role_names(canonical_id);
        let user_scopes = scope_resolver.resolve_scopes(canonical_id);

        let decision = ToolAccessCheck::check_tool_access(
            &user_roles,
            &user_scopes,
            tool_name,
            required_role,
            required_scopes,
        );

        // Log denied decisions (R26.11).
        if let ToolAccessDecision::Denied { ref reason } = decision {
            tracing::warn!(
                user = canonical_id,
                tool = tool_name,
                reason = reason.as_str(),
                "tool access denied"
            );
        }

        decision
    }

    // ── Internal helpers ───────────────────────────────────────────

    fn load_auth_config(
        auth: &AuthConfig,
        roles: &mut HashMap<String, Role>,
        user_roles: &mut HashMap<String, HashSet<String>>,
    ) {
        for rc in &auth.roles {
            roles.insert(
                rc.name.clone(),
                Role {
                    name: rc.name.clone(),
                    permissions: rc.permissions.clone(),
                    scopes: rc.scopes.clone(),
                },
            );
        }

        for mapping in &auth.user_mappings {
            user_roles
                .entry(mapping.user_id.clone())
                .or_default()
                .insert(mapping.role.clone());
        }
    }

    fn build_channel_policy_from_override(
        override_cfg: &ChannelAuthOverride,
        _config: &GatewayConfig,
    ) -> ChannelPolicy {
        let dm_policy = override_cfg.dm_policy.clone().unwrap_or(DmPolicy::Pairing);

        let channel_roles: Vec<Role> = override_cfg
            .roles
            .iter()
            .map(|rc| Role {
                name: rc.name.clone(),
                permissions: rc.permissions.clone(),
                scopes: rc.scopes.clone(),
            })
            .collect();

        ChannelPolicy {
            dm_policy,
            allow_list: HashSet::new(),
            roles: channel_roles,
        }
    }

    fn evaluate_policy(
        policy: &ChannelPolicy,
        identity: &AuthIdentity,
        paired_users: &HashSet<String>,
    ) -> AuthDecision {
        match policy.dm_policy {
            DmPolicy::Open => AuthDecision::Allowed,

            DmPolicy::Allowlist => {
                // Check both the canonical ID and the raw platform ID.
                if policy.allow_list.contains(&identity.canonical_id)
                    || policy.allow_list.contains(&identity.platform_id)
                {
                    AuthDecision::Allowed
                } else {
                    AuthDecision::Denied {
                        reason: "user not in allow list".to_string(),
                    }
                }
            }

            DmPolicy::Pairing => {
                if paired_users.contains(&identity.canonical_id) {
                    AuthDecision::Allowed
                } else {
                    AuthDecision::RequiresPairing
                }
            }

            DmPolicy::Disabled => AuthDecision::Denied {
                reason: "DMs are disabled for this channel".to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;

    /// Helper: build a minimal InboundMessage for testing.
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
            source: Default::default(),
            timestamp: chrono::Utc::now(),
        }
    }

    fn default_config_with_telegram(dm_policy: DmPolicy, allow_from: Vec<String>) -> GatewayConfig {
        let mut cfg = GatewayConfig::default();
        cfg.channels.telegram = Some(TelegramConfig {
            enabled: true,
            bot_token: "tok".into(),
            dm_policy,
            allow_from,
            groups: GroupsConfig::default(),
            stream_mode: None,
            account_id: "default".into(),
        });
        cfg
    }

    // ── DM policy tests ────────────────────────────────────────────

    #[test]
    fn open_policy_allows_everyone() {
        let cfg = default_config_with_telegram(DmPolicy::Open, vec![]);
        let bridge = AccessControlBridge::new(&cfg);
        let msg = make_msg(ChannelType::Telegram, "12345");
        assert_eq!(bridge.check_message_access(&msg), AuthDecision::Allowed);
    }

    #[test]
    fn disabled_policy_denies_everyone() {
        let cfg = default_config_with_telegram(DmPolicy::Disabled, vec![]);
        let bridge = AccessControlBridge::new(&cfg);
        let msg = make_msg(ChannelType::Telegram, "12345");
        assert!(matches!(
            bridge.check_message_access(&msg),
            AuthDecision::Denied { .. }
        ));
    }

    #[test]
    fn allowlist_allows_listed_user() {
        let cfg = default_config_with_telegram(DmPolicy::Allowlist, vec!["alice".into()]);
        let bridge = AccessControlBridge::new(&cfg);

        let allowed = make_msg(ChannelType::Telegram, "alice");
        assert_eq!(bridge.check_message_access(&allowed), AuthDecision::Allowed);

        let denied = make_msg(ChannelType::Telegram, "bob");
        assert!(matches!(
            bridge.check_message_access(&denied),
            AuthDecision::Denied { .. }
        ));
    }

    #[test]
    fn pairing_requires_pairing_then_allows_after_mark() {
        let cfg = default_config_with_telegram(DmPolicy::Pairing, vec![]);
        let mut bridge = AccessControlBridge::new(&cfg);
        let msg = make_msg(ChannelType::Telegram, "99");

        assert_eq!(
            bridge.check_message_access(&msg),
            AuthDecision::RequiresPairing
        );

        bridge.mark_paired("telegram:99");

        assert_eq!(bridge.check_message_access(&msg), AuthDecision::Allowed);
        assert!(bridge.is_paired("telegram:99"));
    }

    // ── Identity mapping ───────────────────────────────────────────

    #[test]
    fn identity_mapping_produces_canonical_id() {
        let cfg = GatewayConfig::default();
        let bridge = AccessControlBridge::new(&cfg);
        let msg = make_msg(ChannelType::Slack, "U0ABC");
        let id = bridge.map_identity(&msg);
        assert_eq!(id.canonical_id, "slack:U0ABC");
        assert_eq!(id.platform_id, "U0ABC");
        assert_eq!(id.channel_type, ChannelType::Slack);
    }

    // ── Per-channel overrides (R26.7) ──────────────────────────────

    #[test]
    fn channel_override_applies_different_policy() {
        let mut cfg = default_config_with_telegram(DmPolicy::Open, vec![]);
        // Override telegram to disabled via auth config.
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
                        dm_policy: Some(DmPolicy::Disabled),
                        roles: vec![],
                    },
                );
                m
            },
            audit: None,
            sso: None,
        });

        let bridge = AccessControlBridge::new(&cfg);
        let msg = make_msg(ChannelType::Telegram, "12345");
        // The auth override should take precedence.
        assert!(matches!(
            bridge.check_message_access(&msg),
            AuthDecision::Denied { .. }
        ));
    }

    // ── Hot-reload preserves paired users ──────────────────────────

    #[test]
    fn rebuild_preserves_paired_users() {
        let cfg = default_config_with_telegram(DmPolicy::Pairing, vec![]);
        let mut bridge = AccessControlBridge::new(&cfg);
        bridge.mark_paired("telegram:42");

        // Rebuild with same config.
        bridge.rebuild(&cfg);

        let msg = make_msg(ChannelType::Telegram, "42");
        assert_eq!(bridge.check_message_access(&msg), AuthDecision::Allowed);
        assert!(bridge.is_paired("telegram:42"));
    }

    // ── Comprehensive hot-reload test (R26.15) ─────────────────────

    /// Verify that `rebuild()` is the correct hot-reload entry point:
    /// - Paired users are preserved across rebuild
    /// - New roles/permissions from updated config take effect
    /// - Updated user-role mappings take effect
    /// - Changed channel DM policies take effect
    /// - Removed roles are gone after rebuild
    #[test]
    fn rebuild_hot_reload_comprehensive() {
        // ── Phase 1: Initial config with roles and pairing policy ──
        let mut initial_cfg = default_config_with_telegram(DmPolicy::Pairing, vec![]);
        initial_cfg.auth = Some(AuthConfig {
            mode: AuthMode::None,
            token: None,
            password: None,
            roles: vec![RoleConfig {
                name: "editor".into(),
                permissions: vec!["edit_content".into()],
                scopes: vec!["write:docs".into()],
            }],
            user_mappings: vec![UserRoleMapping {
                user_id: "telegram:alice".into(),
                role: "editor".into(),
            }],
            channel_overrides: HashMap::new(),
            audit: None,
            sso: None,
        });

        let mut bridge = AccessControlBridge::new(&initial_cfg);

        // Pair two users
        bridge.mark_paired("telegram:alice");
        bridge.mark_paired("telegram:bob");

        // Verify initial state
        assert!(bridge.is_paired("telegram:alice"));
        assert!(bridge.is_paired("telegram:bob"));
        assert!(bridge.get_role("editor").is_some());
        assert!(bridge.user_role_names("telegram:alice").contains("editor"));

        // ── Phase 2: Rebuild with updated config ───────────────────
        // - Change DM policy from Pairing to Open
        // - Remove "editor" role, add "admin" role
        // - Change alice's mapping from "editor" to "admin"
        // - Add a new user mapping for carol
        let mut updated_cfg = default_config_with_telegram(DmPolicy::Open, vec![]);
        updated_cfg.auth = Some(AuthConfig {
            mode: AuthMode::None,
            token: None,
            password: None,
            roles: vec![RoleConfig {
                name: "admin".into(),
                permissions: vec!["all_access".into()],
                scopes: vec!["admin:*".into()],
            }],
            user_mappings: vec![
                UserRoleMapping {
                    user_id: "telegram:alice".into(),
                    role: "admin".into(),
                },
                UserRoleMapping {
                    user_id: "telegram:carol".into(),
                    role: "admin".into(),
                },
            ],
            channel_overrides: HashMap::new(),
            audit: None,
            sso: None,
        });

        bridge.rebuild(&updated_cfg);

        // ── Verify: paired users preserved ─────────────────────────
        assert!(
            bridge.is_paired("telegram:alice"),
            "alice should still be paired"
        );
        assert!(
            bridge.is_paired("telegram:bob"),
            "bob should still be paired"
        );

        // Paired users retain the "paired" role in user_roles
        assert!(
            bridge.user_role_names("telegram:alice").contains("paired"),
            "alice should retain 'paired' role after rebuild"
        );
        assert!(
            bridge.user_role_names("telegram:bob").contains("paired"),
            "bob should retain 'paired' role after rebuild"
        );

        // ── Verify: new roles take effect ──────────────────────────
        assert!(
            bridge.get_role("admin").is_some(),
            "admin role should exist after rebuild"
        );
        let admin_role = bridge.get_role("admin").unwrap();
        assert_eq!(admin_role.permissions, vec!["all_access"]);
        assert_eq!(admin_role.scopes, vec!["admin:*"]);

        // ── Verify: old roles are gone ─────────────────────────────
        assert!(
            bridge.get_role("editor").is_none(),
            "editor role should be removed after rebuild"
        );

        // ── Verify: updated user-role mappings ─────────────────────
        let alice_roles = bridge.user_role_names("telegram:alice");
        assert!(
            alice_roles.contains("admin"),
            "alice should have admin role"
        );
        assert!(
            !alice_roles.contains("editor"),
            "alice should no longer have editor role"
        );

        let carol_roles = bridge.user_role_names("telegram:carol");
        assert!(
            carol_roles.contains("admin"),
            "carol should have admin role from new config"
        );

        // ── Verify: channel policy change takes effect ─────────────
        // Policy changed from Pairing to Open, so even an unknown user is allowed
        let unknown_msg = make_msg(ChannelType::Telegram, "unknown_user");
        assert_eq!(
            bridge.check_message_access(&unknown_msg),
            AuthDecision::Allowed,
            "open policy should allow unknown users after rebuild"
        );

        // Previously paired users still allowed under Open policy
        let alice_msg = make_msg(ChannelType::Telegram, "alice");
        assert_eq!(
            bridge.check_message_access(&alice_msg),
            AuthDecision::Allowed
        );
    }

    /// Verify rebuild with channel auth overrides updates correctly.
    #[test]
    fn rebuild_updates_channel_overrides() {
        let mut cfg = default_config_with_telegram(DmPolicy::Open, vec![]);
        let mut bridge = AccessControlBridge::new(&cfg);
        bridge.mark_paired("telegram:user1");

        // Initially Open → everyone allowed
        let msg = make_msg(ChannelType::Telegram, "anyone");
        assert_eq!(bridge.check_message_access(&msg), AuthDecision::Allowed);

        // Rebuild with a channel override that disables Telegram DMs
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
                        dm_policy: Some(DmPolicy::Disabled),
                        roles: vec![],
                    },
                );
                m
            },
            audit: None,
            sso: None,
        });

        bridge.rebuild(&cfg);

        // After rebuild, Telegram should be disabled
        assert!(
            matches!(
                bridge.check_message_access(&msg),
                AuthDecision::Denied { .. }
            ),
            "telegram should be disabled after rebuild with override"
        );

        // But paired user state is still preserved
        assert!(bridge.is_paired("telegram:user1"));
    }

    /// Verify that multiple rebuilds in sequence preserve paired users.
    #[test]
    fn multiple_rebuilds_preserve_paired_users() {
        let cfg = default_config_with_telegram(DmPolicy::Pairing, vec![]);
        let mut bridge = AccessControlBridge::new(&cfg);

        bridge.mark_paired("telegram:persistent_user");

        // Rebuild several times with different configs
        for i in 0..5 {
            let mut new_cfg = GatewayConfig::default();
            new_cfg.channels.telegram = Some(TelegramConfig {
                enabled: true,
                bot_token: format!("tok_{}", i),
                dm_policy: DmPolicy::Pairing,
                allow_from: vec![],
                groups: GroupsConfig::default(),
                stream_mode: None,
                account_id: "default".into(),
            });
            bridge.rebuild(&new_cfg);
        }

        assert!(
            bridge.is_paired("telegram:persistent_user"),
            "paired user should survive multiple rebuilds"
        );

        let msg = make_msg(ChannelType::Telegram, "persistent_user");
        assert_eq!(bridge.check_message_access(&msg), AuthDecision::Allowed);
    }

    // ── Roles from auth config ─────────────────────────────────────

    #[test]
    fn roles_loaded_from_auth_config() {
        let mut cfg = GatewayConfig::default();
        cfg.auth = Some(AuthConfig {
            mode: AuthMode::None,
            token: None,
            password: None,
            roles: vec![RoleConfig {
                name: "admin".into(),
                permissions: vec!["all_agents".into()],
                scopes: vec!["admin:*".into()],
            }],
            user_mappings: vec![UserRoleMapping {
                user_id: "telegram:1".into(),
                role: "admin".into(),
            }],
            channel_overrides: HashMap::new(),
            audit: None,
            sso: None,
        });

        let bridge = AccessControlBridge::new(&cfg);
        let role = bridge.get_role("admin").expect("admin role should exist");
        assert_eq!(role.permissions, vec!["all_agents"]);

        let user_roles = bridge.user_role_names("telegram:1");
        assert!(user_roles.contains("admin"));
    }

    // ── Unconfigured channel falls back to default policy ──────────

    #[test]
    fn unconfigured_channel_uses_default_pairing_policy() {
        let cfg = GatewayConfig::default();
        let bridge = AccessControlBridge::new(&cfg);
        // Discord is not configured, should fall back to default (Pairing).
        let msg = make_msg(ChannelType::Discord, "user1");
        assert_eq!(
            bridge.check_message_access(&msg),
            AuthDecision::RequiresPairing
        );
    }

    // ── ToolAccessCheck tests (R26.4, R26.5, R26.11) ──────────────

    #[test]
    fn tool_access_allowed_when_no_requirements() {
        let roles = HashSet::new();
        let scopes = HashSet::new();
        let decision = ToolAccessCheck::check_tool_access(&roles, &scopes, "my_tool", None, &[]);
        assert_eq!(decision, ToolAccessDecision::Allowed);
    }

    #[test]
    fn tool_access_denied_missing_role() {
        let roles: HashSet<String> = ["viewer".into()].into_iter().collect();
        let scopes = HashSet::new();
        let decision =
            ToolAccessCheck::check_tool_access(&roles, &scopes, "admin_tool", Some("admin"), &[]);
        assert!(matches!(decision, ToolAccessDecision::Denied { .. }));
        if let ToolAccessDecision::Denied { reason } = &decision {
            assert!(reason.contains("admin"));
        }
    }

    #[test]
    fn tool_access_allowed_with_correct_role() {
        let roles: HashSet<String> = ["admin".into()].into_iter().collect();
        let scopes = HashSet::new();
        let decision =
            ToolAccessCheck::check_tool_access(&roles, &scopes, "admin_tool", Some("admin"), &[]);
        assert_eq!(decision, ToolAccessDecision::Allowed);
    }

    #[test]
    fn tool_access_denied_missing_scopes() {
        let roles = HashSet::new();
        let scopes: HashSet<String> = ["read:data".into()].into_iter().collect();
        let required = vec!["read:data".into(), "write:data".into()];
        let decision =
            ToolAccessCheck::check_tool_access(&roles, &scopes, "data_tool", None, &required);
        assert!(matches!(decision, ToolAccessDecision::Denied { .. }));
        if let ToolAccessDecision::Denied { reason } = &decision {
            assert!(reason.contains("write:data"));
        }
    }

    #[test]
    fn tool_access_allowed_with_all_scopes() {
        let roles = HashSet::new();
        let scopes: HashSet<String> = ["read:data".into(), "write:data".into()]
            .into_iter()
            .collect();
        let required = vec!["read:data".into(), "write:data".into()];
        let decision =
            ToolAccessCheck::check_tool_access(&roles, &scopes, "data_tool", None, &required);
        assert_eq!(decision, ToolAccessDecision::Allowed);
    }

    #[test]
    fn tool_access_checks_role_before_scopes() {
        // User has the scopes but not the role — should be denied for role.
        let roles: HashSet<String> = ["viewer".into()].into_iter().collect();
        let scopes: HashSet<String> = ["admin:*".into()].into_iter().collect();
        let required = vec!["admin:*".into()];
        let decision = ToolAccessCheck::check_tool_access(
            &roles,
            &scopes,
            "admin_tool",
            Some("admin"),
            &required,
        );
        assert!(matches!(decision, ToolAccessDecision::Denied { .. }));
        if let ToolAccessDecision::Denied { reason } = &decision {
            assert!(reason.contains("role"));
        }
    }

    // ── ScopeResolver tests (R26.6) ────────────────────────────────

    #[test]
    fn static_scope_resolver_returns_configured_scopes() {
        let mut map = HashMap::new();
        map.insert(
            "telegram:1".into(),
            ["read:data".into(), "write:data".into()]
                .into_iter()
                .collect(),
        );
        let resolver = StaticScopeResolver::new(map);
        let scopes = resolver.resolve_scopes("telegram:1");
        assert!(scopes.contains("read:data"));
        assert!(scopes.contains("write:data"));
    }

    #[test]
    fn static_scope_resolver_returns_empty_for_unknown_user() {
        let resolver = StaticScopeResolver::new(HashMap::new());
        let scopes = resolver.resolve_scopes("unknown:user");
        assert!(scopes.is_empty());
    }

    #[test]
    fn chained_scope_resolver_merges_all_resolvers() {
        let mut map_a = HashMap::new();
        map_a.insert("user:1".into(), ["scope_a".into()].into_iter().collect());
        let mut map_b = HashMap::new();
        map_b.insert("user:1".into(), ["scope_b".into()].into_iter().collect());

        let chain = ChainedScopeResolver::new(vec![
            Box::new(StaticScopeResolver::new(map_a)),
            Box::new(StaticScopeResolver::new(map_b)),
        ]);

        let scopes = chain.resolve_scopes("user:1");
        assert!(scopes.contains("scope_a"));
        assert!(scopes.contains("scope_b"));
        assert_eq!(scopes.len(), 2);
    }

    #[test]
    fn chained_scope_resolver_handles_empty_chain() {
        let chain = ChainedScopeResolver::new(vec![]);
        let scopes = chain.resolve_scopes("user:1");
        assert!(scopes.is_empty());
    }

    // ── wrap_tool_check integration tests ──────────────────────────

    fn bridge_with_roles_and_mappings() -> (AccessControlBridge, StaticScopeResolver) {
        let mut cfg = GatewayConfig::default();
        cfg.auth = Some(AuthConfig {
            mode: AuthMode::None,
            token: None,
            password: None,
            roles: vec![
                RoleConfig {
                    name: "admin".into(),
                    permissions: vec!["all_tools".into()],
                    scopes: vec!["admin:*".into()],
                },
                RoleConfig {
                    name: "viewer".into(),
                    permissions: vec!["read_only".into()],
                    scopes: vec!["read:data".into()],
                },
            ],
            user_mappings: vec![
                UserRoleMapping {
                    user_id: "telegram:admin_user".into(),
                    role: "admin".into(),
                },
                UserRoleMapping {
                    user_id: "telegram:viewer_user".into(),
                    role: "viewer".into(),
                },
            ],
            channel_overrides: HashMap::new(),
            audit: None,
            sso: None,
        });

        let bridge = AccessControlBridge::new(&cfg);

        let mut scope_map = HashMap::new();
        scope_map.insert(
            "telegram:admin_user".into(),
            ["admin:*".into(), "read:data".into(), "write:data".into()]
                .into_iter()
                .collect(),
        );
        scope_map.insert(
            "telegram:viewer_user".into(),
            ["read:data".into()].into_iter().collect(),
        );
        let resolver = StaticScopeResolver::new(scope_map);

        (bridge, resolver)
    }

    #[test]
    fn wrap_tool_check_allows_admin_with_role_and_scopes() {
        let (bridge, resolver) = bridge_with_roles_and_mappings();
        let decision = bridge.wrap_tool_check(
            "telegram:admin_user",
            "admin_tool",
            Some("admin"),
            &["admin:*".into()],
            &resolver,
        );
        assert_eq!(decision, ToolAccessDecision::Allowed);
    }

    #[test]
    fn wrap_tool_check_denies_viewer_missing_role() {
        let (bridge, resolver) = bridge_with_roles_and_mappings();
        let decision = bridge.wrap_tool_check(
            "telegram:viewer_user",
            "admin_tool",
            Some("admin"),
            &[],
            &resolver,
        );
        assert!(matches!(decision, ToolAccessDecision::Denied { .. }));
    }

    #[test]
    fn wrap_tool_check_denies_viewer_missing_scope() {
        let (bridge, resolver) = bridge_with_roles_and_mappings();
        let decision = bridge.wrap_tool_check(
            "telegram:viewer_user",
            "write_tool",
            None,
            &["write:data".into()],
            &resolver,
        );
        assert!(matches!(decision, ToolAccessDecision::Denied { .. }));
    }

    #[test]
    fn wrap_tool_check_allows_viewer_with_matching_scope() {
        let (bridge, resolver) = bridge_with_roles_and_mappings();
        let decision = bridge.wrap_tool_check(
            "telegram:viewer_user",
            "read_tool",
            None,
            &["read:data".into()],
            &resolver,
        );
        assert_eq!(decision, ToolAccessDecision::Allowed);
    }

    #[test]
    fn wrap_tool_check_allows_when_no_requirements() {
        let (bridge, resolver) = bridge_with_roles_and_mappings();
        let decision =
            bridge.wrap_tool_check("telegram:unknown_user", "public_tool", None, &[], &resolver);
        assert_eq!(decision, ToolAccessDecision::Allowed);
    }

    #[test]
    fn wrap_tool_check_denies_unknown_user_with_role_requirement() {
        let (bridge, resolver) = bridge_with_roles_and_mappings();
        let decision = bridge.wrap_tool_check(
            "telegram:unknown_user",
            "admin_tool",
            Some("admin"),
            &[],
            &resolver,
        );
        assert!(matches!(decision, ToolAccessDecision::Denied { .. }));
    }
}
