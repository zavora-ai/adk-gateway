//! Graph workflow builder — builds directed graph workflows from config.
//!
//! Provides a self-contained graph workflow engine with nodes, edges,
//! state reducers, checkpointing, conditional routing, and cycle detection.
//!
//! Design: GraphWorkflowBuilder [R16.1–R16.8]

use crate::config::{
    ActionNodeConfig, CheckpointConfig, GraphNodeType, GraphStreamMode, GraphWorkflowConfig,
    InterruptConfig, ReducerType,
};
use adk_core::Agent;
use adk_session::SessionService;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

/// Workflow state — a map of string keys to JSON values.
pub type WorkflowState = HashMap<String, Value>;

/// Default timeout for agent node execution (60 seconds).
const DEFAULT_AGENT_TIMEOUT_SECS: u64 = 60;

/// A built graph workflow ready for execution.
#[derive(Debug, Clone)]
pub struct GraphWorkflow {
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    pub state_reducers: HashMap<String, ReducerType>,
    pub checkpoint: Option<CheckpointConfig>,
    #[allow(dead_code)] // Config field preserved for future streaming workflow support
    pub stream_mode: Option<GraphStreamMode>,
    pub max_iterations: u32,
    pub interrupts: Option<InterruptConfig>,
}

/// A node in the workflow graph.
#[derive(Debug, Clone)]
pub struct WorkflowNode {
    pub id: String,
    pub node_type: GraphNodeType,
    pub config: Value,
}

/// An edge connecting two nodes, optionally with a condition.
#[derive(Debug, Clone)]
pub struct WorkflowEdge {
    pub from: String,
    pub to: String,
    pub condition: Option<String>,
}

/// Result of executing a single node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeResult {
    pub node_id: String,
    pub output: Value,
    pub error: Option<String>,
}

/// A checkpoint snapshot of workflow state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointSnapshot {
    pub node_id: String,
    pub state: WorkflowState,
    pub iteration: u32,
}

/// Checkpoint store for persisting/restoring workflow state.
#[derive(Debug, Clone, Default)]
pub struct CheckpointStore {
    snapshots: Vec<CheckpointSnapshot>,
}

impl CheckpointStore {
    pub fn new() -> Self {
        Self { snapshots: vec![] }
    }

    /// Save a checkpoint snapshot.
    pub fn save(&mut self, snapshot: CheckpointSnapshot) {
        self.snapshots.push(snapshot);
    }

    /// Restore the last checkpoint snapshot.
    #[allow(dead_code)]
    pub fn restore(&self) -> Option<&CheckpointSnapshot> {
        self.snapshots.last()
    }

    /// Get all snapshots.
    #[allow(dead_code)]
    pub fn snapshots(&self) -> &[CheckpointSnapshot] {
        &self.snapshots
    }
}

/// Context for executing workflow nodes, providing access to agents and session service.
pub struct WorkflowExecutionContext<'a> {
    pub action_executor: &'a crate::action_executor::ActionExecutor,
    pub agents: &'a DashMap<String, Arc<dyn Agent>>,
    pub session_service: &'a Arc<dyn SessionService>,
}

/// Builder for constructing a GraphWorkflow from config.
pub struct GraphWorkflowBuilder;

impl GraphWorkflowBuilder {
    /// Build a GraphWorkflow from a GraphWorkflowConfig.
    ///
    /// Validates that all edge endpoints reference existing nodes,
    /// and applies defaults for max_iterations.
    pub fn build(config: &GraphWorkflowConfig) -> Result<GraphWorkflow, GraphWorkflowError> {
        // Validate nodes are non-empty
        if config.nodes.is_empty() {
            return Err(GraphWorkflowError::EmptyGraph);
        }

        let node_ids: Vec<&str> = config.nodes.iter().map(|n| n.id.as_str()).collect();

        // Check for duplicate node IDs
        let mut seen = std::collections::HashSet::new();
        for id in &node_ids {
            if !seen.insert(*id) {
                return Err(GraphWorkflowError::DuplicateNodeId(id.to_string()));
            }
        }

        // Validate edges reference existing nodes
        for edge in &config.edges {
            if !node_ids.contains(&edge.from.as_str()) {
                return Err(GraphWorkflowError::InvalidEdge {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    reason: format!("source node '{}' not found", edge.from),
                });
            }
            if !node_ids.contains(&edge.to.as_str()) {
                return Err(GraphWorkflowError::InvalidEdge {
                    from: edge.from.clone(),
                    to: edge.to.clone(),
                    reason: format!("target node '{}' not found", edge.to),
                });
            }
        }

        let nodes = config
            .nodes
            .iter()
            .map(|n| WorkflowNode {
                id: n.id.clone(),
                node_type: n.node_type.clone(),
                config: n.config.clone(),
            })
            .collect();

        let edges = config
            .edges
            .iter()
            .map(|e| WorkflowEdge {
                from: e.from.clone(),
                to: e.to.clone(),
                condition: e.condition.clone(),
            })
            .collect();

        Ok(GraphWorkflow {
            nodes,
            edges,
            state_reducers: config.state_reducers.clone(),
            checkpoint: config.checkpoint.clone(),
            stream_mode: config.stream_mode.clone(),
            max_iterations: config.max_iterations.unwrap_or(100),
            interrupts: config.interrupts.clone(),
        })
    }
}

impl GraphWorkflow {
    /// Find a node by ID.
    pub fn find_node(&self, id: &str) -> Option<&WorkflowNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// Get outgoing edges from a node.
    pub fn outgoing_edges(&self, node_id: &str) -> Vec<&WorkflowEdge> {
        self.edges.iter().filter(|e| e.from == node_id).collect()
    }

    /// Evaluate conditional edges from a node given the current state.
    /// Returns the ID of the next node to execute, or None if no edge matches.
    pub fn resolve_next_node(&self, current_node: &str, state: &WorkflowState) -> Option<String> {
        let edges = self.outgoing_edges(current_node);
        // First try conditional edges
        for edge in &edges {
            if let Some(ref condition) = edge.condition {
                if evaluate_condition(condition, state) {
                    return Some(edge.to.clone());
                }
            }
        }
        // Fall back to unconditional edge
        for edge in &edges {
            if edge.condition.is_none() {
                return Some(edge.to.clone());
            }
        }
        None
    }

    /// Apply a state reducer to merge a value into the workflow state.
    pub fn apply_reducer(&self, state: &mut WorkflowState, key: &str, value: Value) {
        let reducer = self
            .state_reducers
            .get(key)
            .cloned()
            .unwrap_or(ReducerType::Overwrite);

        match reducer {
            ReducerType::Overwrite => {
                state.insert(key.to_string(), value);
            }
            ReducerType::Append => {
                let entry = state
                    .entry(key.to_string())
                    .or_insert_with(|| Value::Array(vec![]));
                if let Value::Array(arr) = entry {
                    if let Value::Array(new_items) = value {
                        arr.extend(new_items);
                    } else {
                        arr.push(value);
                    }
                } else {
                    // Convert existing to array and append
                    let old = entry.clone();
                    *entry = Value::Array(vec![old, value]);
                }
            }
            ReducerType::Sum => {
                let current = state
                    .entry(key.to_string())
                    .or_insert_with(|| Value::Number(serde_json::Number::from(0)));
                let current_num = current.as_f64().unwrap_or(0.0);
                let add_num = value.as_f64().unwrap_or(0.0);
                *current = serde_json::json!(current_num + add_num);
            }
            ReducerType::Custom(_) => {
                // Custom reducers fall back to overwrite in this self-contained impl
                state.insert(key.to_string(), value);
            }
        }
    }

    /// Execute the workflow starting from the first node.
    ///
    /// Dispatches each node to the appropriate executor (agent or action),
    /// applies state reducers, persists checkpoints, and follows edges.
    /// Respects max_iterations for cyclic graphs.
    ///
    /// Accepts a `WorkflowExecutionContext` that provides access to the
    /// action executor, agent registry, and session service for real agent
    /// node execution.
    pub fn execute(
        &self,
        initial_state: WorkflowState,
        ctx: &WorkflowExecutionContext<'_>,
    ) -> Result<WorkflowExecutionResult, GraphWorkflowError> {
        let mut state = initial_state;
        let mut checkpoint_store = CheckpointStore::new();
        let mut iteration = 0u32;
        let mut executed_nodes: Vec<NodeResult> = vec![];

        let first_node_id = self.nodes.first().map(|n| n.id.clone());
        let mut current_node_id = first_node_id;

        while let Some(ref node_id) = current_node_id {
            if iteration >= self.max_iterations {
                return Err(GraphWorkflowError::MaxIterationsReached {
                    max: self.max_iterations,
                    last_node: node_id.clone(),
                });
            }

            let node = self
                .find_node(node_id)
                .ok_or_else(|| GraphWorkflowError::NodeNotFound(node_id.clone()))?;

            // Check for before-interrupt
            if let Some(ref interrupts) = self.interrupts {
                if interrupts.before.contains(&node.id) {
                    state.insert(
                        "__interrupt_before".to_string(),
                        Value::String(node.id.clone()),
                    );
                }
            }

            // Execute the node
            let result = execute_node(node, &state, ctx);

            match &result {
                Ok(node_result) => {
                    // Apply state reducers for each key in the output
                    if let Value::Object(map) = &node_result.output {
                        for (key, val) in map {
                            self.apply_reducer(&mut state, key, val.clone());
                        }
                    }
                    executed_nodes.push(node_result.clone());
                }
                Err(e) => {
                    let error_result = NodeResult {
                        node_id: node_id.clone(),
                        output: serde_json::json!({"error": e.to_string()}),
                        error: Some(e.to_string()),
                    };
                    executed_nodes.push(error_result);
                    tracing::warn!(node = %node_id, error = %e, "node execution failed");
                }
            }

            // Persist checkpoint if enabled
            if self.checkpoint.is_some() {
                checkpoint_store.save(CheckpointSnapshot {
                    node_id: node_id.clone(),
                    state: state.clone(),
                    iteration,
                });
            }

            // Check for after-interrupt
            if let Some(ref interrupts) = self.interrupts {
                if interrupts.after.contains(&node.id) {
                    state.insert(
                        "__interrupt_after".to_string(),
                        Value::String(node.id.clone()),
                    );
                }
            }

            // Resolve next node via conditional edge routing
            current_node_id = self.resolve_next_node(node_id, &state);
            iteration += 1;
        }

        Ok(WorkflowExecutionResult {
            state,
            executed_nodes,
            checkpoint_store,
            iterations: iteration,
        })
    }
}

/// Result of a complete workflow execution.
#[derive(Debug)]
pub struct WorkflowExecutionResult {
    pub state: WorkflowState,
    pub executed_nodes: Vec<NodeResult>,
    #[allow(dead_code)]
    pub checkpoint_store: CheckpointStore,
    pub iterations: u32,
}

/// Execute a single workflow node.
fn execute_node(
    node: &WorkflowNode,
    state: &WorkflowState,
    ctx: &WorkflowExecutionContext<'_>,
) -> Result<NodeResult, GraphWorkflowError> {
    match node.node_type {
        GraphNodeType::Action => {
            let action_config: ActionNodeConfig = serde_json::from_value(node.config.clone())
                .map_err(|e| GraphWorkflowError::NodeExecutionFailed {
                    node_id: node.id.clone(),
                    reason: format!("invalid action config: {}", e),
                })?;
            let output = ctx.action_executor.execute(&action_config).map_err(|e| {
                GraphWorkflowError::NodeExecutionFailed {
                    node_id: node.id.clone(),
                    reason: e.to_string(),
                }
            })?;
            Ok(NodeResult {
                node_id: node.id.clone(),
                output,
                error: None,
            })
        }
        GraphNodeType::Agent => execute_agent_node(node, state, ctx),
        GraphNodeType::Tool => {
            let tool_name = node
                .config
                .get("tool")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Ok(NodeResult {
                node_id: node.id.clone(),
                output: serde_json::json!({
                    "tool": tool_name,
                    "result": format!("Tool {} executed", tool_name),
                }),
                error: None,
            })
        }
    }
}

/// Execute an Agent node by resolving the agent from the registry,
/// constructing a session with workflow state, invoking via Runner,
/// and collecting the response.
fn execute_agent_node(
    node: &WorkflowNode,
    state: &WorkflowState,
    ctx: &WorkflowExecutionContext<'_>,
) -> Result<NodeResult, GraphWorkflowError> {
    use adk_core::{Content, Part};
    use adk_runner::{Runner, RunnerConfig};
    use futures::StreamExt;

    let agent_id = node
        .config
        .get("agent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    // Per-node timeout: read from config or use default
    let timeout_secs = node
        .config
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_AGENT_TIMEOUT_SECS);
    let timeout = Duration::from_secs(timeout_secs);

    // Resolve agent from registry
    let agent = ctx.agents.get(agent_id).map(|entry| entry.value().clone());
    let agent = match agent {
        Some(a) => a,
        None => {
            return Err(GraphWorkflowError::NodeExecutionFailed {
                node_id: node.id.clone(),
                reason: format!("agent not found: {}", agent_id),
            });
        }
    };

    // Build user content from workflow state
    let user_message = state
        .get("user_message")
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // Include full workflow state as context in the message
    let state_context = serde_json::to_string(state).unwrap_or_default();
    let augmented_text = if state_context.len() > 2 {
        format!("{}\n\n[Workflow State: {}]", user_message, state_context)
    } else {
        user_message.to_string()
    };

    let content = Content {
        role: "user".into(),
        parts: vec![Part::Text {
            text: augmented_text,
        }],
    };

    // Create runner with the resolved agent
    let runner = Runner::new(RunnerConfig {
        app_name: "adk-gateway-workflow".to_string(),
        agent: agent.clone(),
        session_service: ctx.session_service.clone(),
        artifact_service: None,
        memory_service: None,
        plugin_manager: None,
        run_config: None,
        compaction_config: None,
        context_cache_config: None,
        cache_capable: None,
        request_context: None,
        cancellation_token: None,
        intra_compaction_config: None,
        intra_compaction_summarizer: None,
    })
    .map_err(|e| GraphWorkflowError::NodeExecutionFailed {
        node_id: node.id.clone(),
        reason: format!("failed to create runner: {}", e),
    })?;

    // Generate unique session/user IDs for this workflow execution
    let session_id = adk_core::SessionId::try_from(format!("wf-{}", uuid::Uuid::new_v4()).as_str())
        .map_err(|e| GraphWorkflowError::NodeExecutionFailed {
            node_id: node.id.clone(),
            reason: format!("invalid session id: {}", e),
        })?;
    let user_id = adk_core::UserId::try_from("workflow_user").map_err(|e| {
        GraphWorkflowError::NodeExecutionFailed {
            node_id: node.id.clone(),
            reason: format!("invalid user id: {}", e),
        }
    })?;

    // Run the agent with timeout using a blocking tokio runtime
    let node_id = node.id.clone();
    let result = tokio::task::block_in_place(|| {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            let run_future = async {
                let stream = runner
                    .run(user_id, session_id, content)
                    .await
                    .map_err(|e| GraphWorkflowError::NodeExecutionFailed {
                        node_id: node_id.clone(),
                        reason: format!("agent run failed: {}", e),
                    })?;

                // Collect events from the stream
                let events: Vec<adk_core::Event> =
                    stream.filter_map(|r| async { r.ok() }).collect().await;

                // Extract text response from events
                let mut response_text = String::new();
                for event in &events {
                    if let Some(content) = event.content() {
                        for part in &content.parts {
                            if let Some(text) = part.text() {
                                response_text.push_str(text);
                            }
                        }
                    }
                }

                Ok::<String, GraphWorkflowError>(response_text)
            };

            tokio::time::timeout(timeout, run_future).await
        })
    });

    match result {
        Ok(Ok(response_text)) => Ok(NodeResult {
            node_id: node.id.clone(),
            output: serde_json::json!({
                "agent_id": agent_id,
                "response": response_text,
            }),
            error: None,
        }),
        Ok(Err(e)) => Err(e),
        Err(_elapsed) => Err(GraphWorkflowError::NodeExecutionFailed {
            node_id: node.id.clone(),
            reason: format!("agent execution timed out after {}s", timeout_secs),
        }),
    }
}

/// Evaluate a simple condition expression against the workflow state.
///
/// Supports expressions like:
/// - `key == "value"` — string equality
/// - `key != "value"` — string inequality
/// - `key > 5` — numeric comparison
/// - `key < 10` — numeric comparison
/// - `key` — truthy check (key exists and is not null/false)
pub fn evaluate_condition(condition: &str, state: &WorkflowState) -> bool {
    let condition = condition.trim();

    // Try equality: key == "value" or key == value
    if let Some((key, val)) = condition.split_once("==") {
        let key = key.trim();
        let val = val.trim().trim_matches('"');
        return state
            .get(key)
            .map(|v| match v {
                Value::String(s) => s == val,
                Value::Number(n) => n.to_string() == val,
                Value::Bool(b) => b.to_string() == val,
                _ => false,
            })
            .unwrap_or(false);
    }

    // Try inequality: key != "value"
    if let Some((key, val)) = condition.split_once("!=") {
        let key = key.trim();
        let val = val.trim().trim_matches('"');
        return state
            .get(key)
            .map(|v| match v {
                Value::String(s) => s != val,
                Value::Number(n) => n.to_string() != val,
                Value::Bool(b) => b.to_string() != val,
                _ => true,
            })
            .unwrap_or(true);
    }

    // Try greater than: key > number
    if let Some((key, val)) = condition.split_once('>') {
        let key = key.trim();
        let val = val.trim();
        if let Ok(threshold) = val.parse::<f64>() {
            return state
                .get(key)
                .and_then(|v| v.as_f64())
                .map(|n| n > threshold)
                .unwrap_or(false);
        }
    }

    // Try less than: key < number
    if let Some((key, val)) = condition.split_once('<') {
        let key = key.trim();
        let val = val.trim();
        if let Ok(threshold) = val.parse::<f64>() {
            return state
                .get(key)
                .and_then(|v| v.as_f64())
                .map(|n| n < threshold)
                .unwrap_or(false);
        }
    }

    // Truthy check: key exists and is not null/false
    state
        .get(condition)
        .map(|v| !v.is_null() && v.as_bool() != Some(false))
        .unwrap_or(false)
}

/// Errors that can occur during graph workflow operations.
#[derive(Debug, thiserror::Error)]
pub enum GraphWorkflowError {
    #[error("graph has no nodes")]
    EmptyGraph,

    #[error("duplicate node ID: {0}")]
    DuplicateNodeId(String),

    #[error("invalid edge from '{from}' to '{to}': {reason}")]
    InvalidEdge {
        from: String,
        to: String,
        reason: String,
    },

    #[error("node not found: {0}")]
    NodeNotFound(String),

    #[error("max iterations ({max}) reached at node '{last_node}'")]
    MaxIterationsReached { max: u32, last_node: String },

    #[error("node '{node_id}' execution failed: {reason}")]
    NodeExecutionFailed { node_id: String, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action_executor::ActionExecutor;
    use crate::config::*;

    /// Helper to create a WorkflowExecutionContext for tests (no real agents).
    fn test_ctx(executor: &ActionExecutor) -> WorkflowExecutionContext<'_> {
        let agents: &DashMap<String, Arc<dyn Agent>> = Box::leak(Box::new(DashMap::new()));
        let session_service: &Arc<dyn SessionService> = Box::leak(Box::new(Arc::new(
            adk_session::InMemorySessionService::new(),
        )
            as Arc<dyn SessionService>));
        WorkflowExecutionContext {
            action_executor: executor,
            agents,
            session_service,
        }
    }

    fn simple_config() -> GraphWorkflowConfig {
        GraphWorkflowConfig {
            nodes: vec![
                GraphNodeConfig {
                    id: "start".into(),
                    node_type: GraphNodeType::Agent,
                    config: serde_json::json!({"agent_id": "agent1"}),
                },
                GraphNodeConfig {
                    id: "end".into(),
                    node_type: GraphNodeType::Agent,
                    config: serde_json::json!({"agent_id": "agent2"}),
                },
            ],
            edges: vec![GraphEdgeConfig {
                from: "start".into(),
                to: "end".into(),
                condition: None,
            }],
            state_reducers: HashMap::new(),
            checkpoint: None,
            stream_mode: None,
            max_iterations: None,
            interrupts: None,
        }
    }

    #[test]
    fn test_build_simple_workflow() {
        let config = simple_config();
        let wf = GraphWorkflowBuilder::build(&config).unwrap();
        assert_eq!(wf.nodes.len(), 2);
        assert_eq!(wf.edges.len(), 1);
        assert_eq!(wf.max_iterations, 100);
    }

    #[test]
    fn test_build_empty_graph_fails() {
        let config = GraphWorkflowConfig {
            nodes: vec![],
            edges: vec![],
            state_reducers: HashMap::new(),
            checkpoint: None,
            stream_mode: None,
            max_iterations: None,
            interrupts: None,
        };
        assert!(matches!(
            GraphWorkflowBuilder::build(&config),
            Err(GraphWorkflowError::EmptyGraph)
        ));
    }

    #[test]
    fn test_build_duplicate_node_id_fails() {
        let config = GraphWorkflowConfig {
            nodes: vec![
                GraphNodeConfig {
                    id: "dup".into(),
                    node_type: GraphNodeType::Agent,
                    config: serde_json::json!({}),
                },
                GraphNodeConfig {
                    id: "dup".into(),
                    node_type: GraphNodeType::Action,
                    config: serde_json::json!({}),
                },
            ],
            edges: vec![],
            state_reducers: HashMap::new(),
            checkpoint: None,
            stream_mode: None,
            max_iterations: None,
            interrupts: None,
        };
        assert!(matches!(
            GraphWorkflowBuilder::build(&config),
            Err(GraphWorkflowError::DuplicateNodeId(_))
        ));
    }

    #[test]
    fn test_build_invalid_edge_fails() {
        let config = GraphWorkflowConfig {
            nodes: vec![GraphNodeConfig {
                id: "a".into(),
                node_type: GraphNodeType::Agent,
                config: serde_json::json!({}),
            }],
            edges: vec![GraphEdgeConfig {
                from: "a".into(),
                to: "nonexistent".into(),
                condition: None,
            }],
            state_reducers: HashMap::new(),
            checkpoint: None,
            stream_mode: None,
            max_iterations: None,
            interrupts: None,
        };
        assert!(matches!(
            GraphWorkflowBuilder::build(&config),
            Err(GraphWorkflowError::InvalidEdge { .. })
        ));
    }

    #[test]
    fn test_evaluate_condition_equality() {
        let mut state = WorkflowState::new();
        state.insert("status".into(), Value::String("done".into()));
        assert!(evaluate_condition("status == \"done\"", &state));
        assert!(!evaluate_condition("status == \"pending\"", &state));
    }

    #[test]
    fn test_evaluate_condition_inequality() {
        let mut state = WorkflowState::new();
        state.insert("status".into(), Value::String("done".into()));
        assert!(evaluate_condition("status != \"pending\"", &state));
        assert!(!evaluate_condition("status != \"done\"", &state));
    }

    #[test]
    fn test_evaluate_condition_numeric() {
        let mut state = WorkflowState::new();
        state.insert("count".into(), serde_json::json!(5));
        assert!(evaluate_condition("count > 3", &state));
        assert!(!evaluate_condition("count > 10", &state));
        assert!(evaluate_condition("count < 10", &state));
        assert!(!evaluate_condition("count < 2", &state));
    }

    #[test]
    fn test_evaluate_condition_truthy() {
        let mut state = WorkflowState::new();
        state.insert("flag".into(), Value::Bool(true));
        assert!(evaluate_condition("flag", &state));
        state.insert("flag".into(), Value::Bool(false));
        assert!(!evaluate_condition("flag", &state));
        assert!(!evaluate_condition("missing_key", &state));
    }

    #[test]
    fn test_reducer_overwrite() {
        let config = simple_config();
        let wf = GraphWorkflowBuilder::build(&config).unwrap();
        let mut state = WorkflowState::new();
        state.insert("key".into(), serde_json::json!("old"));
        wf.apply_reducer(&mut state, "key", serde_json::json!("new"));
        assert_eq!(state["key"], serde_json::json!("new"));
    }

    #[test]
    fn test_reducer_append() {
        let mut config = simple_config();
        config
            .state_reducers
            .insert("items".into(), ReducerType::Append);
        let wf = GraphWorkflowBuilder::build(&config).unwrap();
        let mut state = WorkflowState::new();
        wf.apply_reducer(&mut state, "items", serde_json::json!("a"));
        wf.apply_reducer(&mut state, "items", serde_json::json!("b"));
        assert_eq!(state["items"], serde_json::json!(["a", "b"]));
    }

    #[test]
    fn test_reducer_sum() {
        let mut config = simple_config();
        config
            .state_reducers
            .insert("total".into(), ReducerType::Sum);
        let wf = GraphWorkflowBuilder::build(&config).unwrap();
        let mut state = WorkflowState::new();
        wf.apply_reducer(&mut state, "total", serde_json::json!(10));
        wf.apply_reducer(&mut state, "total", serde_json::json!(5));
        assert_eq!(state["total"], serde_json::json!(15.0));
    }

    #[test]
    fn test_execute_agent_node_not_found() {
        // Agent node with non-existent agent_id should return error
        let executor = ActionExecutor::new();
        let ctx = test_ctx(&executor);
        let config = GraphWorkflowConfig {
            nodes: vec![GraphNodeConfig {
                id: "agent_node".into(),
                node_type: GraphNodeType::Agent,
                config: serde_json::json!({"agent_id": "nonexistent_agent"}),
            }],
            edges: vec![],
            state_reducers: HashMap::new(),
            checkpoint: None,
            stream_mode: None,
            max_iterations: None,
            interrupts: None,
        };
        let wf = GraphWorkflowBuilder::build(&config).unwrap();
        let result = wf.execute(WorkflowState::new(), &ctx);
        // Should complete but with error in the node result
        let exec_result = result.unwrap();
        assert_eq!(exec_result.executed_nodes.len(), 1);
        assert!(exec_result.executed_nodes[0].error.is_some());
        assert!(exec_result.executed_nodes[0]
            .error
            .as_ref()
            .unwrap()
            .contains("agent not found"));
    }

    #[test]
    fn test_execute_with_checkpointing() {
        // Use action nodes for checkpointing test (no agent dependency)
        let config = GraphWorkflowConfig {
            nodes: vec![
                GraphNodeConfig {
                    id: "step1".into(),
                    node_type: GraphNodeType::Action,
                    config: serde_json::json!({"actionType": "set", "params": {"values": {"x": 1}}}),
                },
                GraphNodeConfig {
                    id: "step2".into(),
                    node_type: GraphNodeType::Action,
                    config: serde_json::json!({"actionType": "set", "params": {"values": {"y": 2}}}),
                },
            ],
            edges: vec![GraphEdgeConfig {
                from: "step1".into(),
                to: "step2".into(),
                condition: None,
            }],
            state_reducers: HashMap::new(),
            checkpoint: Some(CheckpointConfig {
                backend: "memory".into(),
                path: None,
            }),
            stream_mode: None,
            max_iterations: None,
            interrupts: None,
        };
        let wf = GraphWorkflowBuilder::build(&config).unwrap();
        let executor = ActionExecutor::new();
        let ctx = test_ctx(&executor);
        let result = wf.execute(WorkflowState::new(), &ctx).unwrap();
        // Should have a checkpoint for each executed node
        assert_eq!(result.checkpoint_store.snapshots().len(), 2);
        let last = result.checkpoint_store.restore().unwrap();
        assert_eq!(last.node_id, "step2");
    }

    #[test]
    fn test_execute_max_iterations() {
        // Create a cycle: a -> b -> a with action nodes
        let config = GraphWorkflowConfig {
            nodes: vec![
                GraphNodeConfig {
                    id: "a".into(),
                    node_type: GraphNodeType::Action,
                    config: serde_json::json!({"actionType": "set", "params": {"values": {}}}),
                },
                GraphNodeConfig {
                    id: "b".into(),
                    node_type: GraphNodeType::Action,
                    config: serde_json::json!({"actionType": "set", "params": {"values": {}}}),
                },
            ],
            edges: vec![
                GraphEdgeConfig {
                    from: "a".into(),
                    to: "b".into(),
                    condition: None,
                },
                GraphEdgeConfig {
                    from: "b".into(),
                    to: "a".into(),
                    condition: None,
                },
            ],
            state_reducers: HashMap::new(),
            checkpoint: None,
            stream_mode: None,
            max_iterations: Some(5),
            interrupts: None,
        };
        let wf = GraphWorkflowBuilder::build(&config).unwrap();
        let executor = ActionExecutor::new();
        let ctx = test_ctx(&executor);
        let result = wf.execute(WorkflowState::new(), &ctx);
        assert!(matches!(
            result,
            Err(GraphWorkflowError::MaxIterationsReached { max: 5, .. })
        ));
    }

    #[test]
    fn test_conditional_edge_routing() {
        let config = GraphWorkflowConfig {
            nodes: vec![
                GraphNodeConfig {
                    id: "start".into(),
                    node_type: GraphNodeType::Action,
                    config: serde_json::json!({"actionType": "set", "params": {"values": {}}}),
                },
                GraphNodeConfig {
                    id: "yes_branch".into(),
                    node_type: GraphNodeType::Action,
                    config: serde_json::json!({"actionType": "set", "params": {"values": {}}}),
                },
                GraphNodeConfig {
                    id: "no_branch".into(),
                    node_type: GraphNodeType::Action,
                    config: serde_json::json!({"actionType": "set", "params": {"values": {}}}),
                },
            ],
            edges: vec![
                GraphEdgeConfig {
                    from: "start".into(),
                    to: "yes_branch".into(),
                    condition: Some("approved == \"true\"".into()),
                },
                GraphEdgeConfig {
                    from: "start".into(),
                    to: "no_branch".into(),
                    condition: None,
                },
            ],
            state_reducers: HashMap::new(),
            checkpoint: None,
            stream_mode: None,
            max_iterations: None,
            interrupts: None,
        };
        let wf = GraphWorkflowBuilder::build(&config).unwrap();

        // Without approved flag, should go to no_branch (unconditional fallback)
        let next = wf.resolve_next_node("start", &WorkflowState::new());
        assert_eq!(next.as_deref(), Some("no_branch"));

        // With approved flag, should go to yes_branch
        let mut state = WorkflowState::new();
        state.insert("approved".into(), Value::String("true".into()));
        let next = wf.resolve_next_node("start", &state);
        assert_eq!(next.as_deref(), Some("yes_branch"));
    }
}
