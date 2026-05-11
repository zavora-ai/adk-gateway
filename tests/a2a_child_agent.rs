//! Integration test: generate, build, spawn, health-check, send one message.
//!
//! This test exercises the full A2A child agent pipeline:
//! 1. Generate agent source code via AgentCodegen
//! 2. Build the generated binary with `cargo build`
//! 3. Spawn the binary as a child process
//! 4. Health-check via GET /.well-known/agent.json
//! 5. Send a POST /a2a message and verify the response

use std::collections::HashMap;
use std::time::Duration;

use adk_gateway::agent_codegen::AgentCodegen;
use adk_gateway::agent_config::{AgentConfig, AgentRoleConfig, AgentType};
use adk_gateway::process_manager::ProcessManager;
use tempfile::TempDir;

/// Helper: build a minimal LLM agent config for testing.
fn test_agent_config() -> AgentConfig {
    AgentConfig {
        id: "test-a2a-agent".to_string(),
        name: "Test A2A Agent".to_string(),
        description: "A test agent for A2A integration".to_string(),
        agent_type: AgentType::Llm,
        model: "anthropic/claude-sonnet-4".to_string(),
        api_key_env: "ANTHROPIC_API_KEY".to_string(),
        instruction: "You are a helpful assistant.".to_string(),
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

// ════════════════════════════════════════════════════════════════════
// 8.5: Verify generated code compiles with `cargo check`
// ════════════════════════════════════════════════════════════════════

/// Generates the full agent project (with real axum routes) and verifies
/// it compiles with `cargo check`.
#[tokio::test]
async fn generated_agent_compiles() {
    let tmp = TempDir::new().unwrap();
    let workspace_root = tmp.path().to_path_buf();
    let codegen = AgentCodegen::new(workspace_root.clone(), None);
    let config = test_agent_config();

    // Generate source.
    let schema = codegen.to_project_schema(&config);
    let mut project = codegen.generate_source(&schema).unwrap();

    // Inject A2A server (replaces stdin loop with real axum routes).
    project.main_rs = codegen.inject_a2a_server(&project.main_rs, &config);

    // Write files to the agent workspace.
    let agent_dir = workspace_root.join("agents").join(&config.id);
    codegen.write_files(&agent_dir, &project).unwrap();

    // Verify the generated main.rs contains real axum code (not commented out).
    let main_content = std::fs::read_to_string(agent_dir.join("src/main.rs")).unwrap();
    assert!(
        main_content.contains("axum::Router::new()"),
        "generated code should contain real axum Router"
    );
    assert!(
        main_content.contains("axum::serve(listener, app)"),
        "generated code should call axum::serve"
    );
    assert!(
        main_content.contains("#[tokio::main]"),
        "generated code should use #[tokio::main]"
    );
    assert!(
        main_content.contains("/.well-known/agent.json"),
        "generated code should have agent card route"
    );
    assert!(
        main_content.contains("/a2a"),
        "generated code should have /a2a route"
    );
    // Verify no commented-out routes remain.
    assert!(
        !main_content.contains("// let app = axum::Router::new()"),
        "generated code should NOT contain commented-out routes"
    );
    assert!(
        !main_content.contains("// Placeholder:"),
        "generated code should NOT contain placeholder comments"
    );

    // Run `cargo check` in the agent workspace.
    let output = tokio::process::Command::new("cargo")
        .args(["check"])
        .current_dir(&agent_dir)
        .output()
        .await
        .expect("failed to run cargo check");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "cargo check failed for generated agent:\n{}",
        stderr
    );
}

// ════════════════════════════════════════════════════════════════════
// 8.6 + 8.7: Build, spawn, health-check, send one message
// ════════════════════════════════════════════════════════════════════

/// Full integration: generate → build → spawn → health-check → send message.
#[tokio::test]
async fn generated_agent_serves_a2a_endpoint() {
    let tmp = TempDir::new().unwrap();
    let workspace_root = tmp.path().to_path_buf();
    let codegen = AgentCodegen::new(workspace_root.clone(), None);
    let config = test_agent_config();

    // Generate and inject A2A server.
    let schema = codegen.to_project_schema(&config);
    let mut project = codegen.generate_source(&schema).unwrap();
    project.main_rs = codegen.inject_a2a_server(&project.main_rs, &config);

    // Write files.
    let agent_dir = workspace_root.join("agents").join(&config.id);
    codegen.write_files(&agent_dir, &project).unwrap();

    // Build the agent binary.
    let binary_path = codegen.compile(&agent_dir).await.unwrap_or_else(|e| {
        panic!("failed to compile generated agent: {}", e);
    });
    assert!(binary_path.exists(), "binary should exist after compile");

    // Spawn the agent process using ProcessManager.
    let pm = ProcessManager::new(19050..=19060, Duration::from_secs(30));
    let port = pm
        .spawn("test-a2a-agent", &binary_path, HashMap::new())
        .await
        .expect("failed to spawn agent");

    // Wait for the agent to become ready (health check on /.well-known/agent.json).
    let ready_result = pm
        .wait_ready("test-a2a-agent", Duration::from_secs(30))
        .await;
    assert!(
        ready_result.is_ok(),
        "agent should become ready: {:?}",
        ready_result.err()
    );

    // Verify health check passes.
    let health = pm.health_check("test-a2a-agent").await;
    assert_eq!(
        health,
        adk_gateway::process_manager::HealthStatus::Healthy,
        "health check should report Healthy"
    );

    // Send a POST /a2a message.
    let client = reqwest::Client::new();
    let a2a_url = format!("http://127.0.0.1:{}/a2a", port);
    let message = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "message/send",
        "params": {
            "message": {
                "role": "user",
                "parts": [{"type": "text", "text": "Hello agent!"}]
            }
        }
    });

    let resp = client
        .post(&a2a_url)
        .json(&message)
        .send()
        .await
        .expect("failed to send A2A message");

    assert_eq!(resp.status(), 200, "POST /a2a should return 200");

    let body: serde_json::Value = resp.json().await.expect("response should be JSON");
    assert_eq!(body["jsonrpc"], "2.0", "response should be JSON-RPC");
    assert_eq!(body["id"], 1, "response id should match request");
    assert!(
        body["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .contains("processed your request"),
        "response should contain agent reply"
    );

    // Verify GET /.well-known/agent.json returns valid agent card.
    let card_url = format!("http://127.0.0.1:{}/.well-known/agent.json", port);
    let card_resp = client
        .get(&card_url)
        .send()
        .await
        .expect("failed to get agent card");
    assert_eq!(card_resp.status(), 200);

    let card: serde_json::Value = card_resp.json().await.expect("card should be JSON");
    assert_eq!(card["name"], "Test A2A Agent");
    assert_eq!(card["description"], "A test agent for A2A integration");
    assert!(card["capabilities"]["streaming"].as_bool().unwrap_or(false));

    // Stop the agent.
    pm.stop("test-a2a-agent", Duration::from_secs(5))
        .await
        .expect("failed to stop agent");
}
