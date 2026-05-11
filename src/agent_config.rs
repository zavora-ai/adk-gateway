//! Agent configuration types for the multi-agent isolation system.
//!
//! Defines `AgentConfig`, `AgentType`, `LifecycleState`, and supporting types
//! used by the AgentRegistry, ProcessManager, and codegen pipeline.

use serde::{Deserialize, Serialize};

use crate::config::CategoryConfig;

/// How an agent's model configuration relates to the global CategoryConfig.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum AgentModelOverride {
    /// Override specific categories; unset categories inherit from global.
    Partial(CategoryConfig),
    /// Replace the entire global config for this agent.
    Full(CategoryConfig),
}

/// The declarative definition of a User Agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentConfig {
    /// Unique agent identifier (kebab-case).
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Agent description for AgentCard.
    pub description: String,
    /// Agent type: Llm, Sequential, Parallel, Loop, Router, Graph.
    pub agent_type: AgentType,
    /// Model identifier (e.g., "anthropic/claude-sonnet-4").
    /// Legacy field — kept for backward compat. New agents use `model_override`.
    pub model: String,
    /// Environment variable name containing the API key.
    /// Deprecated — auto-resolved from model provider prefix. Kept for backward compat.
    #[serde(default)]
    pub api_key_env: String,
    /// Agent system instruction.
    pub instruction: String,
    /// Allowed tool names.
    pub tools: Vec<String>,
    /// Action nodes for the agent's workflow graph.
    pub action_nodes: Vec<ActionNodeEntry>,
    /// Workflow edges connecting agents and action nodes.
    pub workflow_edges: Vec<WorkflowEdge>,
    /// Sub-agent IDs for container agents (sequential, parallel, loop).
    pub sub_agents: Vec<String>,
    /// RBAC role configuration.
    pub role: AgentRoleConfig,
    /// Channel routing bindings.
    pub channel_bindings: Vec<ChannelBinding>,
    /// Whether to auto-start on gateway boot.
    pub auto_start: bool,
    /// Optional temperature override.
    pub temperature: Option<f32>,
    /// Optional max output tokens override.
    pub max_output_tokens: Option<i32>,
    /// Optional category-aware model override (R13/R15).
    /// When None, the agent inherits the global CategoryConfig.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_override: Option<AgentModelOverride>,
}

impl AgentConfig {
    /// Resolve the effective model for a given category, considering the override mode.
    /// Falls back to the global config when the agent doesn't override a category.
    pub fn resolve_model<'a>(
        &'a self,
        category: &str,
        global: &'a CategoryConfig,
    ) -> Option<&'a str> {
        match &self.model_override {
            None => {
                // Mode 1: Inherit all from global
                global.resolve(category)
            }
            Some(AgentModelOverride::Partial(overrides)) => {
                // Mode 2: Try agent override first, fall back to global
                overrides
                    .resolve(category)
                    .or_else(|| global.resolve(category))
            }
            Some(AgentModelOverride::Full(full)) => {
                // Mode 3: Use agent's config only
                full.resolve(category)
            }
        }
    }

    /// Resolve the API key env var from the model's provider prefix.
    /// Falls back to the legacy `api_key_env` field if set.
    pub fn resolve_api_key_env(&self) -> &str {
        if !self.api_key_env.is_empty() {
            return &self.api_key_env;
        }
        // Auto-resolve from model ID provider prefix
        let model_id = match &self.model_override {
            Some(AgentModelOverride::Partial(c)) | Some(AgentModelOverride::Full(c)) => c.primary(),
            None => &self.model,
        };
        crate::model_factory::resolve_api_key_env(model_id)
    }
}

/// RBAC role configuration for an agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentRoleConfig {
    /// Tool/agent names to allow.
    pub allow: Vec<String>,
    /// Tool/agent names to deny.
    pub deny: Vec<String>,
}

/// A channel routing binding — maps a channel to this agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelBinding {
    /// Channel type: "telegram", "slack", "webhook".
    pub channel_type: String,
    /// Specific account or None for all.
    pub account_id: Option<String>,
    /// Optional peer/group filter.
    pub peer_filter: Option<String>,
}

/// An action node entry in the agent's workflow graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionNodeEntry {
    /// Node identifier.
    pub id: String,
    /// Serialized ActionNodeConfig.
    pub config: serde_json::Value,
}

/// A directed edge in the agent's workflow graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkflowEdge {
    /// Source node ID.
    pub from: String,
    /// Target node ID.
    pub to: String,
    /// Optional condition expression.
    pub condition: Option<String>,
}

/// Lifecycle state of an agent in the registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LifecycleState {
    Created,
    Starting,
    Running,
    Stopping,
    Stopped,
    Error { message: String },
}

/// Agent type matching adk-studio's supported agent types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AgentType {
    Llm,
    Sequential,
    Parallel,
    Loop,
    Router,
    Graph,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_state_serializes_simple_variants() {
        for (state, expected) in [
            (LifecycleState::Created, r#""Created""#),
            (LifecycleState::Starting, r#""Starting""#),
            (LifecycleState::Running, r#""Running""#),
            (LifecycleState::Stopping, r#""Stopping""#),
            (LifecycleState::Stopped, r#""Stopped""#),
        ] {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(json, expected);
            let parsed: LifecycleState = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, state);
        }
    }

    #[test]
    fn lifecycle_state_error_round_trip() {
        let state = LifecycleState::Error {
            message: "compile failed".to_string(),
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("compile failed"));
        let parsed: LifecycleState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, state);
    }

    #[test]
    fn agent_type_round_trip() {
        for variant in [
            AgentType::Llm,
            AgentType::Sequential,
            AgentType::Parallel,
            AgentType::Loop,
            AgentType::Router,
            AgentType::Graph,
        ] {
            let json = serde_json::to_string(&variant).unwrap();
            let parsed: AgentType = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed, variant);
        }
    }
}
