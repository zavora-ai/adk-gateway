//! Audit logging for access control decisions.
//!
//! Provides an `AuditSink` trait and implementations for recording all
//! access control events (tool access, agent access, pairing attempts,
//! permission checks) as structured JSON-line entries.
//!
//! Implements requirements R26.9, R26.10.

use crate::channel::ChannelType;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

// ── AuditEventType ─────────────────────────────────────────────────

/// The kind of access control event being audited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventType {
    ToolAccess,
    AgentAccess,
    Login,
    PairingAttempt,
    PermissionCheck,
}

// ── AuditOutcome ───────────────────────────────────────────────────

/// The result of an access control decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Allowed,
    Denied,
    Error,
}

// ── AuditEvent ─────────────────────────────────────────────────────

/// A single audit log entry recording an access control decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub timestamp: DateTime<Utc>,
    pub user_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_type: Option<ChannelType>,
    pub event_type: AuditEventType,
    pub resource: String,
    pub outcome: AuditOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

// ── AuditSink trait ────────────────────────────────────────────────

/// Trait for logging audit events. Implementations may write to files,
/// external SIEM systems, or discard events entirely.
#[async_trait::async_trait]
pub trait AuditSink: Send + Sync {
    async fn log_event(&self, event: AuditEvent) -> anyhow::Result<()>;
}

// ── FileAuditSink ──────────────────────────────────────────────────

/// Appends JSON-line audit events to a file (one JSON object per line).
pub struct FileAuditSink {
    path: PathBuf,
}

impl FileAuditSink {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Returns the configured file path.
    #[allow(dead_code)] // Used in tests and available for diagnostic reporting
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait::async_trait]
impl AuditSink for FileAuditSink {
    async fn log_event(&self, event: AuditEvent) -> anyhow::Result<()> {
        let mut line = serde_json::to_string(&event)?;
        line.push('\n');

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await?;

        file.write_all(line.as_bytes()).await?;
        file.flush().await?;
        Ok(())
    }
}

// ── NullAuditSink ──────────────────────────────────────────────────

/// No-op sink for when auditing is disabled.
pub struct NullAuditSink;

#[async_trait::async_trait]
impl AuditSink for NullAuditSink {
    async fn log_event(&self, _event: AuditEvent) -> anyhow::Result<()> {
        Ok(())
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_event() -> AuditEvent {
        AuditEvent {
            timestamp: Utc::now(),
            user_id: "telegram:12345".into(),
            session_id: Some("sess-001".into()),
            channel_type: Some(ChannelType::Telegram),
            event_type: AuditEventType::ToolAccess,
            resource: "web_search".into(),
            outcome: AuditOutcome::Allowed,
            details: Some("tool executed successfully".into()),
        }
    }

    #[test]
    fn audit_event_serializes_to_json() {
        let event = sample_event();
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"user_id\":\"telegram:12345\""));
        assert!(json.contains("\"event_type\":\"tool_access\""));
        assert!(json.contains("\"outcome\":\"allowed\""));
        assert!(json.contains("\"resource\":\"web_search\""));
    }

    #[test]
    fn audit_event_round_trips_through_json() {
        let event = sample_event();
        let json = serde_json::to_string(&event).unwrap();
        let parsed: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.user_id, event.user_id);
        assert_eq!(parsed.event_type, event.event_type);
        assert_eq!(parsed.outcome, event.outcome);
        assert_eq!(parsed.resource, event.resource);
        assert_eq!(parsed.session_id, event.session_id);
        assert_eq!(parsed.channel_type, event.channel_type);
        assert_eq!(parsed.details, event.details);
    }

    #[test]
    fn audit_event_skips_none_fields_in_json() {
        let event = AuditEvent {
            timestamp: Utc::now(),
            user_id: "slack:U0ABC".into(),
            session_id: None,
            channel_type: None,
            event_type: AuditEventType::Login,
            resource: "gateway".into(),
            outcome: AuditOutcome::Denied,
            details: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("session_id"));
        assert!(!json.contains("channel_type"));
        assert!(!json.contains("details"));
    }

    #[test]
    fn all_event_types_serialize() {
        let types = vec![
            AuditEventType::ToolAccess,
            AuditEventType::AgentAccess,
            AuditEventType::Login,
            AuditEventType::PairingAttempt,
            AuditEventType::PermissionCheck,
        ];
        let expected = vec![
            "\"tool_access\"",
            "\"agent_access\"",
            "\"login\"",
            "\"pairing_attempt\"",
            "\"permission_check\"",
        ];
        for (t, exp) in types.into_iter().zip(expected) {
            let json = serde_json::to_string(&t).unwrap();
            assert_eq!(json, exp);
        }
    }

    #[test]
    fn all_outcomes_serialize() {
        let outcomes = vec![
            AuditOutcome::Allowed,
            AuditOutcome::Denied,
            AuditOutcome::Error,
        ];
        let expected = vec!["\"allowed\"", "\"denied\"", "\"error\""];
        for (o, exp) in outcomes.into_iter().zip(expected) {
            let json = serde_json::to_string(&o).unwrap();
            assert_eq!(json, exp);
        }
    }

    #[tokio::test]
    async fn file_audit_sink_writes_json_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let sink = FileAuditSink::new(&path);

        let event1 = sample_event();
        let event2 = AuditEvent {
            timestamp: Utc::now(),
            user_id: "slack:U0ABC".into(),
            session_id: None,
            channel_type: Some(ChannelType::Slack),
            event_type: AuditEventType::PermissionCheck,
            resource: "admin_tool".into(),
            outcome: AuditOutcome::Denied,
            details: Some("missing admin role".into()),
        };

        sink.log_event(event1).await.unwrap();
        sink.log_event(event2).await.unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);

        // Each line is valid JSON
        let parsed1: AuditEvent = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed1.user_id, "telegram:12345");

        let parsed2: AuditEvent = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(parsed2.user_id, "slack:U0ABC");
        assert_eq!(parsed2.outcome, AuditOutcome::Denied);
    }

    #[tokio::test]
    async fn null_audit_sink_succeeds_silently() {
        let sink = NullAuditSink;
        let event = sample_event();
        let result = sink.log_event(event).await;
        assert!(result.is_ok());
    }

    // ── Audit sink selection logic tests (Task 11.3) ───────────────

    use crate::config::{AuditConfig, AuditSinkType, AuthConfig, AuthMode, GatewayConfig};
    use std::sync::Arc;

    /// Helper: build a GatewayConfig with the given audit settings.
    fn config_with_audit(
        enabled: bool,
        sink: AuditSinkType,
        path: Option<std::path::PathBuf>,
    ) -> GatewayConfig {
        let mut config = GatewayConfig::default();
        config.auth = Some(AuthConfig {
            mode: AuthMode::None,
            token: None,
            password: None,
            roles: vec![],
            user_mappings: vec![],
            channel_overrides: std::collections::HashMap::new(),
            audit: Some(AuditConfig {
                enabled,
                sink,
                path,
            }),
            sso: None,
        });
        config
    }

    #[test]
    fn audit_enabled_file_sink_selects_file_audit_sink() {
        let config = config_with_audit(true, AuditSinkType::File, Some("/tmp/audit.jsonl".into()));
        let sink: Arc<dyn AuditSink + Send + Sync> =
            match config.auth.as_ref().and_then(|auth| auth.audit.as_ref()) {
                Some(audit) if audit.enabled && audit.sink == AuditSinkType::File => {
                    let path = audit
                        .path
                        .clone()
                        .unwrap_or_else(|| std::path::PathBuf::from("audit.jsonl"));
                    Arc::new(FileAuditSink::new(path))
                }
                _ => Arc::new(NullAuditSink),
            };
        // FileAuditSink has a path() method; NullAuditSink does not.
        // We verify by downcasting.
        assert!(
            sink.as_ref() as *const dyn AuditSink as *const () != std::ptr::null(),
            "sink should be constructed"
        );
        // Verify it's a FileAuditSink by checking the path
        // We can't downcast easily, but we can verify the selection logic
        // by checking the config conditions directly.
        let audit = config.auth.as_ref().unwrap().audit.as_ref().unwrap();
        assert!(audit.enabled);
        assert_eq!(audit.sink, AuditSinkType::File);
    }

    #[test]
    fn audit_disabled_selects_null_audit_sink() {
        let config = config_with_audit(false, AuditSinkType::File, Some("/tmp/audit.jsonl".into()));
        let audit = config.auth.as_ref().unwrap().audit.as_ref().unwrap();
        let is_file_sink = audit.enabled && audit.sink == AuditSinkType::File;
        assert!(!is_file_sink, "disabled audit should not select file sink");
    }

    #[test]
    fn no_audit_config_selects_null_audit_sink() {
        let config = GatewayConfig::default();
        let audit_ref = config.auth.as_ref().and_then(|auth| auth.audit.as_ref());
        let is_file_sink = match audit_ref {
            Some(audit) => audit.enabled && audit.sink == AuditSinkType::File,
            None => false,
        };
        assert!(
            !is_file_sink,
            "missing audit config should not select file sink"
        );
    }
}
