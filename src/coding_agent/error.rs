//! Error types for the coding agent subsystem.

use thiserror::Error;

#[cfg(feature = "acp")]
use crate::acp::AcpError;

/// Errors that can occur within the coding agent subsystem.
#[derive(Debug, Error)]
pub enum CodingAgentError {
    /// Configuration validation failed (e.g., missing required fields in backend definition).
    #[error("Configuration validation error: {0}")]
    ConfigValidation(String),

    /// The specified agent was not found in the registry.
    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    /// The agent is disconnected and cannot accept tasks.
    #[error("Agent disconnected: {0}")]
    AgentDisconnected(String),

    /// Task delegation failed.
    #[error("Delegation failed: {0}")]
    DelegationFailed(String),

    /// The task queue is full and cannot accept more tasks.
    #[error("Task queue is full (capacity: {capacity})")]
    QueueFull { capacity: usize },

    /// A file path is outside the allowed workspace directories.
    #[error("Workspace violation: path {path} is outside allowed workspaces")]
    WorkspaceViolation { path: String },

    /// The task exceeded its configured timeout.
    #[error("Task timed out after {elapsed_secs}s (limit: {limit_secs}s)")]
    Timeout { elapsed_secs: u64, limit_secs: u64 },

    /// The task exceeded its configured cost cap.
    #[error("Cost cap exceeded: spent ${spent_usd:.4} (cap: ${cap_usd:.4})")]
    CostCapExceeded { spent_usd: f64, cap_usd: f64 },

    /// The agent provider returned a rate limit response.
    #[error("Rate limited, retry after {retry_after_secs:?}s")]
    RateLimited { retry_after_secs: Option<u64> },

    /// A tool approval request was rejected by the user.
    #[error("Approval rejected for operation: {0}")]
    ApprovalRejected(String),

    /// A tool approval request timed out waiting for user response.
    #[error("Approval timed out after {timeout_secs}s")]
    ApprovalTimeout { timeout_secs: u64 },
}

#[cfg(feature = "acp")]
impl From<AcpError> for CodingAgentError {
    fn from(err: AcpError) -> Self {
        match err {
            AcpError::EndpointUnreachable { endpoint, reason } => {
                CodingAgentError::AgentDisconnected(format!(
                    "endpoint unreachable at {}: {}",
                    endpoint, reason
                ))
            }
            AcpError::Timeout {
                timeout_secs,
                agent_type: _,
            } => CodingAgentError::Timeout {
                elapsed_secs: timeout_secs,
                limit_secs: timeout_secs,
            },
            AcpError::EndpointError { status, message } => {
                if status == 429 {
                    CodingAgentError::RateLimited {
                        retry_after_secs: None,
                    }
                } else {
                    CodingAgentError::DelegationFailed(format!(
                        "endpoint returned status {}: {}",
                        status, message
                    ))
                }
            }
            AcpError::RequestBuildError(msg) => {
                CodingAgentError::DelegationFailed(format!("failed to build request: {}", msg))
            }
            AcpError::ResponseParseError(msg) => {
                CodingAgentError::DelegationFailed(format!("failed to parse response: {}", msg))
            }
            AcpError::ProgressChannelClosed => {
                CodingAgentError::DelegationFailed("progress channel closed".to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_validation_error_display() {
        let err = CodingAgentError::ConfigValidation("missing cli_command field".to_string());
        assert_eq!(
            err.to_string(),
            "Configuration validation error: missing cli_command field"
        );
    }

    #[test]
    fn test_agent_not_found_error_display() {
        let err = CodingAgentError::AgentNotFound("claude-code-1".to_string());
        assert_eq!(err.to_string(), "Agent not found: claude-code-1");
    }

    #[test]
    fn test_queue_full_error_display() {
        let err = CodingAgentError::QueueFull { capacity: 3 };
        assert_eq!(err.to_string(), "Task queue is full (capacity: 3)");
    }

    #[test]
    fn test_workspace_violation_error_display() {
        let err = CodingAgentError::WorkspaceViolation {
            path: "/etc/passwd".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Workspace violation: path /etc/passwd is outside allowed workspaces"
        );
    }

    #[test]
    fn test_timeout_error_display() {
        let err = CodingAgentError::Timeout {
            elapsed_secs: 1800,
            limit_secs: 1800,
        };
        assert_eq!(
            err.to_string(),
            "Task timed out after 1800s (limit: 1800s)"
        );
    }

    #[test]
    fn test_cost_cap_exceeded_error_display() {
        let err = CodingAgentError::CostCapExceeded {
            spent_usd: 5.1234,
            cap_usd: 5.0000,
        };
        let display = err.to_string();
        assert!(display.contains("Cost cap exceeded"));
        assert!(display.contains("5.1234"));
        assert!(display.contains("5.0000"));
    }

    #[test]
    fn test_rate_limited_error_display() {
        let err = CodingAgentError::RateLimited {
            retry_after_secs: Some(60),
        };
        assert_eq!(err.to_string(), "Rate limited, retry after Some(60)s");

        let err_none = CodingAgentError::RateLimited {
            retry_after_secs: None,
        };
        assert_eq!(err_none.to_string(), "Rate limited, retry after Nones");
    }

    #[test]
    fn test_approval_rejected_error_display() {
        let err = CodingAgentError::ApprovalRejected("file deletion".to_string());
        assert_eq!(
            err.to_string(),
            "Approval rejected for operation: file deletion"
        );
    }

    #[test]
    fn test_approval_timeout_error_display() {
        let err = CodingAgentError::ApprovalTimeout { timeout_secs: 120 };
        assert_eq!(err.to_string(), "Approval timed out after 120s");
    }

    #[cfg(feature = "acp")]
    mod acp_conversion_tests {
        use super::*;
        use crate::acp::AcpError;

        #[test]
        fn test_from_acp_error_endpoint_unreachable() {
            let acp_err = AcpError::EndpointUnreachable {
                endpoint: "http://localhost:3000/acp".to_string(),
                reason: "connection refused".to_string(),
            };
            let err: CodingAgentError = acp_err.into();
            match err {
                CodingAgentError::AgentDisconnected(msg) => {
                    assert!(msg.contains("localhost:3000"));
                    assert!(msg.contains("connection refused"));
                }
                other => panic!("Expected AgentDisconnected, got: {:?}", other),
            }
        }

        #[test]
        fn test_from_acp_error_timeout() {
            let acp_err = AcpError::Timeout {
                timeout_secs: 300,
                agent_type: "Claude Code".to_string(),
            };
            let err: CodingAgentError = acp_err.into();
            match err {
                CodingAgentError::Timeout {
                    elapsed_secs,
                    limit_secs,
                } => {
                    assert_eq!(elapsed_secs, 300);
                    assert_eq!(limit_secs, 300);
                }
                other => panic!("Expected Timeout, got: {:?}", other),
            }
        }

        #[test]
        fn test_from_acp_error_rate_limit() {
            let acp_err = AcpError::EndpointError {
                status: 429,
                message: "Too Many Requests".to_string(),
            };
            let err: CodingAgentError = acp_err.into();
            match err {
                CodingAgentError::RateLimited { retry_after_secs } => {
                    assert_eq!(retry_after_secs, None);
                }
                other => panic!("Expected RateLimited, got: {:?}", other),
            }
        }

        #[test]
        fn test_from_acp_error_endpoint_error() {
            let acp_err = AcpError::EndpointError {
                status: 500,
                message: "Internal Server Error".to_string(),
            };
            let err: CodingAgentError = acp_err.into();
            match err {
                CodingAgentError::DelegationFailed(msg) => {
                    assert!(msg.contains("500"));
                    assert!(msg.contains("Internal Server Error"));
                }
                other => panic!("Expected DelegationFailed, got: {:?}", other),
            }
        }

        #[test]
        fn test_from_acp_error_request_build() {
            let acp_err = AcpError::RequestBuildError("invalid JSON".to_string());
            let err: CodingAgentError = acp_err.into();
            match err {
                CodingAgentError::DelegationFailed(msg) => {
                    assert!(msg.contains("failed to build request"));
                    assert!(msg.contains("invalid JSON"));
                }
                other => panic!("Expected DelegationFailed, got: {:?}", other),
            }
        }

        #[test]
        fn test_from_acp_error_response_parse() {
            let acp_err = AcpError::ResponseParseError("unexpected EOF".to_string());
            let err: CodingAgentError = acp_err.into();
            match err {
                CodingAgentError::DelegationFailed(msg) => {
                    assert!(msg.contains("failed to parse response"));
                    assert!(msg.contains("unexpected EOF"));
                }
                other => panic!("Expected DelegationFailed, got: {:?}", other),
            }
        }

        #[test]
        fn test_from_acp_error_progress_channel_closed() {
            let acp_err = AcpError::ProgressChannelClosed;
            let err: CodingAgentError = acp_err.into();
            match err {
                CodingAgentError::DelegationFailed(msg) => {
                    assert!(msg.contains("progress channel closed"));
                }
                other => panic!("Expected DelegationFailed, got: {:?}", other),
            }
        }
    }
}
