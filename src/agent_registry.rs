//! Agent registry with disk persistence for the multi-agent isolation system.
//!
//! Manages agent records (config + lifecycle state), enforces ID uniqueness,
//! system agent singleton, lifecycle state machine transitions, and persists
//! each agent as a JSON file on disk.

use std::path::PathBuf;
use std::sync::RwLock;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::agent_config::{AgentConfig, LifecycleState};

/// A record stored in the registry for each agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRecord {
    pub config: AgentConfig,
    pub state: LifecycleState,
    pub port: Option<u16>,
    pub pid: Option<u32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Concurrent agent registry backed by DashMap with JSON file persistence.
pub struct AgentRegistry {
    agents: DashMap<String, AgentRecord>,
    system_agent_id: RwLock<Option<String>>,
    persist_dir: PathBuf,
}

impl std::fmt::Debug for AgentRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRegistry")
            .field("persist_dir", &self.persist_dir)
            .field("agent_count", &self.agents.len())
            .finish()
    }
}

impl AgentRegistry {
    /// Create a new registry that persists agent JSON files under `persist_dir`.
    pub fn new(persist_dir: PathBuf) -> Self {
        Self {
            agents: DashMap::new(),
            system_agent_id: RwLock::new(None),
            persist_dir,
        }
    }

    /// Create a new agent. Validates ID uniqueness, sets state to Created,
    /// and persists the record to disk.
    pub fn create_agent(&self, config: AgentConfig) -> Result<String> {
        let id = config.id.clone();

        // Check uniqueness — DashMap entry API gives us atomic insert-if-absent.
        use dashmap::mapref::entry::Entry;
        match self.agents.entry(id.clone()) {
            Entry::Occupied(_) => {
                bail!("agent with id '{}' already exists", id);
            }
            Entry::Vacant(slot) => {
                let now = Utc::now();
                let record = AgentRecord {
                    config,
                    state: LifecycleState::Created,
                    port: None,
                    pid: None,
                    created_at: now,
                    updated_at: now,
                };
                self.persist(&id, &record)?;
                slot.insert(record);
            }
        }
        Ok(id)
    }

    /// Register the system agent. Enforces singleton — only one system agent
    /// may exist. Sets the admin role on the config.
    pub fn register_system_agent(&self, mut config: AgentConfig) -> Result<()> {
        let mut guard = self
            .system_agent_id
            .write()
            .map_err(|e| anyhow::anyhow!("lock poisoned: {e}"))?;

        if guard.is_some() {
            bail!("a system agent already exists");
        }

        // Ensure admin role: allow everything, deny nothing.
        config.role.allow = vec!["*".to_string()];
        config.role.deny = Vec::new();

        let id = config.id.clone();
        let now = Utc::now();
        let record = AgentRecord {
            config,
            state: LifecycleState::Created,
            port: None,
            pid: None,
            created_at: now,
            updated_at: now,
        };
        self.persist(&id, &record)?;
        self.agents.insert(id.clone(), record);
        *guard = Some(id);
        Ok(())
    }

    /// Transition an agent to a new lifecycle state.
    /// Validates the transition against the state machine.
    pub fn transition(&self, id: &str, new_state: LifecycleState) -> Result<()> {
        let mut entry = self
            .agents
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("agent '{}' not found", id))?;

        if !is_valid_transition(&entry.state, &new_state) {
            bail!(
                "invalid state transition for '{}': {:?} → {:?}",
                id,
                entry.state,
                new_state
            );
        }

        entry.state = new_state;
        entry.updated_at = Utc::now();
        self.persist(id, &entry)?;
        Ok(())
    }

    /// Update an agent's config and persist.
    pub fn update_config(&self, id: &str, config: AgentConfig) -> Result<()> {
        let mut entry = self
            .agents
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("agent '{}' not found", id))?;

        if config.id != id {
            bail!("config id '{}' does not match agent id '{}'", config.id, id);
        }

        entry.config = config;
        entry.updated_at = Utc::now();
        self.persist(id, &entry)?;
        Ok(())
    }

    /// Delete an agent. Requires Stopped or Error state.
    /// Archives the workspace directory and removes the persisted config file.
    pub fn delete(&self, id: &str) -> Result<AgentRecord> {
        // Check state before removing.
        {
            let entry = self
                .agents
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("agent '{}' not found", id))?;
            match &entry.state {
                LifecycleState::Stopped | LifecycleState::Error { .. } => {}
                other => bail!(
                    "cannot delete agent '{}' in state {:?} — must be Stopped or Error",
                    id,
                    other
                ),
            }
        }

        let (_, record) = self
            .agents
            .remove(id)
            .ok_or_else(|| anyhow::anyhow!("agent '{}' disappeared during delete", id))?;

        // Archive workspace: move agent dir to .archive/{id}-{timestamp}/
        self.archive_workspace(id);

        // Remove persisted JSON file.
        let _ = self.remove_persisted(id);

        Ok(record)
    }

    /// Load all persisted agent JSON files from disk into the registry.
    /// Returns the number of agents loaded.
    pub fn load_from_disk(&self) -> Result<usize> {
        if !self.persist_dir.exists() {
            return Ok(0);
        }

        let mut count = 0usize;
        let entries = std::fs::read_dir(&self.persist_dir)
            .with_context(|| format!("reading persist dir {:?}", self.persist_dir))?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let data =
                std::fs::read_to_string(&path).with_context(|| format!("reading {:?}", path))?;
            let record: AgentRecord =
                serde_json::from_str(&data).with_context(|| format!("parsing {:?}", path))?;

            let id = record.config.id.clone();
            self.agents.insert(id, record);
            count += 1;
        }

        Ok(count)
    }

    /// Return all agents with their current state.
    pub fn list(&self) -> Vec<(String, AgentRecord)> {
        self.agents
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }

    /// Get a reference to an agent record.
    pub fn get(&self, id: &str) -> Option<dashmap::mapref::one::Ref<'_, String, AgentRecord>> {
        self.agents.get(id)
    }

    /// Check whether the given id is the system agent.
    pub fn is_system_agent(&self, id: &str) -> bool {
        self.system_agent_id
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(|s| s == id))
            .unwrap_or(false)
    }

    // ── Private helpers ────────────────────────────────────────────

    fn persist(&self, id: &str, record: &AgentRecord) -> Result<()> {
        std::fs::create_dir_all(&self.persist_dir)
            .with_context(|| format!("creating persist dir {:?}", self.persist_dir))?;
        let path = self.persist_dir.join(format!("{}.json", id));
        let data = serde_json::to_string_pretty(record)?;
        std::fs::write(&path, data).with_context(|| format!("writing {:?}", path))?;
        Ok(())
    }

    fn remove_persisted(&self, id: &str) -> Result<()> {
        let path = self.persist_dir.join(format!("{}.json", id));
        if path.exists() {
            std::fs::remove_file(&path).with_context(|| format!("removing {:?}", path))?;
        }
        Ok(())
    }

    fn archive_workspace(&self, id: &str) {
        // Archive dir lives at {persist_dir}/../.archive/{id}-{timestamp}/
        let archive_root = self.persist_dir.join("..").join(".archive");
        let timestamp = Utc::now().format("%Y%m%dT%H%M%S");
        let dest = archive_root.join(format!("{}-{}", id, timestamp));

        // The agent workspace is at {persist_dir}/../{id}/
        let agent_dir = self.persist_dir.join("..").join(id);
        if agent_dir.exists() {
            let _ = std::fs::create_dir_all(&archive_root);
            let _ = std::fs::rename(&agent_dir, &dest);
        }
    }
}

/// Validate lifecycle state machine transitions.
///
/// Valid transitions:
/// - Created → Starting
/// - Starting → Running
/// - Starting → Error
/// - Running → Stopping
/// - Running → Error
/// - Stopping → Stopped
/// - Stopped → Starting
/// - Error → Starting
pub fn is_valid_transition(from: &LifecycleState, to: &LifecycleState) -> bool {
    matches!(
        (from, to),
        (LifecycleState::Created, LifecycleState::Starting)
            | (LifecycleState::Starting, LifecycleState::Running)
            | (LifecycleState::Starting, LifecycleState::Error { .. })
            | (LifecycleState::Running, LifecycleState::Stopping)
            | (LifecycleState::Running, LifecycleState::Error { .. })
            | (LifecycleState::Stopping, LifecycleState::Stopped)
            | (LifecycleState::Stopped, LifecycleState::Starting)
            | (LifecycleState::Error { .. }, LifecycleState::Starting)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_config::{AgentRoleConfig, AgentType};
    use tempfile::TempDir;

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

    /// Task 3.13: persist and load round-trip
    #[test]
    fn persist_and_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let persist_dir = tmp.path().join("registry");

        // Create registry and add agents.
        let reg = AgentRegistry::new(persist_dir.clone());
        reg.create_agent(make_config("alpha")).unwrap();
        reg.create_agent(make_config("beta")).unwrap();

        // Transition alpha to Starting then Running.
        reg.transition("alpha", LifecycleState::Starting).unwrap();
        reg.transition("alpha", LifecycleState::Running).unwrap();

        // Create a fresh registry from the same dir and load.
        let reg2 = AgentRegistry::new(persist_dir);
        let loaded = reg2.load_from_disk().unwrap();
        assert_eq!(loaded, 2);

        // Verify records survived.
        let alpha = reg2.get("alpha").unwrap();
        assert_eq!(alpha.config.id, "alpha");
        assert_eq!(alpha.state, LifecycleState::Running);

        let beta = reg2.get("beta").unwrap();
        assert_eq!(beta.config.id, "beta");
        assert_eq!(beta.state, LifecycleState::Created);
    }
}
