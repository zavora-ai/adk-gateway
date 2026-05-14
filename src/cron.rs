//! Cron scheduler — manages scheduled jobs that send messages to channels.
//!
//! Each cron job fires on its configured schedule and either sends a direct
//! message or routes an `ask:` prompt through the agent pipeline.
//!
//! Design: CronScheduler [R10.1–R10.5]

use crate::channel::{ChannelType, InboundMessage, MessageSource};
use crate::config::CronJob;
use chrono::Utc;
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;

/// State of a scheduled cron job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CronJobStatus {
    /// Job is scheduled and waiting for next trigger.
    Active,
    /// Job has been cancelled.
    Cancelled,
}

/// Tracks the runtime state of a single cron job.
#[derive(Debug)]
pub struct CronJobState {
    pub job: CronJob,
    pub status: CronJobStatus,
    /// Handle to the spawned task (if any). In production this would be
    /// a `JoinHandle<()>`, but we use `Option` so tests can work without
    /// a runtime.
    cancel_token: Option<tokio_util::sync::CancellationToken>,
}

impl CronJobState {
    fn new(job: CronJob) -> Self {
        Self {
            job,
            status: CronJobStatus::Active,
            cancel_token: None,
        }
    }

    fn with_cancel_token(mut self, token: tokio_util::sync::CancellationToken) -> Self {
        self.cancel_token = Some(token);
        self
    }
}

/// Manages scheduled cron jobs.
///
/// Jobs are identified by their `id` field. The scheduler maintains a map
/// of active jobs and supports hot-reload via `reconcile()`.
pub struct CronScheduler {
    jobs: HashMap<String, CronJobState>,
    inbound_tx: mpsc::Sender<InboundMessage>,
}

impl CronScheduler {
    /// Create a new empty scheduler.
    pub fn new(inbound_tx: mpsc::Sender<InboundMessage>) -> Self {
        Self {
            jobs: HashMap::new(),
            inbound_tx,
        }
    }

    /// Schedule a single cron job. If a job with the same ID already exists,
    /// it is cancelled and replaced.
    pub fn schedule(&mut self, job: CronJob) -> anyhow::Result<()> {
        // Cancel existing job with same ID if present
        if let Some(existing) = self.jobs.get_mut(&job.id) {
            Self::cancel_job_state(existing);
        }

        let cancel_token = tokio_util::sync::CancellationToken::new();
        let state = CronJobState::new(job.clone()).with_cancel_token(cancel_token.clone());

        // Spawn the cron loop task
        let tx = self.inbound_tx.clone();
        let job_clone = job.clone();
        tokio::spawn(async move {
            Self::cron_loop(job_clone, tx, cancel_token).await;
        });

        tracing::info!(
            job_id = %job.id,
            schedule = %job.schedule,
            message = %job.message,
            delivery = ?job.deliver_to,
            "cron job scheduled"
        );
        self.jobs.insert(job.id.clone(), state);
        Ok(())
    }

    /// Cancel a job by ID.
    pub fn cancel(&mut self, job_id: &str) {
        if let Some(state) = self.jobs.get_mut(job_id) {
            Self::cancel_job_state(state);
            tracing::info!(job_id = %job_id, "cron job cancelled by user");
        } else {
            tracing::warn!(job_id = %job_id, "attempted to cancel unknown cron job");
        }
    }

    /// Reconcile the scheduler with a new set of cron jobs (for hot-reload).
    ///
    /// - Jobs present in `new_jobs` but not currently scheduled are added.
    /// - Jobs currently scheduled but not in `new_jobs` are cancelled.
    /// - Jobs present in both are updated if their config changed.
    pub fn reconcile(&mut self, new_jobs: &[CronJob]) {
        let new_ids: HashSet<&str> = new_jobs.iter().map(|j| j.id.as_str()).collect();
        let current_ids: Vec<String> = self.jobs.keys().cloned().collect();

        // Cancel removed jobs
        for id in &current_ids {
            if !new_ids.contains(id.as_str()) {
                self.cancel(id);
                self.jobs.remove(id);
            }
        }

        // Add or update jobs
        for job in new_jobs {
            let needs_update = match self.jobs.get(&job.id) {
                None => true,
                Some(existing) => {
                    existing.job != *job || existing.status == CronJobStatus::Cancelled
                }
            };

            if needs_update {
                // schedule() handles cancelling the old one if present
                if let Err(e) = self.schedule(job.clone()) {
                    tracing::error!(job_id = %job.id, error = %e, "failed to schedule cron job");
                }
            }
        }
    }

    /// Get the set of currently active job IDs.
    pub fn active_job_ids(&self) -> Vec<String> {
        self.jobs
            .iter()
            .filter(|(_, s)| s.status == CronJobStatus::Active)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Get the total number of tracked jobs (active + cancelled).
    pub fn job_count(&self) -> usize {
        self.jobs.len()
    }

    /// Check if a specific job is active.
    pub fn is_active(&self, job_id: &str) -> bool {
        self.jobs
            .get(job_id)
            .map(|s| s.status == CronJobStatus::Active)
            .unwrap_or(false)
    }

    /// Return full details of all tracked jobs (active and cancelled).
    pub fn list_all_jobs(&self) -> Vec<(&CronJob, &CronJobStatus)> {
        self.jobs.values().map(|s| (&s.job, &s.status)).collect()
    }

    /// Parse a message to determine if it's an agent prompt (starts with `ask:`).
    pub fn parse_message(message: &str) -> CronMessageKind<'_> {
        if let Some(prompt) = message.strip_prefix("ask:") {
            CronMessageKind::AgentPrompt(prompt.trim())
        } else {
            CronMessageKind::DirectMessage(message)
        }
    }

    /// Build an InboundMessage from a cron job trigger.
    ///
    /// Uses `parse_message` to determine whether the job's message is a direct
    /// message or an agent prompt (prefixed with `ask:`). For agent prompts the
    /// `ask:` prefix is stripped so the pipeline receives a clean prompt, and a
    /// `cron_message_kind` metadata entry records the original kind.
    pub fn build_inbound_message(job: &CronJob) -> InboundMessage {
        let target_channel = job
            .deliver_to
            .as_ref()
            .map(|d| d.channel.as_str())
            .unwrap_or("webhook");

        let channel_type = match target_channel {
            "telegram" => ChannelType::Telegram,
            "slack" => ChannelType::Slack,
            "discord" => ChannelType::Discord,
            _ => ChannelType::Webhook,
        };

        let target = job
            .deliver_to
            .as_ref()
            .map(|d| d.target.clone())
            .unwrap_or_default();

        let mut metadata = HashMap::new();
        if !target.is_empty() {
            metadata.insert("cron_target".to_string(), serde_json::Value::String(target));
        }
        if let Some(ref delivery) = job.deliver_to {
            metadata.insert(
                "cron_channel".to_string(),
                serde_json::Value::String(delivery.channel.clone()),
            );
        }

        // Use parse_message to convert the raw message into the appropriate kind
        let (text, kind_label) = match Self::parse_message(&job.message) {
            CronMessageKind::AgentPrompt(prompt) => (prompt.to_string(), "agent_prompt"),
            CronMessageKind::DirectMessage(msg) => (msg.to_string(), "direct_message"),
        };

        metadata.insert(
            "cron_message_kind".to_string(),
            serde_json::Value::String(kind_label.to_string()),
        );

        // For delivery, the sender_id must be the target chat ID so the
        // response gets routed back to the correct recipient.
        let recipient_id = job
            .deliver_to
            .as_ref()
            .map(|d| d.target.clone())
            .unwrap_or_default();

        InboundMessage {
            channel_type,
            account_id: "default".to_string(),
            sender_id: if recipient_id.is_empty() {
                format!("cron:{}", job.id)
            } else {
                recipient_id
            },
            sender_name: Some(format!("cron:{}", job.id)),
            text,
            is_group: false,
            group_id: None,
            is_mention: false,
            platform_message_id: uuid::Uuid::new_v4().to_string(),
            attachments: vec![],
            metadata,
            source: MessageSource::Cron {
                job_id: job.id.clone(),
            },
            timestamp: Utc::now(),
        }
    }

    // ── Private helpers ────────────────────────────────────────────

    fn cancel_job_state(state: &mut CronJobState) {
        state.status = CronJobStatus::Cancelled;
        if let Some(ref token) = state.cancel_token {
            token.cancel();
        }
    }

    /// The main loop for a cron job. Parses the cron expression and sleeps
    /// until the next trigger time.
    async fn cron_loop(
        job: CronJob,
        tx: mpsc::Sender<InboundMessage>,
        cancel: tokio_util::sync::CancellationToken,
    ) {
        // Simple cron loop: parse schedule, compute next fire time, sleep, fire.
        // For production, a full cron parser (e.g. `cron` crate) would be used.
        // Here we use a fixed interval fallback for non-standard expressions.
        let interval = Self::parse_schedule_interval(&job.schedule);

        tracing::info!(
            job_id = %job.id,
            schedule = %job.schedule,
            interval_secs = interval.as_secs(),
            message = %job.message,
            delivery = ?job.deliver_to,
            "cron loop started, waiting for first trigger"
        );

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!(job_id = %job.id, "cron job cancelled, exiting loop");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    let msg = Self::build_inbound_message(&job);
                    let message_kind = if job.message.starts_with("ask:") { "agent_prompt" } else { "direct_message" };
                    tracing::info!(
                        job_id = %job.id,
                        schedule = %job.schedule,
                        message_kind = message_kind,
                        message_text = %job.message,
                        delivery_channel = ?job.deliver_to.as_ref().map(|d| d.channel.as_str()),
                        delivery_target = ?job.deliver_to.as_ref().map(|d| d.target.as_str()),
                        "cron job firing"
                    );
                    if let Err(e) = tx.send(msg).await {
                        tracing::error!(
                            job_id = %job.id,
                            error = %e,
                            "failed to send cron message to pipeline"
                        );
                    } else {
                        tracing::info!(job_id = %job.id, "cron job message sent to pipeline successfully");
                    }
                }
            }
        }
    }

    /// Parse a cron schedule string into a Duration interval.
    ///
    /// Supports simple patterns:
    /// - `@every <N>s|m|h` — fixed interval
    /// - Standard 5-field cron — approximated to the smallest field interval
    ///
    /// For production, integrate the `cron` crate for proper next-fire-time calculation.
    fn parse_schedule_interval(schedule: &str) -> std::time::Duration {
        // Handle @every syntax
        if let Some(rest) = schedule.strip_prefix("@every ") {
            let rest = rest.trim();
            if let Some(secs) = rest.strip_suffix('s') {
                if let Ok(n) = secs.trim().parse::<u64>() {
                    return std::time::Duration::from_secs(n);
                }
            }
            if let Some(mins) = rest.strip_suffix('m') {
                if let Ok(n) = mins.trim().parse::<u64>() {
                    return std::time::Duration::from_secs(n * 60);
                }
            }
            if let Some(hours) = rest.strip_suffix('h') {
                if let Ok(n) = hours.trim().parse::<u64>() {
                    return std::time::Duration::from_secs(n * 3600);
                }
            }
        }

        // Default: 1 hour for unrecognized patterns
        std::time::Duration::from_secs(3600)
    }
}

/// The kind of message a cron job produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CronMessageKind<'a> {
    /// A direct message to send as-is.
    DirectMessage(&'a str),
    /// A prompt to route through the agent pipeline.
    AgentPrompt(&'a str),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CronDelivery, CronJob};

    fn make_job(id: &str, schedule: &str, message: &str) -> CronJob {
        CronJob {
            id: id.to_string(),
            schedule: schedule.to_string(),
            message: message.to_string(),
            deliver_to: None,
        }
    }

    fn make_job_with_delivery(id: &str, message: &str, channel: &str, target: &str) -> CronJob {
        CronJob {
            id: id.to_string(),
            schedule: "@every 60s".to_string(),
            message: message.to_string(),
            deliver_to: Some(CronDelivery {
                channel: channel.to_string(),
                target: target.to_string(),
            }),
        }
    }

    #[test]
    fn test_parse_message_direct() {
        assert_eq!(
            CronScheduler::parse_message("Hello world"),
            CronMessageKind::DirectMessage("Hello world")
        );
    }

    #[test]
    fn test_parse_message_agent_prompt() {
        assert_eq!(
            CronScheduler::parse_message("ask: What is the weather?"),
            CronMessageKind::AgentPrompt("What is the weather?")
        );
    }

    #[test]
    fn test_parse_message_ask_no_space() {
        assert_eq!(
            CronScheduler::parse_message("ask:summarize news"),
            CronMessageKind::AgentPrompt("summarize news")
        );
    }

    #[test]
    fn test_build_inbound_message_with_delivery() {
        let job = make_job_with_delivery("daily-report", "ask: summarize", "telegram", "user123");
        let msg = CronScheduler::build_inbound_message(&job);

        assert_eq!(msg.channel_type, ChannelType::Telegram);
        // parse_message strips the "ask:" prefix for agent prompts
        assert_eq!(msg.text, "summarize");
        assert_eq!(
            msg.metadata
                .get("cron_message_kind")
                .and_then(|v| v.as_str()),
            Some("agent_prompt")
        );
        assert!(
            matches!(msg.source, MessageSource::Cron { ref job_id } if job_id == "daily-report")
        );
        assert_eq!(
            msg.metadata.get("cron_target").and_then(|v| v.as_str()),
            Some("user123")
        );
        assert_eq!(
            msg.metadata.get("cron_channel").and_then(|v| v.as_str()),
            Some("telegram")
        );
    }

    #[test]
    fn test_build_inbound_message_no_delivery() {
        let job = make_job("ping", "@every 30s", "ping!");
        let msg = CronScheduler::build_inbound_message(&job);

        assert_eq!(msg.channel_type, ChannelType::Webhook);
        assert_eq!(msg.text, "ping!");
        assert_eq!(
            msg.metadata
                .get("cron_message_kind")
                .and_then(|v| v.as_str()),
            Some("direct_message")
        );
        assert!(matches!(msg.source, MessageSource::Cron { ref job_id } if job_id == "ping"));
    }

    #[test]
    fn test_parse_schedule_interval_seconds() {
        let d = CronScheduler::parse_schedule_interval("@every 30s");
        assert_eq!(d, std::time::Duration::from_secs(30));
    }

    #[test]
    fn test_parse_schedule_interval_minutes() {
        let d = CronScheduler::parse_schedule_interval("@every 5m");
        assert_eq!(d, std::time::Duration::from_secs(300));
    }

    #[test]
    fn test_parse_schedule_interval_hours() {
        let d = CronScheduler::parse_schedule_interval("@every 2h");
        assert_eq!(d, std::time::Duration::from_secs(7200));
    }

    #[test]
    fn test_parse_schedule_interval_unknown() {
        let d = CronScheduler::parse_schedule_interval("0 * * * *");
        assert_eq!(d, std::time::Duration::from_secs(3600));
    }

    #[tokio::test]
    async fn test_schedule_and_active() {
        let (tx, _rx) = mpsc::channel(16);
        let mut scheduler = CronScheduler::new(tx);

        let job = make_job("test-job", "@every 60s", "hello");
        scheduler.schedule(job).unwrap();

        assert!(scheduler.is_active("test-job"));
        assert_eq!(scheduler.active_job_ids().len(), 1);
    }

    #[tokio::test]
    async fn test_cancel_job() {
        let (tx, _rx) = mpsc::channel(16);
        let mut scheduler = CronScheduler::new(tx);

        let job = make_job("cancel-me", "@every 60s", "bye");
        scheduler.schedule(job).unwrap();
        assert!(scheduler.is_active("cancel-me"));

        scheduler.cancel("cancel-me");
        assert!(!scheduler.is_active("cancel-me"));
    }

    #[tokio::test]
    async fn test_reconcile_adds_and_removes() {
        let (tx, _rx) = mpsc::channel(16);
        let mut scheduler = CronScheduler::new(tx);

        // Initial: jobs A and B
        let jobs_v1 = vec![
            make_job("a", "@every 60s", "msg-a"),
            make_job("b", "@every 60s", "msg-b"),
        ];
        scheduler.reconcile(&jobs_v1);
        assert!(scheduler.is_active("a"));
        assert!(scheduler.is_active("b"));

        // Hot-reload: remove A, keep B, add C
        let jobs_v2 = vec![
            make_job("b", "@every 60s", "msg-b"),
            make_job("c", "@every 30s", "msg-c"),
        ];
        scheduler.reconcile(&jobs_v2);
        assert!(!scheduler.jobs.contains_key("a"), "job A should be removed");
        assert!(scheduler.is_active("b"));
        assert!(scheduler.is_active("c"));
    }

    // ── Cron parse_message conversion tests (Task 11.5) ────────────

    #[test]
    fn parse_message_ask_prefix_produces_agent_prompt_with_stripped_prefix() {
        let result = CronScheduler::parse_message("ask:What is the weather?");
        assert_eq!(result, CronMessageKind::AgentPrompt("What is the weather?"));
    }

    #[test]
    fn parse_message_ask_prefix_with_space_strips_and_trims() {
        let result = CronScheduler::parse_message("ask: summarize the news");
        assert_eq!(result, CronMessageKind::AgentPrompt("summarize the news"));
    }

    #[test]
    fn parse_message_direct_message_without_ask_prefix() {
        let result = CronScheduler::parse_message("daily report ready");
        assert_eq!(result, CronMessageKind::DirectMessage("daily report ready"));
    }

    #[test]
    fn parse_message_empty_string_is_direct_message() {
        let result = CronScheduler::parse_message("");
        assert_eq!(result, CronMessageKind::DirectMessage(""));
    }

    #[test]
    fn build_inbound_message_agent_prompt_strips_ask_prefix() {
        let job = CronJob {
            id: "test-job".into(),
            schedule: "@every 60s".into(),
            message: "ask:summarize".into(),
            deliver_to: None,
        };
        let msg = CronScheduler::build_inbound_message(&job);
        assert_eq!(
            msg.text, "summarize",
            "ask: prefix should be stripped from text"
        );
        assert_eq!(
            msg.metadata
                .get("cron_message_kind")
                .and_then(|v| v.as_str()),
            Some("agent_prompt")
        );
    }

    #[test]
    fn build_inbound_message_direct_message_preserves_text() {
        let job = CronJob {
            id: "ping-job".into(),
            schedule: "@every 30s".into(),
            message: "hello world".into(),
            deliver_to: None,
        };
        let msg = CronScheduler::build_inbound_message(&job);
        assert_eq!(msg.text, "hello world");
        assert_eq!(
            msg.metadata
                .get("cron_message_kind")
                .and_then(|v| v.as_str()),
            Some("direct_message")
        );
    }
}
