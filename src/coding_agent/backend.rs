//! CodingAgentBackend trait and config-driven backend implementation.
//!
//! Defines the core abstraction for coding agent backends and provides a
//! `ConfigDrivenBackend` implementation that derives behavior from
//! `BackendDefinition` configuration values.

use std::time::Instant;

use async_trait::async_trait;

use super::config::{AgentCapabilities, AuthMethod, BackendDefinition};
use super::error::CodingAgentError;
use super::status::{AuthStatus, HealthStatus, InstallationStatus};

/// Defines the behavior contract for a coding agent backend.
///
/// Each agent type (Claude Code, Kiro CLI, etc.) implements this trait
/// to customize health checks, authentication, and capability reporting
/// while sharing the common task execution pipeline.
#[async_trait]
pub trait CodingAgentBackend: Send + Sync {
    /// Returns the agent type identifier (e.g., "claude-code", "kiro-cli").
    fn agent_type(&self) -> &str;

    /// Returns human-readable display name.
    fn display_name(&self) -> &str;

    /// Check if the CLI binary is installed and return version info.
    ///
    /// Runs the configured `install_check_command` and parses the output
    /// for version information.
    async fn check_installation(&self) -> Result<InstallationStatus, CodingAgentError>;

    /// Perform a health/connectivity check against the agent endpoint.
    ///
    /// Sends an HTTP GET to the agent's ACP endpoint and measures latency.
    async fn health_check(&self) -> Result<HealthStatus, CodingAgentError>;

    /// Returns the authentication method required by this backend.
    fn auth_method(&self) -> AuthMethod;

    /// Validate that authentication credentials are still valid.
    async fn validate_auth(&self) -> Result<AuthStatus, CodingAgentError>;

    /// Returns the set of capabilities this backend supports.
    fn capabilities(&self) -> &AgentCapabilities;

    /// Returns installation instructions for this agent's package manager.
    fn installation_instructions(&self) -> &str;
}

/// A coding agent backend driven entirely by configuration values.
///
/// This struct implements `CodingAgentBackend` using a `BackendDefinition`
/// loaded from the gateway configuration file. It enables adding new agent
/// types without code changes.
pub struct ConfigDrivenBackend {
    /// The backend definition loaded from configuration.
    definition: BackendDefinition,
    /// Optional ACP endpoint URL for health checks.
    endpoint: Option<String>,
}

impl ConfigDrivenBackend {
    /// Create a new `ConfigDrivenBackend` from a backend definition.
    ///
    /// # Arguments
    /// * `definition` - The backend definition from configuration.
    /// * `endpoint` - Optional ACP endpoint URL for health checks.
    pub fn new(definition: BackendDefinition, endpoint: Option<String>) -> Self {
        Self {
            definition,
            endpoint,
        }
    }

    /// Parse a version string from command output.
    ///
    /// Looks for common version patterns (e.g., "v1.2.3", "1.2.3", "version 1.2.3")
    /// in the output text.
    fn parse_version(output: &str) -> Option<String> {
        // Try to find a version pattern in the output
        let trimmed = output.trim();
        if trimmed.is_empty() {
            return None;
        }

        // Look for patterns like "v1.2.3", "1.2.3", "version 1.2.3"
        for word in trimmed.split_whitespace() {
            let candidate = word.trim_start_matches('v').trim_end_matches(',');
            // Check if it looks like a version number (starts with digit, contains dots)
            if candidate.starts_with(|c: char| c.is_ascii_digit())
                && candidate.contains('.')
            {
                return Some(candidate.to_string());
            }
        }

        // Fallback: return the first line trimmed
        trimmed.lines().next().map(|s| s.trim().to_string())
    }
}

#[async_trait]
impl CodingAgentBackend for ConfigDrivenBackend {
    fn agent_type(&self) -> &str {
        &self.definition.agent_type
    }

    fn display_name(&self) -> &str {
        &self.definition.display_name
    }

    async fn check_installation(&self) -> Result<InstallationStatus, CodingAgentError> {
        let command_str = &self.definition.install_check_command;

        if command_str.trim().is_empty() {
            return Err(CodingAgentError::ConfigValidation(
                "install_check_command is empty".to_string(),
            ));
        }

        let program = command_str.split_whitespace().next().unwrap_or(command_str);

        // Build an extended PATH that includes common binary install locations
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        let extra_paths = format!(
            "{}:{}/go/bin:{}/.local/bin:{}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin",
            std::env::var("PATH").unwrap_or_default(),
            home, home, home
        );

        // Use a login shell with extended PATH to find recently installed binaries
        let shell_cmd = format!("export PATH=\"{}\"; {}", extra_paths, command_str);
        let output = tokio::process::Command::new("sh")
            .args(["-c", &shell_cmd])
            .output()
            .await;

        match output {
            Ok(output) if output.status.success() => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let version = Self::parse_version(&stdout);

                // Find the binary path with extended PATH
                let which_cmd = format!("export PATH=\"{}\"; which {}", extra_paths, program);
                let path = tokio::process::Command::new("sh")
                    .args(["-c", &which_cmd])
                    .output()
                    .await
                    .ok()
                    .and_then(|o| {
                        if o.status.success() {
                            let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
                            if p.is_empty() {
                                None
                            } else {
                                Some(std::path::PathBuf::from(p))
                            }
                        } else {
                            None
                        }
                    });

                Ok(InstallationStatus {
                    installed: true,
                    version,
                    path,
                })
            }
            Ok(_output) => {
                // Command ran but returned non-zero exit code — binary exists but may have issues
                Ok(InstallationStatus {
                    installed: false,
                    version: None,
                    path: None,
                })
            }
            Err(_) => {
                // Command failed to execute — binary not found
                Ok(InstallationStatus {
                    installed: false,
                    version: None,
                    path: None,
                })
            }
        }
    }

    async fn health_check(&self) -> Result<HealthStatus, CodingAgentError> {
        let endpoint = match &self.endpoint {
            Some(ep) => ep.clone(),
            None => {
                return Ok(HealthStatus {
                    reachable: false,
                    latency_ms: None,
                    version: None,
                });
            }
        };

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| {
                CodingAgentError::DelegationFailed(format!("failed to create HTTP client: {}", e))
            })?;

        let start = Instant::now();
        let response = client.get(&endpoint).send().await;
        let latency_ms = start.elapsed().as_millis() as u64;

        match response {
            Ok(resp) if resp.status().is_success() => Ok(HealthStatus {
                reachable: true,
                latency_ms: Some(latency_ms),
                version: None,
            }),
            Ok(_resp) => {
                // Endpoint responded but with an error status
                Ok(HealthStatus {
                    reachable: true,
                    latency_ms: Some(latency_ms),
                    version: None,
                })
            }
            Err(_) => Ok(HealthStatus {
                reachable: false,
                latency_ms: None,
                version: None,
            }),
        }
    }

    fn auth_method(&self) -> AuthMethod {
        self.definition.auth_method.clone()
    }

    async fn validate_auth(&self) -> Result<AuthStatus, CodingAgentError> {
        // For config-driven backends, auth validation depends on the auth method.
        // API key: check if the env var is set.
        // OAuth/CliLogin: would require more complex validation.
        // None: always valid.
        match &self.definition.auth_method {
            AuthMethod::ApiKey { env_var } => {
                if std::env::var(env_var).is_ok() {
                    Ok(AuthStatus::Valid { expires_at: None })
                } else {
                    Ok(AuthStatus::NotConfigured)
                }
            }
            AuthMethod::None => Ok(AuthStatus::Valid { expires_at: None }),
            AuthMethod::OAuth { .. } | AuthMethod::CliLogin { .. } => {
                // For OAuth and CLI login, we cannot validate without stored tokens.
                // Return NotConfigured as a safe default.
                Ok(AuthStatus::NotConfigured)
            }
        }
    }

    fn capabilities(&self) -> &AgentCapabilities {
        &self.definition.capabilities
    }

    fn installation_instructions(&self) -> &str {
        &self.definition.install_instructions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::config::{AgentCapabilities, AuthMethod, BackendDefinition};

    fn make_test_definition() -> BackendDefinition {
        BackendDefinition {
            agent_type: "test-agent".to_string(),
            display_name: "Test Agent".to_string(),
            cli_command: "test-cli".to_string(),
            install_check_command: "echo v1.2.3".to_string(),
            auth_method: AuthMethod::None,
            capabilities: AgentCapabilities {
                file_context: true,
                streaming_output: false,
                cost_reporting: true,
                cancellation: false,
            },
            install_instructions: "Run: cargo install test-agent".to_string(),
            install_instructions_windows: None,
            install_instructions_linux: None,
        }
    }

    #[test]
    fn test_config_driven_backend_agent_type() {
        let def = make_test_definition();
        let backend = ConfigDrivenBackend::new(def, None);
        assert_eq!(backend.agent_type(), "test-agent");
    }

    #[test]
    fn test_config_driven_backend_display_name() {
        let def = make_test_definition();
        let backend = ConfigDrivenBackend::new(def, None);
        assert_eq!(backend.display_name(), "Test Agent");
    }

    #[test]
    fn test_config_driven_backend_capabilities() {
        let def = make_test_definition();
        let backend = ConfigDrivenBackend::new(def, None);
        let caps = backend.capabilities();
        assert!(caps.file_context);
        assert!(!caps.streaming_output);
        assert!(caps.cost_reporting);
        assert!(!caps.cancellation);
    }

    #[test]
    fn test_config_driven_backend_installation_instructions() {
        let def = make_test_definition();
        let backend = ConfigDrivenBackend::new(def, None);
        assert_eq!(
            backend.installation_instructions(),
            "Run: cargo install test-agent"
        );
    }

    #[test]
    fn test_config_driven_backend_auth_method() {
        let def = make_test_definition();
        let backend = ConfigDrivenBackend::new(def, None);
        assert!(matches!(backend.auth_method(), AuthMethod::None));
    }

    #[test]
    fn test_parse_version_semver() {
        assert_eq!(
            ConfigDrivenBackend::parse_version("v1.2.3"),
            Some("1.2.3".to_string())
        );
    }

    #[test]
    fn test_parse_version_with_prefix() {
        assert_eq!(
            ConfigDrivenBackend::parse_version("claude-code version 1.0.5"),
            Some("1.0.5".to_string())
        );
    }

    #[test]
    fn test_parse_version_plain() {
        assert_eq!(
            ConfigDrivenBackend::parse_version("2.1.0"),
            Some("2.1.0".to_string())
        );
    }

    #[test]
    fn test_parse_version_empty() {
        assert_eq!(ConfigDrivenBackend::parse_version(""), None);
    }

    #[test]
    fn test_parse_version_no_version_pattern() {
        // Falls back to first line
        assert_eq!(
            ConfigDrivenBackend::parse_version("hello world"),
            Some("hello world".to_string())
        );
    }

    #[tokio::test]
    async fn test_check_installation_with_echo() {
        let def = BackendDefinition {
            agent_type: "echo-agent".to_string(),
            display_name: "Echo Agent".to_string(),
            cli_command: "echo".to_string(),
            install_check_command: "echo v3.5.1".to_string(),
            auth_method: AuthMethod::None,
            capabilities: AgentCapabilities::default(),
            install_instructions: "Already installed".to_string(),
            install_instructions_windows: None,
            install_instructions_linux: None,
        };
        let backend = ConfigDrivenBackend::new(def, None);
        let status = backend.check_installation().await.unwrap();
        assert!(status.installed);
        assert_eq!(status.version, Some("3.5.1".to_string()));
    }

    #[tokio::test]
    async fn test_check_installation_not_found() {
        let def = BackendDefinition {
            agent_type: "missing-agent".to_string(),
            display_name: "Missing Agent".to_string(),
            cli_command: "nonexistent_binary_xyz_12345".to_string(),
            install_check_command: "nonexistent_binary_xyz_12345 --version".to_string(),
            auth_method: AuthMethod::None,
            capabilities: AgentCapabilities::default(),
            install_instructions: "Install it somehow".to_string(),
            install_instructions_windows: None,
            install_instructions_linux: None,
        };
        let backend = ConfigDrivenBackend::new(def, None);
        let status = backend.check_installation().await.unwrap();
        assert!(!status.installed);
        assert_eq!(status.version, None);
    }

    #[tokio::test]
    async fn test_health_check_no_endpoint() {
        let def = make_test_definition();
        let backend = ConfigDrivenBackend::new(def, None);
        let status = backend.health_check().await.unwrap();
        assert!(!status.reachable);
        assert_eq!(status.latency_ms, None);
    }

    #[tokio::test]
    async fn test_health_check_unreachable_endpoint() {
        let def = make_test_definition();
        // Use a port that's almost certainly not listening
        let backend =
            ConfigDrivenBackend::new(def, Some("http://127.0.0.1:19999/health".to_string()));
        let status = backend.health_check().await.unwrap();
        assert!(!status.reachable);
    }

    #[tokio::test]
    async fn test_validate_auth_none() {
        let def = make_test_definition();
        let backend = ConfigDrivenBackend::new(def, None);
        let status = backend.validate_auth().await.unwrap();
        assert!(matches!(status, AuthStatus::Valid { expires_at: None }));
    }

    #[tokio::test]
    async fn test_validate_auth_api_key_not_set() {
        let def = BackendDefinition {
            agent_type: "api-agent".to_string(),
            display_name: "API Agent".to_string(),
            cli_command: "api-cli".to_string(),
            install_check_command: "api-cli --version".to_string(),
            auth_method: AuthMethod::ApiKey {
                env_var: "NONEXISTENT_TEST_KEY_XYZ_98765".to_string(),
            },
            capabilities: AgentCapabilities::default(),
            install_instructions: "".to_string(),
            install_instructions_windows: None,
            install_instructions_linux: None,
        };
        let backend = ConfigDrivenBackend::new(def, None);
        let status = backend.validate_auth().await.unwrap();
        assert!(matches!(status, AuthStatus::NotConfigured));
    }
}
