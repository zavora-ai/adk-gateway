//! Remote agent proxy pool for routing A2A messages to agent binaries.
//!
//! Each running user agent is represented by an [`AgentProxy`] in the pool,
//! keyed by agent ID. The gateway looks up proxies here when routing inbound
//! messages to the correct child process.

use dashmap::DashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Trait + concrete proxy
// ---------------------------------------------------------------------------

/// Trait representing a remote agent proxy that can receive A2A messages.
pub trait AgentProxy: Send + Sync + std::fmt::Debug {
    /// The unique agent identifier.
    #[allow(dead_code)]
    fn agent_id(&self) -> &str;
    /// The HTTP URL of the agent's A2A endpoint (e.g. `http://127.0.0.1:19001`).
    fn agent_url(&self) -> &str;
}

/// A concrete proxy pointing at a remote agent binary's A2A endpoint.
#[derive(Debug, Clone)]
pub struct RemoteAgentProxy {
    /// Agent identifier, used by the AgentProxy trait implementation.
    #[allow(dead_code)]
    pub id: String,
    pub url: String,
    /// Port the agent is listening on, stored for diagnostics and monitoring.
    #[allow(dead_code)]
    pub port: u16,
}

impl AgentProxy for RemoteAgentProxy {
    fn agent_id(&self) -> &str {
        &self.id
    }

    fn agent_url(&self) -> &str {
        &self.url
    }
}

// ---------------------------------------------------------------------------
// Pool
// ---------------------------------------------------------------------------

/// Thread-safe pool of remote agent proxies, keyed by agent ID.
pub struct RemoteAgentProxyPool {
    proxies: DashMap<String, Arc<dyn AgentProxy>>,
}

impl RemoteAgentProxyPool {
    /// Create an empty proxy pool.
    pub fn new() -> Self {
        Self {
            proxies: DashMap::new(),
        }
    }

    /// Register a new remote agent proxy for the given agent ID and port.
    ///
    /// Creates a [`RemoteAgentProxy`] with URL `http://127.0.0.1:{port}` and
    /// inserts it into the pool. If an entry already exists for `agent_id` it
    /// is replaced.
    pub fn register(&self, agent_id: &str, port: u16) {
        let proxy = RemoteAgentProxy {
            id: agent_id.to_string(),
            url: format!("http://127.0.0.1:{}", port),
            port,
        };
        self.proxies
            .insert(agent_id.to_string(), Arc::new(proxy) as Arc<dyn AgentProxy>);
    }

    /// Remove the proxy for `agent_id`, returning `true` if it existed.
    pub fn remove(&self, agent_id: &str) -> bool {
        self.proxies.remove(agent_id).is_some()
    }

    /// Look up the proxy for `agent_id`.
    pub fn get(&self, agent_id: &str) -> Option<Arc<dyn AgentProxy>> {
        self.proxies.get(agent_id).map(|r| Arc::clone(r.value()))
    }

    /// Return the IDs of all currently registered agents.
    /// Used by integration tests and monitoring.
    #[allow(dead_code)]
    pub fn agent_ids(&self) -> Vec<String> {
        self.proxies.iter().map(|r| r.key().clone()).collect()
    }
}

impl Default for RemoteAgentProxyPool {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_get_round_trip() {
        let pool = RemoteAgentProxyPool::new();

        // Pool starts empty.
        assert!(pool.get("research").is_none());
        assert!(pool.agent_ids().is_empty());

        // Register an agent.
        pool.register("research", 19001);

        // Retrieve it and verify fields.
        let proxy = pool.get("research").expect("proxy should exist");
        assert_eq!(proxy.agent_id(), "research");
        assert_eq!(proxy.agent_url(), "http://127.0.0.1:19001");

        // agent_ids should contain the registered ID.
        let ids = pool.agent_ids();
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&"research".to_string()));

        // Remove it.
        assert!(pool.remove("research"));
        assert!(pool.get("research").is_none());
        assert!(pool.agent_ids().is_empty());

        // Removing again returns false.
        assert!(!pool.remove("research"));
    }

    #[test]
    fn register_multiple_agents() {
        let pool = RemoteAgentProxyPool::new();

        pool.register("agent-a", 19001);
        pool.register("agent-b", 19002);
        pool.register("agent-c", 19003);

        assert_eq!(pool.agent_ids().len(), 3);

        let b = pool.get("agent-b").unwrap();
        assert_eq!(b.agent_url(), "http://127.0.0.1:19002");

        pool.remove("agent-b");
        assert!(pool.get("agent-b").is_none());
        assert_eq!(pool.agent_ids().len(), 2);
    }

    #[test]
    fn register_replaces_existing() {
        let pool = RemoteAgentProxyPool::new();

        pool.register("research", 19001);
        assert_eq!(
            pool.get("research").unwrap().agent_url(),
            "http://127.0.0.1:19001"
        );

        // Re-register on a different port.
        pool.register("research", 19099);
        assert_eq!(
            pool.get("research").unwrap().agent_url(),
            "http://127.0.0.1:19099"
        );

        // Still only one entry.
        assert_eq!(pool.agent_ids().len(), 1);
    }
}
