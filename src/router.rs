//! Message router — resolves which agent handles an inbound message
//! based on routing bindings (adk-gateway-compatible) and agent-level
//! channel bindings from AgentConfig.

use crate::agent_config::ChannelBinding;
use crate::channel::InboundMessage;
use crate::config::{RoutingBinding, RoutingConfig};

/// An agent-level routing binding derived from `AgentConfig.channel_bindings`.
#[derive(Debug, Clone)]
pub struct AgentBinding {
    pub channel_type: String,
    pub account_id: Option<String>,
    pub peer_filter: Option<String>,
    pub agent_id: String,
}

/// Determines which agent should handle a message.
///
/// Resolution order (most specific → least specific):
/// 1. Exact match: channel + accountId + peer
/// 2. Channel + accountId match
/// 3. Channel-only match
/// 4. Legacy adk-gateway routing bindings
/// 5. Default agent (system)
#[derive(Clone)]
pub struct MessageRouter {
    /// Legacy adk-gateway routing bindings from config.
    bindings: Vec<RoutingBinding>,
    /// Agent-level routing bindings from AgentConfig.channel_bindings.
    agent_bindings: Vec<AgentBinding>,
    /// Default agent ID (falls back to "system").
    default_agent_id: String,
}

impl MessageRouter {
    pub fn new(routing: &RoutingConfig, default_agent: String) -> Self {
        Self {
            bindings: routing.bindings.clone(),
            agent_bindings: Vec::new(),
            default_agent_id: default_agent,
        }
    }

    /// Return the default agent ID.
    /// Used by integration tests and diagnostics.
    #[allow(dead_code)]
    pub fn default_agent_id(&self) -> &str {
        &self.default_agent_id
    }

    /// Add agent-level bindings from an AgentConfig's channel_bindings.
    pub fn add_agent_bindings(&mut self, agent_id: &str, channel_bindings: &[ChannelBinding]) {
        for cb in channel_bindings {
            self.agent_bindings.push(AgentBinding {
                channel_type: cb.channel_type.clone(),
                account_id: cb.account_id.clone(),
                peer_filter: cb.peer_filter.clone(),
                agent_id: agent_id.to_string(),
            });
        }
    }

    /// Remove all agent-level bindings for a given agent ID.
    pub fn remove_agent_bindings(&mut self, agent_id: &str) {
        self.agent_bindings.retain(|b| b.agent_id != agent_id);
    }

    /// Replace all agent-level bindings for a given agent ID.
    pub fn update_agent_bindings(&mut self, agent_id: &str, channel_bindings: &[ChannelBinding]) {
        self.remove_agent_bindings(agent_id);
        self.add_agent_bindings(agent_id, channel_bindings);
    }

    /// Resolve the agent ID for an inbound message.
    ///
    /// Resolution order:
    /// 1. Exact match on agent bindings: channel + account_id + peer
    /// 2. Channel + account_id match on agent bindings
    /// 3. Channel-only match on agent bindings
    /// 4. Legacy adk-gateway routing bindings
    /// 5. Default agent
    pub fn resolve_agent(&self, msg: &InboundMessage) -> &str {
        let channel_name = msg.channel_type.to_string();
        let msg_account = if msg.account_id.is_empty() {
            None
        } else {
            Some(msg.account_id.as_str())
        };
        let msg_peer = msg.sender_id.as_str();

        // Phase 1: exact match (channel + account + peer)
        for ab in &self.agent_bindings {
            if ab.channel_type != channel_name {
                continue;
            }
            let account_matches = match (&ab.account_id, msg_account) {
                (Some(bind_acct), Some(msg_acct)) => bind_acct == msg_acct,
                (Some(_), None) => false,
                (None, _) => false, // need account for exact match
            };
            if !account_matches {
                continue;
            }
            let peer_matches = match &ab.peer_filter {
                Some(peer) => peer == msg_peer,
                None => false, // need peer for exact match
            };
            if peer_matches {
                return &ab.agent_id;
            }
        }

        // Phase 2: channel + account match (no peer)
        for ab in &self.agent_bindings {
            if ab.channel_type != channel_name {
                continue;
            }
            if ab.peer_filter.is_some() {
                continue; // skip bindings that require peer match
            }
            let account_matches = match (&ab.account_id, msg_account) {
                (Some(bind_acct), Some(msg_acct)) => bind_acct == msg_acct,
                (Some(_), None) => false,
                (None, _) => false, // need account for this phase
            };
            if account_matches {
                return &ab.agent_id;
            }
        }

        // Phase 3: channel-only match
        for ab in &self.agent_bindings {
            if ab.channel_type != channel_name {
                continue;
            }
            if ab.account_id.is_some() || ab.peer_filter.is_some() {
                continue; // skip more specific bindings
            }
            return &ab.agent_id;
        }

        // Phase 4: legacy adk-gateway routing bindings
        for binding in &self.bindings {
            let channel_matches = binding
                .match_rule
                .channel
                .as_ref()
                .map(|c| c == &channel_name)
                .unwrap_or(true);

            if !channel_matches {
                continue;
            }

            // Check account_id match if specified in legacy binding
            if let Some(ref bind_account) = binding.match_rule.account_id {
                if msg_account != Some(bind_account.as_str()) {
                    continue;
                }
            }

            // Check peer match if specified
            if let Some(ref peer) = binding.match_rule.peer {
                if let Some(kind) = peer.get("kind").and_then(|v| v.as_str()) {
                    if let Some(id) = peer.get("id").and_then(|v| v.as_str()) {
                        let peer_matches = match kind {
                            "user" => {
                                id.trim_start_matches('@') == msg.sender_id
                                    || msg.sender_name.as_deref()
                                        == Some(id.trim_start_matches('@'))
                            }
                            "group" => msg.group_id.as_deref() == Some(id),
                            _ => false,
                        };
                        if !peer_matches {
                            continue;
                        }
                    }
                }
            }

            return &binding.agent_id;
        }

        // Phase 5: default
        &self.default_agent_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::ChannelType;
    use crate::config::{RoutingBinding, RoutingConfig, RoutingMatch};

    fn make_msg(channel: ChannelType, account_id: &str, sender_id: &str) -> InboundMessage {
        InboundMessage {
            channel_type: channel,
            account_id: account_id.to_string(),
            sender_id: sender_id.to_string(),
            sender_name: None,
            text: "hello".into(),
            is_group: false,
            group_id: None,
            is_mention: false,
            platform_message_id: "1".into(),
            attachments: vec![],
            metadata: std::collections::HashMap::new(),
            source: crate::channel::MessageSource::Channel,
            timestamp: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_default_routing() {
        let router = MessageRouter::new(&RoutingConfig { bindings: vec![] }, "main".into());
        let msg = make_msg(ChannelType::Telegram, "", "123");
        assert_eq!(router.resolve_agent(&msg), "main");
    }

    #[test]
    fn test_channel_routing() {
        let routing = RoutingConfig {
            bindings: vec![RoutingBinding {
                agent_id: "work".into(),
                match_rule: RoutingMatch {
                    channel: Some("slack".into()),
                    account_id: None,
                    peer: None,
                },
            }],
        };
        let router = MessageRouter::new(&routing, "main".into());
        assert_eq!(
            router.resolve_agent(&make_msg(ChannelType::Slack, "", "U123")),
            "work"
        );
        assert_eq!(
            router.resolve_agent(&make_msg(ChannelType::Telegram, "", "456")),
            "main"
        );
    }

    /// Task 10.9: routing falls back to system agent when no binding matches
    #[test]
    fn routing_falls_back_to_system_when_no_binding_matches() {
        let mut router = MessageRouter::new(&RoutingConfig { bindings: vec![] }, "system".into());

        // Add some specific bindings that won't match
        router.add_agent_bindings(
            "research",
            &[ChannelBinding {
                channel_type: "telegram".into(),
                account_id: Some("default".into()),
                peer_filter: None,
            }],
        );
        router.add_agent_bindings(
            "writer",
            &[ChannelBinding {
                channel_type: "slack".into(),
                account_id: Some("team-alpha".into()),
                peer_filter: None,
            }],
        );

        // A webhook message should fall back to system
        let msg = make_msg(ChannelType::Webhook, "some-hook", "sender1");
        assert_eq!(router.resolve_agent(&msg), "system");

        // A discord message should also fall back
        let msg2 = make_msg(ChannelType::Discord, "guild1", "user1");
        assert_eq!(router.resolve_agent(&msg2), "system");
    }

    #[test]
    fn agent_binding_exact_match_takes_priority() {
        let mut router = MessageRouter::new(&RoutingConfig { bindings: vec![] }, "system".into());

        // Channel-only binding
        router.add_agent_bindings(
            "general-tg",
            &[ChannelBinding {
                channel_type: "telegram".into(),
                account_id: None,
                peer_filter: None,
            }],
        );
        // Channel + account binding
        router.add_agent_bindings(
            "team-tg",
            &[ChannelBinding {
                channel_type: "telegram".into(),
                account_id: Some("default".into()),
                peer_filter: None,
            }],
        );
        // Exact match binding
        router.add_agent_bindings(
            "vip-tg",
            &[ChannelBinding {
                channel_type: "telegram".into(),
                account_id: Some("default".into()),
                peer_filter: Some("vip-user".into()),
            }],
        );

        // Exact match wins
        let msg = make_msg(ChannelType::Telegram, "default", "vip-user");
        assert_eq!(router.resolve_agent(&msg), "vip-tg");

        // Channel + account wins over channel-only
        let msg2 = make_msg(ChannelType::Telegram, "default", "regular-user");
        assert_eq!(router.resolve_agent(&msg2), "team-tg");

        // Channel-only match for different account
        let msg3 = make_msg(ChannelType::Telegram, "other-account", "someone");
        assert_eq!(router.resolve_agent(&msg3), "general-tg");
    }

    #[test]
    fn remove_agent_bindings_works() {
        let mut router = MessageRouter::new(&RoutingConfig { bindings: vec![] }, "system".into());
        router.add_agent_bindings(
            "research",
            &[ChannelBinding {
                channel_type: "telegram".into(),
                account_id: None,
                peer_filter: None,
            }],
        );
        let msg = make_msg(ChannelType::Telegram, "", "123");
        assert_eq!(router.resolve_agent(&msg), "research");

        router.remove_agent_bindings("research");
        assert_eq!(router.resolve_agent(&msg), "system");
    }

    #[test]
    fn update_agent_bindings_replaces() {
        let mut router = MessageRouter::new(&RoutingConfig { bindings: vec![] }, "system".into());
        router.add_agent_bindings(
            "research",
            &[ChannelBinding {
                channel_type: "telegram".into(),
                account_id: None,
                peer_filter: None,
            }],
        );
        assert_eq!(
            router.resolve_agent(&make_msg(ChannelType::Telegram, "", "x")),
            "research"
        );

        // Update to slack instead
        router.update_agent_bindings(
            "research",
            &[ChannelBinding {
                channel_type: "slack".into(),
                account_id: None,
                peer_filter: None,
            }],
        );
        // Telegram should now fall back to system
        assert_eq!(
            router.resolve_agent(&make_msg(ChannelType::Telegram, "", "x")),
            "system"
        );
        // Slack should route to research
        assert_eq!(
            router.resolve_agent(&make_msg(ChannelType::Slack, "", "x")),
            "research"
        );
    }
}
