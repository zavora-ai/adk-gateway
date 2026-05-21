//! Onboarding wizard API for coding agent registration.
//!
//! Provides step-by-step endpoints for the frontend to guide users through
//! agent setup: listing available backends, verifying CLI installation,
//! validating authentication, and finalizing registration.
//!
//! Design: Onboarding Wizard API [R1.2, R1.3, R1.4, R1.5, R1.7]

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use super::ControlPanelState;
use crate::coding_agent::backend::{CodingAgentBackend, ConfigDrivenBackend};
use crate::coding_agent::config::{
    AgentAuthConfig, CodingAgentInstanceConfig,
};
use crate::coding_agent::status::AuthStatus;

// ── Response types ─────────────────────────────────────────────────

/// Response for listing available backend definitions.
#[derive(Serialize)]
pub struct BackendsListResponse {
    pub ok: bool,
    pub backends: Vec<BackendSummary>,
}

/// Summary of a backend definition for the onboarding UI.
#[derive(Serialize)]
pub struct BackendSummary {
    pub agent_type: String,
    pub display_name: String,
    pub cli_command: String,
    pub auth_method: serde_json::Value,
    pub capabilities: serde_json::Value,
    pub install_instructions: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_instructions_windows: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_instructions_linux: Option<String>,
}

/// Response for the installation check step.
#[derive(Serialize)]
pub struct CheckInstallResponse {
    pub ok: bool,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Installation instructions returned when binary is not found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Response for the authentication validation step.
#[derive(Serialize)]
pub struct ValidateAuthResponse {
    pub ok: bool,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Response for the onboarding completion step.
#[derive(Serialize)]
pub struct OnboardingCompleteResponse {
    pub ok: bool,
    pub agent_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Generic error response.
#[derive(Serialize)]
pub struct ErrorResponse {
    pub ok: bool,
    pub message: String,
}

// ── Request types ──────────────────────────────────────────────────

/// Request body for the installation check step.
#[derive(Deserialize)]
pub struct CheckInstallRequest {
    /// The backend agent type to check installation for.
    pub agent_type: String,
}

/// Request body for the authentication validation step.
#[derive(Deserialize)]
pub struct ValidateAuthRequest {
    /// The backend agent type to validate auth for.
    pub agent_type: String,
    /// Optional credentials to validate (API key, token, etc.).
    pub credentials: Option<String>,
}

/// Request body for the onboarding completion step.
#[derive(Deserialize)]
pub struct OnboardingCompleteRequest {
    /// Unique identifier for the new agent instance.
    pub id: String,
    /// The backend type (must reference a loaded BackendDefinition).
    pub backend_type: String,
    /// ACP endpoint URL for this agent.
    pub endpoint: String,
    /// Workspace directories this agent can operate in.
    pub workspaces: Vec<PathBuf>,
    /// Per-agent timeout override in seconds.
    pub timeout_secs: Option<u64>,
    /// Per-task cost cap in USD.
    pub cost_cap_usd: Option<f64>,
    /// Monthly budget alert threshold in USD.
    pub monthly_budget_usd: Option<f64>,
    /// Shorthand alias for delegation commands.
    pub alias: Option<String>,
    /// Authentication credentials.
    pub credentials: Option<String>,
}

// ── Handlers ───────────────────────────────────────────────────────

/// GET /ui/api/coding-agents/backends
///
/// Lists all available backend definitions that can be used to register
/// new coding agent instances. The frontend uses this to populate the
/// onboarding wizard's agent type selector.
pub async fn list_backends(
    State(state): State<Arc<ControlPanelState>>,
) -> Json<serde_json::Value> {
    let registry = match &state.coding_agent_registry {
        Some(r) => r,
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": "Coding agent subsystem is not enabled",
                "backends": []
            }));
        }
    };

    let backends: Vec<BackendSummary> = registry
        .list_backends()
        .into_iter()
        .map(|b| BackendSummary {
            agent_type: b.agent_type.clone(),
            display_name: b.display_name.clone(),
            cli_command: b.cli_command.clone(),
            auth_method: serde_json::to_value(&b.auth_method).unwrap_or_default(),
            capabilities: serde_json::to_value(&b.capabilities).unwrap_or_default(),
            install_instructions: b.install_instructions.clone(),
            install_instructions_windows: b.install_instructions_windows.clone(),
            install_instructions_linux: b.install_instructions_linux.clone(),
        })
        .collect();

    Json(serde_json::json!({
        "ok": true,
        "backends": backends
    }))
}

/// POST /ui/api/coding-agents/onboarding/check-install
///
/// Verifies that the CLI binary for the specified backend is installed
/// on the system PATH. Runs the backend's `install_check_command` and
/// returns version info if found, or installation instructions if not.
pub async fn check_install(
    State(state): State<Arc<ControlPanelState>>,
    Json(payload): Json<CheckInstallRequest>,
) -> Json<serde_json::Value> {
    let registry = match &state.coding_agent_registry {
        Some(r) => r,
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": "Coding agent subsystem is not enabled"
            }));
        }
    };

    let backend_def = match registry.get_backend(&payload.agent_type) {
        Some(b) => b,
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": format!("Unknown backend type: {}", payload.agent_type)
            }));
        }
    };

    // Create a ConfigDrivenBackend to run the installation check
    let backend = ConfigDrivenBackend::new(backend_def.clone(), None);

    match backend.check_installation().await {
        Ok(status) => {
            if status.installed {
                Json(serde_json::json!({
                    "ok": true,
                    "installed": true,
                    "version": status.version,
                    "path": status.path.map(|p| p.display().to_string()),
                }))
            } else {
                // Binary not found — return installation instructions
                Json(serde_json::json!({
                    "ok": true,
                    "installed": false,
                    "install_instructions": backend_def.install_instructions,
                    "message": format!(
                        "CLI binary '{}' not found on system PATH. Please install it first.",
                        backend_def.cli_command
                    )
                }))
            }
        }
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "installed": false,
            "message": format!("Installation check failed: {}", e)
        })),
    }
}

/// POST /ui/api/coding-agents/onboarding/run-install
///
/// Runs the install command for the specified backend type and returns the
/// combined stdout/stderr output. This allows the UI to show installation
/// progress directly instead of requiring the user to open a terminal.
pub async fn run_install(
    State(state): State<Arc<ControlPanelState>>,
    Json(payload): Json<CheckInstallRequest>,
) -> Json<serde_json::Value> {
    let registry = match &state.coding_agent_registry {
        Some(r) => r,
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": "Coding agent subsystem is not enabled"
            }));
        }
    };

    let backend_def = match registry.get_backend(&payload.agent_type) {
        Some(b) => b,
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": format!("Unknown backend type: {}", payload.agent_type)
            }));
        }
    };

    let install_cmd = if cfg!(target_os = "windows") {
        backend_def.install_instructions_windows.as_deref()
            .unwrap_or(&backend_def.install_instructions)
    } else if cfg!(target_os = "linux") {
        backend_def.install_instructions_linux.as_deref()
            .unwrap_or(&backend_def.install_instructions)
    } else {
        &backend_def.install_instructions
    };

    if install_cmd.is_empty() {
        return Json(serde_json::json!({
            "ok": false,
            "message": "No install command configured for this backend"
        }));
    }

    // Run the install command via shell
    let output = tokio::process::Command::new("sh")
        .args(["-c", install_cmd])
        .output()
        .await;

    match output {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let combined = if stderr.is_empty() {
                stdout.clone()
            } else if stdout.is_empty() {
                stderr.clone()
            } else {
                format!("{}\n{}", stdout, stderr)
            };

            if output.status.success() {
                Json(serde_json::json!({
                    "ok": true,
                    "success": true,
                    "output": combined.trim(),
                    "exit_code": 0
                }))
            } else {
                Json(serde_json::json!({
                    "ok": true,
                    "success": false,
                    "output": combined.trim(),
                    "exit_code": output.status.code()
                }))
            }
        }
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to run install command: {}", e)
        })),
    }
}

/// POST /ui/api/coding-agents/onboarding/validate-auth
///
/// Validates authentication credentials for the specified backend type.
/// For API key auth, checks if the environment variable is set or validates
/// the provided credentials. For OAuth/CLI login, reports configuration status.
pub async fn validate_auth(
    State(state): State<Arc<ControlPanelState>>,
    Json(payload): Json<ValidateAuthRequest>,
) -> Json<serde_json::Value> {
    let registry = match &state.coding_agent_registry {
        Some(r) => r,
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": "Coding agent subsystem is not enabled"
            }));
        }
    };

    let backend_def = match registry.get_backend(&payload.agent_type) {
        Some(b) => b,
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": format!("Unknown backend type: {}", payload.agent_type)
            }));
        }
    };

    // If credentials are provided, temporarily set the env var for validation
    let _env_guard = if let Some(ref creds) = payload.credentials {
        match &backend_def.auth_method {
            crate::coding_agent::config::AuthMethod::ApiKey { env_var } => {
                let old_val = std::env::var(env_var).ok();
                // Safety: we're in a single-threaded handler context for this request
                unsafe { std::env::set_var(env_var, creds) };
                Some((env_var.clone(), old_val))
            }
            _ => None,
        }
    } else {
        None
    };

    let backend = ConfigDrivenBackend::new(backend_def, None);
    let result = backend.validate_auth().await;

    // Restore the original env var value
    if let Some((env_var, old_val)) = _env_guard {
        match old_val {
            Some(val) => unsafe { std::env::set_var(&env_var, &val) },
            None => unsafe { std::env::remove_var(&env_var) },
        }
    }

    match result {
        Ok(auth_status) => {
            let (status_str, expires_at) = match &auth_status {
                AuthStatus::Valid { expires_at } => (
                    "valid".to_string(),
                    expires_at.map(|dt| dt.to_rfc3339()),
                ),
                AuthStatus::Expired { expired_at } => (
                    "expired".to_string(),
                    Some(expired_at.to_rfc3339()),
                ),
                AuthStatus::NotConfigured => ("not_configured".to_string(), None),
            };

            Json(serde_json::json!({
                "ok": true,
                "status": status_str,
                "expires_at": expires_at,
            }))
        }
        Err(e) => Json(serde_json::json!({
            "ok": false,
            "status": "error",
            "message": format!("Authentication validation failed: {}", e)
        })),
    }
}

/// POST /ui/api/coding-agents/onboarding/complete
///
/// Finalizes the coding agent registration. Persists the agent configuration
/// and registers it in the runtime registry. This is the last step of the
/// onboarding wizard.
pub async fn onboarding_complete(
    State(state): State<Arc<ControlPanelState>>,
    Json(payload): Json<OnboardingCompleteRequest>,
) -> Json<serde_json::Value> {
    let registry = match &state.coding_agent_registry {
        Some(r) => r,
        None => {
            return Json(serde_json::json!({
                "ok": false,
                "message": "Coding agent subsystem is not enabled"
            }));
        }
    };

    // Validate that the backend type exists
    if registry.get_backend(&payload.backend_type).is_none() {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Unknown backend type: {}", payload.backend_type)
        }));
    }

    // Validate required fields
    if payload.id.trim().is_empty() {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Agent ID is required"
        }));
    }

    if payload.endpoint.trim().is_empty() {
        return Json(serde_json::json!({
            "ok": false,
            "message": "Endpoint URL is required"
        }));
    }

    if payload.workspaces.is_empty() {
        return Json(serde_json::json!({
            "ok": false,
            "message": "At least one workspace directory is required"
        }));
    }

    // Build the agent instance config
    let auth = payload.credentials.as_ref().map(|creds| AgentAuthConfig {
        credentials: Some(creds.clone()),
        token: None,
        expires_at: None,
    });

    let agent_config = CodingAgentInstanceConfig {
        id: payload.id.clone(),
        backend_type: payload.backend_type.clone(),
        endpoint: payload.endpoint.clone(),
        transport: None,
        workspaces: payload.workspaces.clone(),
        timeout_secs: payload.timeout_secs,
        cost_cap_usd: payload.cost_cap_usd,
        monthly_budget_usd: payload.monthly_budget_usd,
        alias: payload.alias.clone(),
        auth,
    };

    // Register in the runtime registry
    if let Err(e) = registry.register_agent(agent_config.clone()) {
        return Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to register agent: {}", e)
        }));
    }

    // Persist to config file if config_path is available
    if let Some(config_path) = &state.config_path {
        if let Err(e) = persist_agent_config(config_path, &agent_config) {
            // Agent is registered in memory but config persistence failed.
            // Log the error but don't fail the request — the agent is usable.
            tracing::warn!(
                agent_id = %payload.id,
                error = %e,
                "Agent registered but config persistence failed"
            );
            return Json(serde_json::json!({
                "ok": true,
                "agent_id": payload.id,
                "message": "Agent registered successfully, but config file could not be updated. Changes may not persist across restarts."
            }));
        }
    }

    Json(serde_json::json!({
        "ok": true,
        "agent_id": payload.id,
        "message": "Agent registered and configuration saved successfully."
    }))
}

// ── Helpers ────────────────────────────────────────────────────────

/// Persist a new agent configuration to the gateway config file.
///
/// Reads the existing config, appends the new agent to the `codingAgents.agents`
/// array, and writes it back.
fn persist_agent_config(
    config_path: &std::path::Path,
    agent_config: &CodingAgentInstanceConfig,
) -> Result<(), String> {
    let raw = std::fs::read_to_string(config_path)
        .map_err(|e| format!("Failed to read config file: {}", e))?;

    let mut config_value: serde_json::Value = serde_json::from_str(&raw)
        .or_else(|_| json5::from_str(&raw))
        .map_err(|e| format!("Failed to parse config file: {}", e))?;

    // Ensure codingAgents.agents array exists
    let coding_agents = config_value
        .as_object_mut()
        .ok_or("Config is not a JSON object")?
        .entry("codingAgents")
        .or_insert_with(|| serde_json::json!({"enabled": true, "agents": [], "backends": []}));

    let agents_array = coding_agents
        .as_object_mut()
        .ok_or("codingAgents is not an object")?
        .entry("agents")
        .or_insert_with(|| serde_json::json!([]));

    let arr = agents_array
        .as_array_mut()
        .ok_or("codingAgents.agents is not an array")?;

    // Serialize the new agent config and append
    let agent_value = serde_json::to_value(agent_config)
        .map_err(|e| format!("Failed to serialize agent config: {}", e))?;

    arr.push(agent_value);

    // Write back
    let output = serde_json::to_string_pretty(&config_value)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;

    std::fs::write(config_path, output)
        .map_err(|e| format!("Failed to write config file: {}", e))?;

    Ok(())
}
