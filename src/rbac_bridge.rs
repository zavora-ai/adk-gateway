//! RBAC bridge for the multi-agent isolation system.
//!
//! Provides permission enforcement for agent tool invocations and
//! sub-agent delegation. Wraps a self-contained AccessControl system
//! (standing in for adk-auth) that stores roles with allow/deny
//! permission lists and evaluates checks against them.
//!
//! Implements requirements R5 (RBAC Permission Enforcement) and
//! R10 (Admin-Only Agent Management).

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::RwLock;

use crate::agent_config::AgentRoleConfig;
use crate::agent_registry::AgentRegistry;

// ── Permission Model ───────────────────────────────────────────────

/// A permission that can be granted or denied to an agent role.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Permission to invoke a specific tool by name.
    Tool(String),
    /// Permission to delegate to a specific agent by ID.
    /// Used by check_delegation for inter-agent communication.
    #[allow(dead_code)]
    Agent(String),
    /// Wildcard: permission to invoke all tools.
    AllTools,
    /// Wildcard: permission to delegate to all agents.
    AllAgents,
}

/// A role with allow and deny permission lists.
#[derive(Debug, Clone)]
pub struct Role {
    /// Human-readable role name for logging and diagnostics.
    #[allow(dead_code)]
    pub name: String,
    pub allow: HashSet<Permission>,
    pub deny: HashSet<Permission>,
}

/// Error returned when an access check is denied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessDenied {
    pub agent_id: String,
    pub permission: String,
    pub reason: String,
}

impl fmt::Display for AccessDenied {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "access denied for agent '{}': {} — {}",
            self.agent_id, self.permission, self.reason
        )
    }
}

impl std::error::Error for AccessDenied {}

// ── AccessControl ──────────────────────────────────────────────────

/// Self-contained access control store. Manages roles keyed by agent ID
/// and evaluates permission checks against them.
#[derive(Debug, Default)]
pub struct AccessControl {
    roles: HashMap<String, Role>,
}

impl AccessControl {
    pub fn new() -> Self {
        Self {
            roles: HashMap::new(),
        }
    }

    /// Add or replace a role for the given agent ID.
    pub fn add_role(&mut self, agent_id: &str, role: Role) {
        self.roles.insert(agent_id.to_string(), role);
    }

    /// Remove a role by agent ID.
    pub fn remove_role(&mut self, agent_id: &str) {
        self.roles.remove(agent_id);
    }

    /// Check whether `agent_id` holds the given permission.
    ///
    /// Evaluation order:
    /// 1. If the permission is in the deny list → Denied.
    /// 2. If the permission is in the allow list → Allowed.
    /// 3. If AllTools is in the allow list and the permission is a Tool → Allowed.
    /// 4. If AllAgents is in the allow list and the permission is an Agent → Allowed.
    /// 5. Otherwise → Denied.
    pub fn check(&self, agent_id: &str, permission: &Permission) -> Result<(), AccessDenied> {
        let role = self.roles.get(agent_id).ok_or_else(|| AccessDenied {
            agent_id: agent_id.to_string(),
            permission: format!("{:?}", permission),
            reason: "no role registered".to_string(),
        })?;

        // Deny list takes precedence.
        if role.deny.contains(permission) {
            return Err(AccessDenied {
                agent_id: agent_id.to_string(),
                permission: format!("{:?}", permission),
                reason: "explicitly denied".to_string(),
            });
        }

        // Exact match in allow list.
        if role.allow.contains(permission) {
            return Ok(());
        }

        // Wildcard checks.
        match permission {
            Permission::Tool(_) if role.allow.contains(&Permission::AllTools) => Ok(()),
            Permission::Agent(_) if role.allow.contains(&Permission::AllAgents) => Ok(()),
            _ => Err(AccessDenied {
                agent_id: agent_id.to_string(),
                permission: format!("{:?}", permission),
                reason: "not in allow list".to_string(),
            }),
        }
    }
}

// ── System Tools ───────────────────────────────────────────────────

/// The agent management, scheduled task, and filesystem tool names that are admin-only.
pub const SYSTEM_TOOLS: &[&str] = &[
    "agent_create",
    "agent_start",
    "agent_stop",
    "agent_delete",
    "agent_list",
    "agent_configure",
    "task_list",
    "task_create",
    "task_cancel",
    "task_delete",
    "fs_list",
    "fs_read",
    "fs_search",
    "fs_pwd",
    "fs_tree",
];

// ── RbacBridge ─────────────────────────────────────────────────────

/// Central RBAC bridge for the multi-agent isolation system.
///
/// Wraps an `AccessControl` behind a `RwLock` so it can be shared
/// across async tasks. Provides high-level methods for registering
/// agents, checking permissions, and rebuilding state on startup.
pub struct RbacBridge {
    access_control: RwLock<AccessControl>,
}

impl Default for RbacBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl RbacBridge {
    pub fn new() -> Self {
        Self {
            access_control: RwLock::new(AccessControl::new()),
        }
    }

    /// Register the system agent with admin role (AllTools + AllAgents).
    pub fn register_system_agent(&self, agent_id: &str) {
        let role = Role {
            name: format!("{}-admin", agent_id),
            allow: [Permission::AllTools, Permission::AllAgents]
                .into_iter()
                .collect(),
            deny: HashSet::new(),
        };
        let mut ac = self.access_control.write().expect("lock poisoned");
        ac.add_role(agent_id, role);
    }

    /// Register a user agent with its configured role.
    ///
    /// Automatically strips system tool permissions from the allow list
    /// and returns the names of any stripped permissions (also logs warnings).
    pub fn register_agent(&self, agent_id: &str, role_config: &AgentRoleConfig) -> Vec<String> {
        let mut stripped = Vec::new();

        let mut allow = HashSet::new();
        for entry in &role_config.allow {
            if SYSTEM_TOOLS.contains(&entry.as_str()) {
                tracing::warn!(
                    agent_id = agent_id,
                    tool = entry.as_str(),
                    "stripped system permission from user agent role"
                );
                stripped.push(entry.clone());
            } else {
                allow.insert(Permission::Tool(entry.clone()));
            }
        }

        let deny: HashSet<Permission> = role_config
            .deny
            .iter()
            .map(|d| Permission::Tool(d.clone()))
            .collect();

        let role = Role {
            name: format!("{}-role", agent_id),
            allow,
            deny,
        };

        let mut ac = self.access_control.write().expect("lock poisoned");
        ac.add_role(agent_id, role);

        stripped
    }

    /// Check if an agent can invoke a specific tool.
    pub fn check_tool(&self, agent_id: &str, tool_name: &str) -> Result<(), AccessDenied> {
        let ac = self.access_control.read().expect("lock poisoned");
        ac.check(agent_id, &Permission::Tool(tool_name.to_string()))
    }

    /// Check if an agent can delegate to another agent.
    /// Used for inter-agent delegation authorization.
    #[allow(dead_code)]
    pub fn check_delegation(&self, caller_id: &str, target_id: &str) -> Result<(), AccessDenied> {
        let ac = self.access_control.read().expect("lock poisoned");
        ac.check(caller_id, &Permission::Agent(target_id.to_string()))
    }

    /// Remove an agent's role (on delete).
    pub fn remove_agent(&self, agent_id: &str) {
        let mut ac = self.access_control.write().expect("lock poisoned");
        ac.remove_role(agent_id);
    }

    /// Rebuild the AccessControl from the registry (on startup).
    ///
    /// Iterates all agents in the registry and re-registers their roles.
    /// The system agent gets the admin role; user agents get their
    /// configured roles with system permissions stripped.
    pub fn rebuild_from_registry(&self, registry: &AgentRegistry) {
        let mut ac = self.access_control.write().expect("lock poisoned");
        *ac = AccessControl::new();

        for (id, record) in registry.list() {
            if registry.is_system_agent(&id) {
                let role = Role {
                    name: format!("{}-admin", id),
                    allow: [Permission::AllTools, Permission::AllAgents]
                        .into_iter()
                        .collect(),
                    deny: HashSet::new(),
                };
                ac.add_role(&id, role);
            } else {
                // Build user role, stripping system tools.
                let mut allow = HashSet::new();
                for entry in &record.config.role.allow {
                    if !SYSTEM_TOOLS.contains(&entry.as_str()) {
                        allow.insert(Permission::Tool(entry.clone()));
                    }
                }
                let deny: HashSet<Permission> = record
                    .config
                    .role
                    .deny
                    .iter()
                    .map(|d| Permission::Tool(d.clone()))
                    .collect();

                let role = Role {
                    name: format!("{}-role", id),
                    allow,
                    deny,
                };
                ac.add_role(&id, role);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_config::{AgentConfig, AgentRoleConfig, AgentType};

    fn make_config(id: &str) -> AgentConfig {
        AgentConfig {
            id: id.to_string(),
            name: format!("Agent {}", id),
            description: "test agent".to_string(),
            agent_type: AgentType::Llm,
            model: "test/model".to_string(),
            api_key_env: "TEST_KEY".to_string(),
            instruction: "do stuff".to_string(),
            tools: vec![],
            action_nodes: vec![],
            workflow_edges: vec![],
            sub_agents: vec![],
            role: AgentRoleConfig {
                allow: vec![],
                deny: vec![],
            },
            channel_bindings: vec![],
            auto_start: false,
            temperature: None,
            max_output_tokens: None,
            model_override: None,
        }
    }

    /// Task 4.10: system agent has AllTools access
    #[test]
    fn system_agent_has_all_tools_access() {
        let bridge = RbacBridge::new();
        bridge.register_system_agent("system");

        // System agent should be able to invoke any tool, including system tools.
        for tool in SYSTEM_TOOLS {
            assert!(
                bridge.check_tool("system", tool).is_ok(),
                "system agent should have access to system tool '{}'",
                tool
            );
        }

        // Also arbitrary user tools.
        assert!(bridge.check_tool("system", "web_search").is_ok());
        assert!(bridge.check_tool("system", "code_exec").is_ok());

        // And delegation to any agent.
        assert!(bridge.check_delegation("system", "research").is_ok());
        assert!(bridge.check_delegation("system", "writer").is_ok());
    }

    /// Task 4.11: stripped system permissions are logged
    #[test]
    fn stripped_system_permissions_are_logged() {
        let bridge = RbacBridge::new();

        // Register a user agent that tries to include system tools.
        let role_config = AgentRoleConfig {
            allow: vec![
                "web_search".to_string(),
                "agent_create".to_string(),
                "agent_delete".to_string(),
                "code_exec".to_string(),
            ],
            deny: vec![],
        };

        let stripped = bridge.register_agent("research", &role_config);

        // The two system tools should have been stripped.
        assert_eq!(stripped.len(), 2);
        assert!(stripped.contains(&"agent_create".to_string()));
        assert!(stripped.contains(&"agent_delete".to_string()));

        // The user agent should NOT have access to the stripped tools.
        assert!(bridge.check_tool("research", "agent_create").is_err());
        assert!(bridge.check_tool("research", "agent_delete").is_err());

        // But should have access to the non-system tools.
        assert!(bridge.check_tool("research", "web_search").is_ok());
        assert!(bridge.check_tool("research", "code_exec").is_ok());
    }

    #[test]
    fn remove_agent_removes_role() {
        let bridge = RbacBridge::new();
        let role_config = AgentRoleConfig {
            allow: vec!["web_search".to_string()],
            deny: vec![],
        };
        bridge.register_agent("research", &role_config);
        assert!(bridge.check_tool("research", "web_search").is_ok());

        bridge.remove_agent("research");
        assert!(bridge.check_tool("research", "web_search").is_err());
    }

    #[test]
    fn check_delegation_works() {
        let bridge = RbacBridge::new();
        // User agent with no agent permissions.
        let role_config = AgentRoleConfig {
            allow: vec!["web_search".to_string()],
            deny: vec![],
        };
        bridge.register_agent("research", &role_config);

        // Should not be able to delegate (no Agent permission).
        assert!(bridge.check_delegation("research", "writer").is_err());
    }

    #[test]
    fn rebuild_from_registry_restores_roles() {
        let tmp = tempfile::TempDir::new().unwrap();
        let registry = AgentRegistry::new(tmp.path().join("registry"));

        // Register system agent.
        let mut sys_config = make_config("system");
        sys_config.role.allow = vec!["*".to_string()];
        registry.register_system_agent(sys_config).unwrap();

        // Create a user agent.
        let mut user_config = make_config("research");
        user_config.role.allow = vec!["web_search".to_string(), "code_exec".to_string()];
        registry.create_agent(user_config).unwrap();

        // Build bridge from registry.
        let bridge = RbacBridge::new();
        bridge.rebuild_from_registry(&registry);

        // System agent should have AllTools.
        assert!(bridge.check_tool("system", "agent_create").is_ok());
        assert!(bridge.check_tool("system", "web_search").is_ok());

        // User agent should have its configured tools.
        assert!(bridge.check_tool("research", "web_search").is_ok());
        assert!(bridge.check_tool("research", "code_exec").is_ok());

        // User agent should NOT have system tools.
        assert!(bridge.check_tool("research", "agent_create").is_err());
    }

    #[test]
    fn deny_list_takes_precedence() {
        let bridge = RbacBridge::new();
        let role_config = AgentRoleConfig {
            allow: vec!["web_search".to_string()],
            deny: vec!["web_search".to_string()],
        };
        bridge.register_agent("research", &role_config);

        // Deny should override allow.
        assert!(bridge.check_tool("research", "web_search").is_err());
    }
}
