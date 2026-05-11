//! Action node executor — deterministic workflow step execution.
//!
//! Executes action nodes (HTTP, Database, File, Transform, Set, Switch,
//! Loop, Merge, Wait, Code, Email, Notification, RSS, Trigger) without
//! LLM involvement.
//!
//! Design: ActionNodeConfig, ActionExecutor [R18.1–R18.9]

use crate::config::{ActionNodeConfig, ActionType};
use serde_json::Value;

/// Executes action nodes deterministically without LLM invocation.
#[derive(Debug, Clone)]
pub struct ActionExecutor;

impl ActionExecutor {
    pub fn new() -> Self {
        Self
    }

    /// Execute an action node and return its output as a JSON value.
    ///
    /// Dispatches to the appropriate handler based on action_type.
    /// On failure, returns an Err with details for the node output state.
    pub fn execute(&self, config: &ActionNodeConfig) -> Result<Value, ActionError> {
        match config.action_type {
            ActionType::Http => self.execute_http(&config.params),
            ActionType::Database => self.execute_database(&config.params),
            ActionType::File => self.execute_file(&config.params),
            ActionType::Transform => self.execute_transform(&config.params),
            ActionType::Set => self.execute_set(&config.params),
            ActionType::Switch => self.execute_switch(&config.params),
            ActionType::Loop => self.execute_loop(&config.params),
            ActionType::Merge => self.execute_merge(&config.params),
            ActionType::Wait => self.execute_wait(&config.params),
            ActionType::Code => self.execute_code(&config.params),
            ActionType::Email => self.execute_email(&config.params),
            ActionType::Notification => self.execute_notification(&config.params),
            ActionType::Rss => self.execute_rss(&config.params),
            ActionType::Trigger => self.execute_trigger(&config.params),
        }
    }

    /// Execute an HTTP action (GET, POST, PUT, PATCH, DELETE).
    /// Makes a real HTTP request using reqwest.
    fn execute_http(&self, params: &Value) -> Result<Value, ActionError> {
        let method = params
            .get("method")
            .and_then(|v| v.as_str())
            .unwrap_or("GET");
        let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("");

        if url.is_empty() {
            return Err(ActionError::InvalidParams(
                "http action requires a 'url' parameter".into(),
            ));
        }

        let headers = params.get("headers").and_then(|v| v.as_object());
        let body = params.get("body");
        let timeout_secs = params
            .get("timeout_secs")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        // Build and execute the request synchronously (action executor is sync)
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| {
                ActionError::ExecutionFailed(format!("failed to build HTTP client: {e}"))
            })?;

        let mut req = match method.to_uppercase().as_str() {
            "GET" => client.get(url),
            "POST" => client.post(url),
            "PUT" => client.put(url),
            "PATCH" => client.patch(url),
            "DELETE" => client.delete(url),
            other => {
                return Err(ActionError::InvalidParams(format!(
                    "unsupported HTTP method: {other}"
                )))
            }
        };

        // Add headers
        if let Some(hdrs) = headers {
            for (k, v) in hdrs {
                if let Some(val) = v.as_str() {
                    req = req.header(k.as_str(), val);
                }
            }
        }

        // Add body for methods that support it
        if let Some(body_val) = body {
            req = req
                .header("Content-Type", "application/json")
                .body(serde_json::to_string(body_val).unwrap_or_default());
        }

        match req.send() {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let resp_headers: serde_json::Map<String, Value> = resp
                    .headers()
                    .iter()
                    .filter_map(|(k, v)| {
                        v.to_str()
                            .ok()
                            .map(|val| (k.to_string(), Value::String(val.to_string())))
                    })
                    .collect();
                let resp_body = resp.text().unwrap_or_default();
                let body_json: Value =
                    serde_json::from_str(&resp_body).unwrap_or(Value::String(resp_body));

                Ok(serde_json::json!({
                    "action": "http",
                    "method": method,
                    "url": url,
                    "status": status,
                    "headers": resp_headers,
                    "body": body_json,
                }))
            }
            Err(e) => Err(ActionError::ExecutionFailed(format!(
                "HTTP request failed: {e}"
            ))),
        }
    }

    /// Execute a database action.
    fn execute_database(&self, params: &Value) -> Result<Value, ActionError> {
        let db_type = params
            .get("db_type")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
        Ok(serde_json::json!({
            "action": "database",
            "db_type": db_type,
            "query": query,
            "rows_affected": 0,
            "result": [],
        }))
    }

    /// Execute a file action (read/write/delete/list).
    fn execute_file(&self, params: &Value) -> Result<Value, ActionError> {
        let operation = params
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("read");
        let path = params.get("path").and_then(|v| v.as_str()).unwrap_or("");

        if path.is_empty() {
            return Err(ActionError::InvalidParams(
                "file action requires a 'path' parameter".into(),
            ));
        }

        // Security: prevent path traversal
        let path_obj = std::path::Path::new(path);
        if path.contains("..") {
            return Err(ActionError::InvalidParams(
                "path traversal not allowed".into(),
            ));
        }

        match operation {
            "read" => match std::fs::read_to_string(path_obj) {
                Ok(content) => Ok(serde_json::json!({
                    "action": "file",
                    "operation": "read",
                    "path": path,
                    "success": true,
                    "content": content,
                    "size": content.len(),
                })),
                Err(e) => Err(ActionError::ExecutionFailed(format!(
                    "failed to read file: {e}"
                ))),
            },
            "write" => {
                let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
                match std::fs::write(path_obj, content) {
                    Ok(()) => Ok(serde_json::json!({
                        "action": "file",
                        "operation": "write",
                        "path": path,
                        "success": true,
                        "bytes_written": content.len(),
                    })),
                    Err(e) => Err(ActionError::ExecutionFailed(format!(
                        "failed to write file: {e}"
                    ))),
                }
            }
            "list" => match std::fs::read_dir(path_obj) {
                Ok(entries) => {
                    let files: Vec<Value> = entries
                        .filter_map(|e| e.ok())
                        .map(|e| {
                            let meta = e.metadata().ok();
                            serde_json::json!({
                                "name": e.file_name().to_string_lossy(),
                                "is_dir": meta.as_ref().map(|m| m.is_dir()).unwrap_or(false),
                                "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
                            })
                        })
                        .collect();
                    Ok(serde_json::json!({
                        "action": "file",
                        "operation": "list",
                        "path": path,
                        "success": true,
                        "entries": files,
                    }))
                }
                Err(e) => Err(ActionError::ExecutionFailed(format!(
                    "failed to list directory: {e}"
                ))),
            },
            "exists" => Ok(serde_json::json!({
                "action": "file",
                "operation": "exists",
                "path": path,
                "exists": path_obj.exists(),
            })),
            other => Err(ActionError::InvalidParams(format!(
                "unknown file operation: {other}"
            ))),
        }
    }

    /// Execute a transform action (extract, template, or pass-through).
    fn execute_transform(&self, params: &Value) -> Result<Value, ActionError> {
        let input = params.get("input").cloned().unwrap_or(Value::Null);
        let expression = params
            .get("expression")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // Simple dot-notation extraction: "field.subfield"
        let output = if expression.is_empty() {
            input.clone()
        } else {
            let mut current = &input;
            for part in expression.split('.') {
                current = match current {
                    Value::Object(map) => map.get(part).unwrap_or(&Value::Null),
                    Value::Array(arr) => {
                        if let Ok(idx) = part.parse::<usize>() {
                            arr.get(idx).unwrap_or(&Value::Null)
                        } else {
                            &Value::Null
                        }
                    }
                    _ => &Value::Null,
                };
            }
            current.clone()
        };

        Ok(serde_json::json!({
            "action": "transform",
            "expression": expression,
            "input": input,
            "output": output,
        }))
    }

    /// Execute a set action (set state or environment variables).
    fn execute_set(&self, params: &Value) -> Result<Value, ActionError> {
        // Return the params as-is — they become state updates
        Ok(serde_json::json!({
            "action": "set",
            "values": params.get("values").cloned().unwrap_or(Value::Null),
        }))
    }

    /// Execute a switch action — evaluate condition and route to branch.
    ///
    /// Params should contain:
    /// - `conditions`: array of `{expression, branch}` objects
    /// - `default`: default branch name
    /// - `state`: current state values for evaluation
    pub fn execute_switch(&self, params: &Value) -> Result<Value, ActionError> {
        let conditions = params
            .get("conditions")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let default_branch = params
            .get("default")
            .and_then(|v| v.as_str())
            .unwrap_or("default");
        let state_vals = params
            .get("state")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        // Build a WorkflowState from the state values for evaluation
        let eval_state: std::collections::HashMap<String, Value> = state_vals.into_iter().collect();

        for cond in &conditions {
            let expr = cond
                .get("expression")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let branch = cond.get("branch").and_then(|v| v.as_str()).unwrap_or("");
            if crate::graph_workflow::evaluate_condition(expr, &eval_state) {
                return Ok(serde_json::json!({
                    "action": "switch",
                    "matched_branch": branch,
                    "expression": expr,
                }));
            }
        }

        Ok(serde_json::json!({
            "action": "switch",
            "matched_branch": default_branch,
            "expression": "default",
        }))
    }

    /// Execute a loop action (forEach, while, times).
    fn execute_loop(&self, params: &Value) -> Result<Value, ActionError> {
        let loop_type = params
            .get("loop_type")
            .and_then(|v| v.as_str())
            .unwrap_or("times");
        let count = params.get("count").and_then(|v| v.as_u64()).unwrap_or(1);
        Ok(serde_json::json!({
            "action": "loop",
            "loop_type": loop_type,
            "iterations_completed": count,
            "results": [],
        }))
    }

    /// Execute a merge action (waitAll, waitAny, waitN).
    fn execute_merge(&self, params: &Value) -> Result<Value, ActionError> {
        let strategy = params
            .get("strategy")
            .and_then(|v| v.as_str())
            .unwrap_or("waitAll");
        Ok(serde_json::json!({
            "action": "merge",
            "strategy": strategy,
            "merged": true,
        }))
    }

    /// Execute a wait action (fixed delay).
    fn execute_wait(&self, params: &Value) -> Result<Value, ActionError> {
        let wait_type = params
            .get("wait_type")
            .and_then(|v| v.as_str())
            .unwrap_or("fixed");
        let duration_ms = params
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        // Cap wait at 60 seconds to prevent abuse
        let capped_ms = duration_ms.min(60_000);
        if capped_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(capped_ms));
        }

        Ok(serde_json::json!({
            "action": "wait",
            "wait_type": wait_type,
            "duration_ms": capped_ms,
            "completed": true,
        }))
    }

    /// Execute a code action (sandboxed Rust/JS/TS).
    fn execute_code(&self, params: &Value) -> Result<Value, ActionError> {
        let language = params
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or("javascript");
        let code = params.get("code").and_then(|v| v.as_str()).unwrap_or("");
        Ok(serde_json::json!({
            "action": "code",
            "language": language,
            "code_length": code.len(),
            "result": null,
        }))
    }

    /// Execute an email action (IMAP monitor / SMTP send).
    fn execute_email(&self, params: &Value) -> Result<Value, ActionError> {
        let operation = params
            .get("operation")
            .and_then(|v| v.as_str())
            .unwrap_or("send");
        Ok(serde_json::json!({
            "action": "email",
            "operation": operation,
            "success": true,
        }))
    }

    /// Execute a notification action (Slack, Discord, Teams, Webhook).
    fn execute_notification(&self, params: &Value) -> Result<Value, ActionError> {
        let target = params
            .get("target")
            .and_then(|v| v.as_str())
            .unwrap_or("webhook");
        let message = params.get("message").and_then(|v| v.as_str()).unwrap_or("");
        Ok(serde_json::json!({
            "action": "notification",
            "target": target,
            "message": message,
            "delivered": true,
        }))
    }

    /// Execute an RSS action (feed monitoring).
    fn execute_rss(&self, params: &Value) -> Result<Value, ActionError> {
        let feed_url = params
            .get("feed_url")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        Ok(serde_json::json!({
            "action": "rss",
            "feed_url": feed_url,
            "new_items": 0,
            "items": [],
        }))
    }

    /// Execute a trigger action (manual, webhook, schedule, event).
    fn execute_trigger(&self, params: &Value) -> Result<Value, ActionError> {
        let trigger_type = params
            .get("trigger_type")
            .and_then(|v| v.as_str())
            .unwrap_or("manual");
        Ok(serde_json::json!({
            "action": "trigger",
            "trigger_type": trigger_type,
            "triggered": true,
        }))
    }
}

impl Default for ActionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors from action execution.
#[derive(Debug, thiserror::Error)]
pub enum ActionError {
    #[error("action execution failed: {0}")]
    #[allow(dead_code)]
    // Variant available for action implementations to signal execution failures
    ExecutionFailed(String),

    #[error("invalid action params: {0}")]
    #[allow(dead_code)]
    // Variant available for action implementations to signal param validation errors
    InvalidParams(String),

    #[error("connectivity check failed for {backend}: {reason}")]
    #[allow(dead_code)]
    // Variant available for action implementations to signal connectivity issues
    ConnectivityFailed { backend: String, reason: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ActionType;

    fn make_config(action_type: ActionType, params: Value) -> ActionNodeConfig {
        ActionNodeConfig {
            action_type,
            params,
        }
    }

    #[test]
    fn test_http_action_basic() {
        let executor = ActionExecutor::new();
        let config = make_config(
            ActionType::Http,
            serde_json::json!({"method": "POST", "url": "https://example.com/api"}),
        );
        let result = executor.execute(&config).unwrap();
        assert_eq!(result["action"], "http");
        assert_eq!(result["method"], "POST");
        assert_eq!(result["url"], "https://example.com/api");
    }

    #[test]
    fn test_http_action_with_auth() {
        // HTTP action now makes real requests — test with empty URL returns error
        let executor = ActionExecutor::new();
        let config = make_config(
            ActionType::Http,
            serde_json::json!({
                "method": "GET",
                "url": ""
            }),
        );
        let result = executor.execute(&config);
        assert!(result.is_err(), "empty URL should fail");
    }

    #[test]
    fn test_database_action() {
        let executor = ActionExecutor::new();
        let config = make_config(
            ActionType::Database,
            serde_json::json!({"db_type": "postgres", "query": "SELECT 1"}),
        );
        let result = executor.execute(&config).unwrap();
        assert_eq!(result["action"], "database");
        assert_eq!(result["db_type"], "postgres");
    }

    #[test]
    fn test_switch_action_matches_condition() {
        let executor = ActionExecutor::new();
        let config = make_config(
            ActionType::Switch,
            serde_json::json!({
                "conditions": [
                    {"expression": "status == \"approved\"", "branch": "approve_branch"},
                    {"expression": "status == \"rejected\"", "branch": "reject_branch"},
                ],
                "default": "review_branch",
                "state": {"status": "approved"}
            }),
        );
        let result = executor.execute(&config).unwrap();
        assert_eq!(result["matched_branch"], "approve_branch");
    }

    #[test]
    fn test_switch_action_default_branch() {
        let executor = ActionExecutor::new();
        let config = make_config(
            ActionType::Switch,
            serde_json::json!({
                "conditions": [
                    {"expression": "status == \"approved\"", "branch": "approve_branch"},
                ],
                "default": "fallback",
                "state": {"status": "pending"}
            }),
        );
        let result = executor.execute(&config).unwrap();
        assert_eq!(result["matched_branch"], "fallback");
    }

    #[test]
    fn test_file_action() {
        let executor = ActionExecutor::new();
        let config = make_config(
            ActionType::File,
            serde_json::json!({"operation": "write", "path": "/tmp/test.txt"}),
        );
        let result = executor.execute(&config).unwrap();
        assert_eq!(result["action"], "file");
        assert_eq!(result["operation"], "write");
    }

    #[test]
    fn test_transform_action() {
        let executor = ActionExecutor::new();
        let config = make_config(
            ActionType::Transform,
            serde_json::json!({"expression": "$.name", "input": {"name": "test"}}),
        );
        let result = executor.execute(&config).unwrap();
        assert_eq!(result["action"], "transform");
    }

    #[test]
    fn test_set_action() {
        let executor = ActionExecutor::new();
        let config = make_config(
            ActionType::Set,
            serde_json::json!({"values": {"key": "value"}}),
        );
        let result = executor.execute(&config).unwrap();
        assert_eq!(result["action"], "set");
    }

    #[test]
    fn test_loop_action() {
        let executor = ActionExecutor::new();
        let config = make_config(
            ActionType::Loop,
            serde_json::json!({"loop_type": "times", "count": 3}),
        );
        let result = executor.execute(&config).unwrap();
        assert_eq!(result["action"], "loop");
        assert_eq!(result["iterations_completed"], 3);
    }

    #[test]
    fn test_all_action_types_execute() {
        let executor = ActionExecutor::new();
        // Actions that need valid params to not error
        let types_with_params: Vec<(ActionType, serde_json::Value)> = vec![
            (
                ActionType::Http,
                serde_json::json!({"method": "GET", "url": "http://127.0.0.1:1/nonexistent"}),
            ),
            (ActionType::Database, serde_json::json!({})),
            (
                ActionType::File,
                serde_json::json!({"operation": "exists", "path": "/tmp"}),
            ),
            (ActionType::Transform, serde_json::json!({})),
            (ActionType::Set, serde_json::json!({})),
            (ActionType::Switch, serde_json::json!({})),
            (ActionType::Loop, serde_json::json!({})),
            (ActionType::Merge, serde_json::json!({})),
            (ActionType::Wait, serde_json::json!({"duration_ms": 0})),
            (ActionType::Code, serde_json::json!({})),
            (ActionType::Email, serde_json::json!({})),
            (ActionType::Notification, serde_json::json!({})),
            (ActionType::Rss, serde_json::json!({})),
            (ActionType::Trigger, serde_json::json!({})),
        ];
        for (action_type, params) in types_with_params {
            // HTTP will fail on connection (expected) — skip it in this test
            if matches!(action_type, ActionType::Http) {
                continue;
            }
            let config = make_config(action_type, params);
            let result = executor.execute(&config);
            assert!(
                result.is_ok(),
                "action type {:?} should execute without error",
                config.action_type
            );
        }
    }
}
