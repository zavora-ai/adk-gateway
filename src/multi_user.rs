//! Multi-User Support — per-user session isolation, agent routing, and group chat handling.
//!
//! Extends the pairing system to support multiple users per channel. Each paired user
//! gets an independent session, heartbeat schedule, and delivery target.
//!
//! Design: MultiUserManager [Req 8.1–8.7]
//!
//! Key properties:
//! - Property 9: Adding/removing a user SHALL NOT affect any other user's state.
//! - Property 10: Agent Router SHALL route messages to the correct agent based on group context.

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::channel::ChannelType;
use crate::config::MultiUserConfig;
use crate::heartbeat_v2::HeartbeatV2;

// ── Error Types ────────────────────────────────────────────────────

/// Errors that can occur during multi-user operations.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PairingError {
    #[error("user '{0}' is already paired on this channel")]
    AlreadyPaired(String),
    #[error("user '{0}' not found")]
    UserNotFound(String),
    #[error("invalid channel type")]
    InvalidChannel,
}

// ── Data Types ─────────────────────────────────────────────────────

/// A user that has been paired with the gateway on a specific channel.
///
/// Each paired user has an independent session, heartbeat schedule, and delivery target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PairedUser {
    /// Unique user identifier (e.g., Telegram user ID).
    pub user_id: String,
    /// The channel type this user is paired on.
    pub channel_type: ChannelType,
    /// When this user was paired.
    pub paired_at: DateTime<Utc>,
    /// Optional display name for the user.
    pub display_name: Option<String>,
    /// Whether heartbeat is enabled for this user.
    pub heartbeat_enabled: bool,
    /// Custom heartbeat interval override (None = use default).
    pub heartbeat_interval_secs: Option<u64>,
}

impl PairedUser {
    /// Create a new paired user with default settings.
    pub fn new(user_id: impl Into<String>, channel_type: ChannelType) -> Self {
        Self {
            user_id: user_id.into(),
            channel_type,
            paired_at: Utc::now(),
            display_name: None,
            heartbeat_enabled: true,
            heartbeat_interval_secs: None,
        }
    }
}

/// Handle to a user's session, providing isolated session history.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    /// The user this session belongs to.
    pub user_id: String,
    /// The channel type for this session.
    pub channel_type: ChannelType,
    /// Per-user session history (isolated from other users).
    pub history: Vec<SessionMessage>,
    /// When this session was created.
    pub created_at: DateTime<Utc>,
    /// When this session was last active.
    pub last_activity: DateTime<Utc>,
}

/// A message in a user's session history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMessage {
    /// The role of the sender (user, assistant, system).
    pub role: String,
    /// The message content.
    pub content: String,
    /// When this message was sent.
    pub timestamp: DateTime<Utc>,
    /// Optional thread ID for group chat scoping.
    pub thread_id: Option<String>,
}

/// Thread context for scoping responses in group chats.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadContext {
    /// The group/channel ID.
    pub group_id: String,
    /// The thread/topic ID within the group.
    pub thread_id: Option<String>,
    /// The sender's user ID.
    pub sender_id: String,
}

// ── Agent Router ───────────────────────────────────────────────────

/// A routing rule that maps a group to a specific agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRoutingRule {
    /// The group ID this rule applies to.
    pub group_id: String,
    /// Optional thread ID for more specific routing.
    pub thread_id: Option<String>,
    /// The agent ID to route messages to.
    pub agent_id: String,
}

/// Routes messages from specific groups to designated agents.
///
/// Implements Property 10: For any routing configuration mapping groups to agents,
/// and any incoming message with a group context, the Agent Router SHALL select
/// the agent whose routing rule matches the message's group. Messages without a
/// matching rule SHALL fall through to the default agent.
pub struct AgentRouter {
    /// Routing rules indexed by group_id for fast lookup.
    rules: HashMap<String, Vec<AgentRoutingRule>>,
    /// The default agent ID for messages without a matching rule.
    default_agent: String,
}

impl AgentRouter {
    /// Create a new AgentRouter with the given rules and default agent.
    pub fn new(rules: Vec<AgentRoutingRule>, default_agent: impl Into<String>) -> Self {
        let mut rule_map: HashMap<String, Vec<AgentRoutingRule>> = HashMap::new();
        for rule in rules {
            rule_map
                .entry(rule.group_id.clone())
                .or_default()
                .push(rule);
        }
        Self {
            rules: rule_map,
            default_agent: default_agent.into(),
        }
    }

    /// Route a message to the appropriate agent based on its group context.
    ///
    /// Returns the agent ID that should handle this message.
    /// If no matching rule is found, returns the default agent.
    pub fn route(&self, context: &ThreadContext) -> &str {
        if let Some(rules) = self.rules.get(&context.group_id) {
            // Try to find a rule matching both group_id and thread_id
            for rule in rules {
                if rule.thread_id.is_some() && rule.thread_id == context.thread_id {
                    return &rule.agent_id;
                }
            }
            // Fall back to group-level rule (no thread_id specified)
            for rule in rules {
                if rule.thread_id.is_none() {
                    return &rule.agent_id;
                }
            }
        }
        &self.default_agent
    }

    /// Get the default agent ID.
    pub fn default_agent(&self) -> &str {
        &self.default_agent
    }

    /// Get all routing rules.
    pub fn rules(&self) -> Vec<&AgentRoutingRule> {
        self.rules.values().flatten().collect()
    }

    /// Update the routing rules (e.g., from config reload).
    pub fn update_rules(&mut self, rules: Vec<AgentRoutingRule>) {
        self.rules.clear();
        for rule in rules {
            self.rules
                .entry(rule.group_id.clone())
                .or_default()
                .push(rule);
        }
    }
}

// ── MultiUserManager ───────────────────────────────────────────────

/// Manages multiple paired users per channel with independent sessions and heartbeats.
///
/// Implements Property 9: Adding/removing a user SHALL NOT affect any other user's state.
/// Each user has an independent session history, heartbeat schedule, and delivery target.
pub struct MultiUserManager {
    /// All paired users indexed by (channel_type, user_id).
    paired_users: DashMap<(ChannelType, String), PairedUser>,
    /// Per-user sessions indexed by user_id.
    sessions: DashMap<String, SessionHandle>,
    /// The heartbeat system for per-user heartbeat delivery.
    heartbeat: HeartbeatV2,
    /// Agent router for group chat routing.
    router: std::sync::RwLock<AgentRouter>,
}

impl MultiUserManager {
    /// Create a new MultiUserManager with the given configuration.
    pub fn new(config: &MultiUserConfig) -> Self {
        let heartbeat_config = crate::config::HeartbeatV2Config::default();
        let router = AgentRouter::new(
            config
                .routing_rules
                .iter()
                .map(|r| AgentRoutingRule {
                    group_id: r.group_id.clone(),
                    thread_id: r.thread_id.clone(),
                    agent_id: r.agent_id.clone(),
                })
                .collect(),
            &config.default_agent,
        );

        Self {
            paired_users: DashMap::new(),
            sessions: DashMap::new(),
            heartbeat: HeartbeatV2::new(heartbeat_config),
            router: std::sync::RwLock::new(router),
        }
    }

    /// Create a new MultiUserManager with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(&MultiUserConfig::default())
    }

    // ── User Management ────────────────────────────────────────────

    /// Register a new paired user without affecting existing pairings.
    ///
    /// Property 9: Adding a new user SHALL NOT modify any existing user's pairing state,
    /// session history, or heartbeat schedule.
    pub fn add_user(&self, user: PairedUser) -> Result<(), PairingError> {
        let key = (user.channel_type, user.user_id.clone());

        // Check if user is already paired
        if self.paired_users.contains_key(&key) {
            return Err(PairingError::AlreadyPaired(user.user_id.clone()));
        }

        // Create an independent session for this user
        let session = SessionHandle {
            user_id: user.user_id.clone(),
            channel_type: user.channel_type,
            history: Vec::new(),
            created_at: Utc::now(),
            last_activity: Utc::now(),
        };

        // Insert user and session (these are independent operations on separate DashMaps)
        self.paired_users.insert(key, user.clone());
        self.sessions.insert(user.user_id.clone(), session);

        tracing::info!(
            user_id = %user.user_id,
            channel = %user.channel_type,
            "multi-user: added new paired user"
        );

        Ok(())
    }

    /// Remove a paired user, stopping their heartbeat and session.
    ///
    /// Property 9: Removing a user SHALL NOT affect any other user's state.
    pub fn remove_user(&self, user_id: &str) -> Result<(), PairingError> {
        // Find and remove the user from paired_users
        let removed = self
            .paired_users
            .iter()
            .find(|entry| entry.value().user_id == user_id)
            .map(|entry| entry.key().clone());

        match removed {
            Some(key) => {
                self.paired_users.remove(&key);

                // Stop heartbeat for this user only
                self.heartbeat.cancel_for_user(user_id);

                // Remove the user's session
                self.sessions.remove(user_id);

                tracing::info!(
                    user_id = user_id,
                    "multi-user: removed paired user, stopped heartbeat and session"
                );

                Ok(())
            }
            None => Err(PairingError::UserNotFound(user_id.to_string())),
        }
    }

    /// Get all paired users for a specific channel type.
    pub fn users_for_channel(&self, channel: ChannelType) -> Vec<PairedUser> {
        self.paired_users
            .iter()
            .filter(|entry| entry.key().0 == channel)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Get a specific paired user by channel type and user ID.
    pub fn get_user(&self, channel: ChannelType, user_id: &str) -> Option<PairedUser> {
        self.paired_users
            .get(&(channel, user_id.to_string()))
            .map(|entry| entry.value().clone())
    }

    /// Get the total number of paired users.
    pub fn user_count(&self) -> usize {
        self.paired_users.len()
    }

    /// Get all paired users across all channels.
    pub fn all_users(&self) -> Vec<PairedUser> {
        self.paired_users
            .iter()
            .map(|entry| entry.value().clone())
            .collect()
    }

    // ── Session Management ─────────────────────────────────────────

    /// Resolve the session for a specific user on a channel.
    ///
    /// Returns the user's independent session handle, or None if the user
    /// is not paired on this channel.
    pub fn resolve_session(
        &self,
        channel: ChannelType,
        sender_id: &str,
    ) -> Option<SessionHandle> {
        // Verify the user is paired on this channel
        let key = (channel, sender_id.to_string());
        if !self.paired_users.contains_key(&key) {
            return None;
        }

        self.sessions
            .get(sender_id)
            .map(|entry| entry.value().clone())
    }

    /// Add a message to a user's session history.
    ///
    /// Each user has an independent session history that contains only their own messages.
    pub fn add_message_to_session(
        &self,
        user_id: &str,
        message: SessionMessage,
    ) -> Result<(), PairingError> {
        match self.sessions.get_mut(user_id) {
            Some(mut session) => {
                session.last_activity = Utc::now();
                session.history.push(message);
                Ok(())
            }
            None => Err(PairingError::UserNotFound(user_id.to_string())),
        }
    }

    /// Get the session history for a specific user.
    pub fn get_session_history(&self, user_id: &str) -> Option<Vec<SessionMessage>> {
        self.sessions
            .get(user_id)
            .map(|session| session.history.clone())
    }

    // ── Thread Context & Group Chat ────────────────────────────────

    /// Route a message to the appropriate agent based on thread context.
    ///
    /// Property 10: The Agent Router SHALL select the agent whose routing rule
    /// matches the message's group. Messages without a matching rule SHALL fall
    /// through to the default agent.
    pub fn route_message(&self, context: &ThreadContext) -> String {
        self.router
            .read()
            .map(|router| router.route(context).to_string())
            .unwrap_or_else(|_| "default".to_string())
    }

    /// Update the agent routing rules.
    pub fn update_routing_rules(&self, rules: Vec<AgentRoutingRule>) {
        if let Ok(mut router) = self.router.write() {
            router.update_rules(rules);
        }
    }

    // ── Heartbeat Integration ──────────────────────────────────────

    /// Schedule heartbeat for a specific user based on their configuration.
    ///
    /// Each paired user gets an independent heartbeat schedule.
    pub async fn schedule_heartbeat_for_user(&self, user_id: &str) {
        if let Some(user) = self.get_user_by_id(user_id) {
            if user.heartbeat_enabled {
                let interval = user
                    .heartbeat_interval_secs
                    .map(Duration::from_secs)
                    .unwrap_or_else(|| {
                        Duration::from_secs(self.heartbeat.config().default_interval_secs)
                    });
                self.heartbeat.schedule_for_user(user_id, interval).await;
            }
        }
    }

    /// Cancel heartbeat for a specific user.
    pub fn cancel_heartbeat_for_user(&self, user_id: &str) {
        self.heartbeat.cancel_for_user(user_id);
    }

    /// Check if a user has an active heartbeat schedule.
    pub fn has_heartbeat(&self, user_id: &str) -> bool {
        self.heartbeat.has_schedule(user_id)
    }

    /// Get a reference to the underlying HeartbeatV2 instance.
    pub fn heartbeat(&self) -> &HeartbeatV2 {
        &self.heartbeat
    }

    // ── Internal Helpers ───────────────────────────────────────────

    /// Find a user by ID across all channels.
    fn get_user_by_id(&self, user_id: &str) -> Option<PairedUser> {
        self.paired_users
            .iter()
            .find(|entry| entry.value().user_id == user_id)
            .map(|entry| entry.value().clone())
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_user(id: &str, channel: ChannelType) -> PairedUser {
        PairedUser::new(id, channel)
    }

    // ── add_user tests ─────────────────────────────────────────────

    #[test]
    fn add_user_registers_successfully() {
        let mgr = MultiUserManager::with_defaults();
        let user = make_user("user1", ChannelType::Telegram);
        assert!(mgr.add_user(user).is_ok());
        assert_eq!(mgr.user_count(), 1);
    }

    #[test]
    fn add_user_creates_independent_session() {
        let mgr = MultiUserManager::with_defaults();
        let user = make_user("user1", ChannelType::Telegram);
        mgr.add_user(user).unwrap();

        let session = mgr.resolve_session(ChannelType::Telegram, "user1");
        assert!(session.is_some());
        let session = session.unwrap();
        assert_eq!(session.user_id, "user1");
        assert!(session.history.is_empty());
    }

    #[test]
    fn add_user_duplicate_returns_error() {
        let mgr = MultiUserManager::with_defaults();
        let user = make_user("user1", ChannelType::Telegram);
        mgr.add_user(user.clone()).unwrap();

        let result = mgr.add_user(user);
        assert!(matches!(result, Err(PairingError::AlreadyPaired(_))));
    }

    #[test]
    fn add_user_does_not_affect_existing_users() {
        let mgr = MultiUserManager::with_defaults();

        // Add first user with some session history
        let user1 = make_user("user1", ChannelType::Telegram);
        mgr.add_user(user1).unwrap();
        mgr.add_message_to_session(
            "user1",
            SessionMessage {
                role: "user".into(),
                content: "Hello from user1".into(),
                timestamp: Utc::now(),
                thread_id: None,
            },
        )
        .unwrap();

        // Add second user
        let user2 = make_user("user2", ChannelType::Telegram);
        mgr.add_user(user2).unwrap();

        // Verify user1's session is unchanged
        let history = mgr.get_session_history("user1").unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, "Hello from user1");

        // Verify user2 has empty session
        let history2 = mgr.get_session_history("user2").unwrap();
        assert!(history2.is_empty());
    }

    // ── remove_user tests ──────────────────────────────────────────

    #[test]
    fn remove_user_stops_session() {
        let mgr = MultiUserManager::with_defaults();
        let user = make_user("user1", ChannelType::Telegram);
        mgr.add_user(user).unwrap();

        mgr.remove_user("user1").unwrap();

        assert_eq!(mgr.user_count(), 0);
        assert!(mgr.resolve_session(ChannelType::Telegram, "user1").is_none());
    }

    #[test]
    fn remove_user_not_found_returns_error() {
        let mgr = MultiUserManager::with_defaults();
        let result = mgr.remove_user("nonexistent");
        assert!(matches!(result, Err(PairingError::UserNotFound(_))));
    }

    #[test]
    fn remove_user_does_not_affect_other_users() {
        let mgr = MultiUserManager::with_defaults();

        let user1 = make_user("user1", ChannelType::Telegram);
        let user2 = make_user("user2", ChannelType::Telegram);
        let user3 = make_user("user3", ChannelType::Slack);

        mgr.add_user(user1).unwrap();
        mgr.add_user(user2).unwrap();
        mgr.add_user(user3).unwrap();

        // Add messages to each user's session
        for id in &["user1", "user2", "user3"] {
            mgr.add_message_to_session(
                id,
                SessionMessage {
                    role: "user".into(),
                    content: format!("Message from {}", id),
                    timestamp: Utc::now(),
                    thread_id: None,
                },
            )
            .unwrap();
        }

        // Remove user2
        mgr.remove_user("user2").unwrap();

        // Verify user1 and user3 are unaffected
        assert_eq!(mgr.user_count(), 2);
        assert!(mgr.get_user(ChannelType::Telegram, "user1").is_some());
        assert!(mgr.get_user(ChannelType::Slack, "user3").is_some());

        let h1 = mgr.get_session_history("user1").unwrap();
        assert_eq!(h1.len(), 1);
        assert_eq!(h1[0].content, "Message from user1");

        let h3 = mgr.get_session_history("user3").unwrap();
        assert_eq!(h3.len(), 1);
        assert_eq!(h3[0].content, "Message from user3");
    }

    // ── Session isolation tests ────────────────────────────────────

    #[test]
    fn sessions_are_isolated_per_user() {
        let mgr = MultiUserManager::with_defaults();

        let user1 = make_user("user1", ChannelType::Telegram);
        let user2 = make_user("user2", ChannelType::Telegram);
        mgr.add_user(user1).unwrap();
        mgr.add_user(user2).unwrap();

        // Add messages to user1 only
        mgr.add_message_to_session(
            "user1",
            SessionMessage {
                role: "user".into(),
                content: "Private message for user1".into(),
                timestamp: Utc::now(),
                thread_id: None,
            },
        )
        .unwrap();

        // user2's session should be empty
        let h2 = mgr.get_session_history("user2").unwrap();
        assert!(h2.is_empty());

        // user1's session should have the message
        let h1 = mgr.get_session_history("user1").unwrap();
        assert_eq!(h1.len(), 1);
        assert_eq!(h1[0].content, "Private message for user1");
    }

    // ── Agent routing tests ────────────────────────────────────────

    #[test]
    fn agent_router_routes_by_group() {
        let rules = vec![
            AgentRoutingRule {
                group_id: "group-dev".into(),
                thread_id: None,
                agent_id: "dev-agent".into(),
            },
            AgentRoutingRule {
                group_id: "group-ops".into(),
                thread_id: None,
                agent_id: "ops-agent".into(),
            },
        ];

        let router = AgentRouter::new(rules, "default-agent");

        let ctx = ThreadContext {
            group_id: "group-dev".into(),
            thread_id: None,
            sender_id: "user1".into(),
        };
        assert_eq!(router.route(&ctx), "dev-agent");

        let ctx2 = ThreadContext {
            group_id: "group-ops".into(),
            thread_id: None,
            sender_id: "user2".into(),
        };
        assert_eq!(router.route(&ctx2), "ops-agent");
    }

    #[test]
    fn agent_router_falls_through_to_default() {
        let rules = vec![AgentRoutingRule {
            group_id: "group-dev".into(),
            thread_id: None,
            agent_id: "dev-agent".into(),
        }];

        let router = AgentRouter::new(rules, "default-agent");

        let ctx = ThreadContext {
            group_id: "unknown-group".into(),
            thread_id: None,
            sender_id: "user1".into(),
        };
        assert_eq!(router.route(&ctx), "default-agent");
    }

    #[test]
    fn agent_router_thread_specific_routing() {
        let rules = vec![
            AgentRoutingRule {
                group_id: "group-dev".into(),
                thread_id: Some("thread-123".into()),
                agent_id: "thread-agent".into(),
            },
            AgentRoutingRule {
                group_id: "group-dev".into(),
                thread_id: None,
                agent_id: "group-agent".into(),
            },
        ];

        let router = AgentRouter::new(rules, "default-agent");

        // Thread-specific match
        let ctx = ThreadContext {
            group_id: "group-dev".into(),
            thread_id: Some("thread-123".into()),
            sender_id: "user1".into(),
        };
        assert_eq!(router.route(&ctx), "thread-agent");

        // Group-level match (no thread match)
        let ctx2 = ThreadContext {
            group_id: "group-dev".into(),
            thread_id: Some("thread-999".into()),
            sender_id: "user1".into(),
        };
        assert_eq!(router.route(&ctx2), "group-agent");

        // Group-level match (no thread specified)
        let ctx3 = ThreadContext {
            group_id: "group-dev".into(),
            thread_id: None,
            sender_id: "user1".into(),
        };
        assert_eq!(router.route(&ctx3), "group-agent");
    }

    // ── users_for_channel tests ────────────────────────────────────

    #[test]
    fn users_for_channel_filters_correctly() {
        let mgr = MultiUserManager::with_defaults();

        mgr.add_user(make_user("tg1", ChannelType::Telegram)).unwrap();
        mgr.add_user(make_user("tg2", ChannelType::Telegram)).unwrap();
        mgr.add_user(make_user("sl1", ChannelType::Slack)).unwrap();

        let tg_users = mgr.users_for_channel(ChannelType::Telegram);
        assert_eq!(tg_users.len(), 2);

        let sl_users = mgr.users_for_channel(ChannelType::Slack);
        assert_eq!(sl_users.len(), 1);
        assert_eq!(sl_users[0].user_id, "sl1");
    }

    // ── Heartbeat integration tests ────────────────────────────────

    #[tokio::test]
    async fn heartbeat_scheduled_independently_per_user() {
        let mgr = MultiUserManager::with_defaults();

        let user1 = make_user("user1", ChannelType::Telegram);
        let user2 = make_user("user2", ChannelType::Telegram);
        mgr.add_user(user1).unwrap();
        mgr.add_user(user2).unwrap();

        mgr.schedule_heartbeat_for_user("user1").await;
        mgr.schedule_heartbeat_for_user("user2").await;

        assert!(mgr.has_heartbeat("user1"));
        assert!(mgr.has_heartbeat("user2"));

        // Cancel user1's heartbeat — user2 should be unaffected
        mgr.cancel_heartbeat_for_user("user1");
        assert!(!mgr.has_heartbeat("user1"));
        assert!(mgr.has_heartbeat("user2"));
    }

    #[tokio::test]
    async fn remove_user_cancels_heartbeat() {
        let mgr = MultiUserManager::with_defaults();

        let user1 = make_user("user1", ChannelType::Telegram);
        mgr.add_user(user1).unwrap();
        mgr.schedule_heartbeat_for_user("user1").await;
        assert!(mgr.has_heartbeat("user1"));

        mgr.remove_user("user1").unwrap();
        assert!(!mgr.has_heartbeat("user1"));
    }

    // ── Multi-user route_message tests ─────────────────────────────

    #[test]
    fn route_message_uses_agent_router() {
        let config = MultiUserConfig {
            enabled: true,
            default_agent: "main-agent".into(),
            routing_rules: vec![crate::config::GroupRoutingRule {
                group_id: "dev-group".into(),
                thread_id: None,
                agent_id: "dev-agent".into(),
            }],
            default_heartbeat_interval_secs: 3600,
        };
        let mgr = MultiUserManager::new(&config);

        let ctx = ThreadContext {
            group_id: "dev-group".into(),
            thread_id: None,
            sender_id: "user1".into(),
        };
        assert_eq!(mgr.route_message(&ctx), "dev-agent");

        let ctx2 = ThreadContext {
            group_id: "other-group".into(),
            thread_id: None,
            sender_id: "user1".into(),
        };
        assert_eq!(mgr.route_message(&ctx2), "main-agent");
    }
}

