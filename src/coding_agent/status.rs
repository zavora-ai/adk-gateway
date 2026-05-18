//! Status and health models for coding agent monitoring.
//!
//! Provides types for tracking agent connection status, health check results,
//! installation verification, authentication state, and real-time status events
//! broadcast over WebSocket.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Connection status for a registered coding agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum AgentConnectionStatus {
    Connected,
    Disconnected { since: DateTime<Utc> },
    Error { message: String, since: DateTime<Utc> },
    Unknown,
}

/// Health check result from a connectivity probe against an agent endpoint.
#[derive(Debug, Clone)]
pub struct HealthStatus {
    /// Whether the agent endpoint is reachable.
    pub reachable: bool,
    /// Round-trip latency in milliseconds, if reachable.
    pub latency_ms: Option<u64>,
    /// Agent version string, if reported.
    pub version: Option<String>,
}

/// Installation check result for a coding agent CLI binary.
#[derive(Debug, Clone)]
pub struct InstallationStatus {
    /// Whether the CLI binary was found on the system PATH.
    pub installed: bool,
    /// Installed version string, if detected.
    pub version: Option<String>,
    /// Filesystem path to the CLI binary, if found.
    pub path: Option<PathBuf>,
}

/// Authentication status for a coding agent's credentials.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AuthStatus {
    Valid { expires_at: Option<DateTime<Utc>> },
    Expired { expired_at: DateTime<Utc> },
    NotConfigured,
}

/// WebSocket event emitted when a coding agent's connection status changes.
/// Used for real-time updates to the Control Panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStatusEvent {
    /// The unique identifier of the agent whose status changed.
    pub agent_id: String,
    /// The status before the transition.
    pub previous_status: AgentConnectionStatus,
    /// The status after the transition.
    pub new_status: AgentConnectionStatus,
    /// When the transition occurred.
    pub timestamp: DateTime<Utc>,
}
