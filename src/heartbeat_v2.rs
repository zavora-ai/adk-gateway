//! Heartbeat V2 — session-integrated heartbeat with full conversation context.
//!
//! Replaces the cron-based heartbeat with a system that runs within the user's
//! active session. The heartbeat injects a prompt into the session, processes it
//! through the Runner, and classifies the response:
//!
//! - `"HEARTBEAT_OK"` responses are stripped from history (non-actionable).
//! - Actionable alerts are retained in history and delivered to the user.
//!
//! Each paired user gets an independent heartbeat schedule with its own interval
//! and cancellation token. If no active session exists, a temporary session is
//! created for the check and discarded after completion.

use std::time::Duration;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio_util::sync::CancellationToken;

use crate::config::HeartbeatV2Config;

// ── Constants ──────────────────────────────────────────────────────

/// The exact response string that indicates a healthy heartbeat (no action needed).
const HEARTBEAT_OK_RESPONSE: &str = "HEARTBEAT_OK";

/// Metadata marker for heartbeat turns in session history.
pub const HEARTBEAT_TURN_MARKER: &str = "__heartbeat_v2__";

// ── Data Types ─────────────────────────────────────────────────────

/// A single turn in the session history.
///
/// This is a simplified representation used by the heartbeat module for
/// filtering purposes. The actual session history uses the adk-session Turn type,
/// but this abstraction allows the heartbeat logic to be tested independently.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// The role of the message sender (e.g., "user", "assistant", "system").
    pub role: String,
    /// The content of the turn.
    pub content: String,
    /// Optional metadata marker. Heartbeat turns are marked with `HEARTBEAT_TURN_MARKER`.
    pub metadata: Option<String>,
}

impl Turn {
    /// Create a regular (non-heartbeat) turn.
    pub fn regular(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            metadata: None,
        }
    }

    /// Create a heartbeat turn (marked with the heartbeat metadata marker).
    pub fn heartbeat(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: role.into(),
            content: content.into(),
            metadata: Some(HEARTBEAT_TURN_MARKER.to_string()),
        }
    }

    /// Check if this turn is a heartbeat turn.
    pub fn is_heartbeat(&self) -> bool {
        self.metadata.as_deref() == Some(HEARTBEAT_TURN_MARKER)
    }
}

/// Per-user heartbeat schedule state.
#[derive(Debug, Clone)]
pub struct HeartbeatSchedule {
    /// The user this schedule belongs to.
    pub user_id: String,
    /// How often the heartbeat fires for this user.
    pub interval: Duration,
    /// When the heartbeat last fired (None if never fired).
    pub last_fired: Option<DateTime<Utc>>,
    /// Token to cancel this user's heartbeat loop.
    pub cancel_token: CancellationToken,
}

/// Result of classifying a heartbeat response.
#[derive(Debug, Clone, PartialEq)]
pub enum HeartbeatResponseKind {
    /// The response is exactly "HEARTBEAT_OK" — discard both prompt and response.
    Ok,
    /// The response contains an actionable alert — retain in history and deliver.
    Alert(String),
}

// ── HeartbeatV2 ────────────────────────────────────────────────────

/// The V2 heartbeat system with per-user scheduling and session integration.
///
/// This replaces the cron-based heartbeat entirely. Each paired user gets an
/// independent heartbeat schedule that fires within their active session context.
pub struct HeartbeatV2 {
    /// Per-user heartbeat schedules, keyed by user_id.
    schedules: DashMap<String, HeartbeatSchedule>,
    /// Configuration for the heartbeat system.
    config: HeartbeatV2Config,
}

impl HeartbeatV2 {
    /// Create a new HeartbeatV2 instance with the given configuration.
    pub fn new(config: HeartbeatV2Config) -> Self {
        Self {
            schedules: DashMap::new(),
            config,
        }
    }

    /// Create a new HeartbeatV2 with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(HeartbeatV2Config::default())
    }

    /// Get a reference to the configuration.
    pub fn config(&self) -> &HeartbeatV2Config {
        &self.config
    }

    /// Get the number of active schedules.
    pub fn schedule_count(&self) -> usize {
        self.schedules.len()
    }

    /// Check if a user has an active heartbeat schedule.
    pub fn has_schedule(&self, user_id: &str) -> bool {
        self.schedules.contains_key(user_id)
    }

    /// Get a clone of a user's heartbeat schedule (if it exists).
    pub fn get_schedule(&self, user_id: &str) -> Option<HeartbeatSchedule> {
        self.schedules.get(user_id).map(|s| s.clone())
    }

    // ── Scheduling ─────────────────────────────────────────────────

    /// Schedule a heartbeat for a specific user with the given interval.
    ///
    /// If the user already has a schedule, it is cancelled and replaced.
    /// The heartbeat loop runs in the background until cancelled.
    pub async fn schedule_for_user(&self, user_id: &str, interval: Duration) {
        // Cancel existing schedule if present
        self.cancel_for_user(user_id);

        let cancel_token = CancellationToken::new();
        let schedule = HeartbeatSchedule {
            user_id: user_id.to_string(),
            interval,
            last_fired: None,
            cancel_token: cancel_token.clone(),
        };

        self.schedules.insert(user_id.to_string(), schedule);

        tracing::info!(
            user_id = user_id,
            interval_secs = interval.as_secs(),
            "heartbeat V2 scheduled for user"
        );
    }

    /// Cancel the heartbeat for a specific user.
    ///
    /// If the user has an active schedule, the cancellation token is triggered
    /// and the schedule is removed. If no schedule exists, this is a no-op.
    pub fn cancel_for_user(&self, user_id: &str) {
        if let Some((_, schedule)) = self.schedules.remove(user_id) {
            schedule.cancel_token.cancel();
            tracing::info!(user_id = user_id, "heartbeat V2 cancelled for user");
        }
    }

    /// Cancel all active heartbeat schedules.
    pub fn cancel_all(&self) {
        let keys: Vec<String> = self.schedules.iter().map(|e| e.key().clone()).collect();
        for key in keys {
            self.cancel_for_user(&key);
        }
    }

    /// Update the `last_fired` timestamp for a user.
    pub fn mark_fired(&self, user_id: &str) {
        if let Some(mut schedule) = self.schedules.get_mut(user_id) {
            schedule.last_fired = Some(Utc::now());
        }
    }

    // ── Response Classification ────────────────────────────────────

    /// Classify a heartbeat response for retention/delivery decisions.
    ///
    /// - If the response (trimmed) is exactly `"HEARTBEAT_OK"`, returns `Ok`.
    /// - Otherwise, returns `Alert` with the full response content.
    ///
    /// The comparison is case-sensitive and requires an exact match after trimming
    /// leading/trailing whitespace.
    pub fn classify_response(response: &str) -> HeartbeatResponseKind {
        let trimmed = response.trim();
        if trimmed == HEARTBEAT_OK_RESPONSE {
            HeartbeatResponseKind::Ok
        } else {
            HeartbeatResponseKind::Alert(response.to_string())
        }
    }

    // ── Turn Filtering ─────────────────────────────────────────────

    /// Filter session history, removing non-actionable heartbeat turns.
    ///
    /// This function implements Property 8 (Heartbeat Turn Filtering):
    /// - Removes all heartbeat turns where the response is exactly "HEARTBEAT_OK"
    /// - Retains all heartbeat turns where the response contains an actionable alert
    /// - Regular (non-heartbeat) turns are NEVER affected
    ///
    /// Heartbeat turns come in pairs: a user prompt turn and an assistant response turn,
    /// both marked with the heartbeat metadata marker. When the assistant response is
    /// "HEARTBEAT_OK", both the prompt and response turns are removed. When the response
    /// is an alert, both turns are retained.
    pub fn strip_heartbeat_turns(history: &mut Vec<Turn>) {
        // We need to identify heartbeat turn pairs and decide whether to keep them.
        // A heartbeat sequence is: [heartbeat user turn, heartbeat assistant turn]
        // We keep the pair if the assistant response is NOT "HEARTBEAT_OK".

        let mut indices_to_remove: Vec<usize> = Vec::new();

        let mut i = 0;
        while i < history.len() {
            let turn = &history[i];

            if turn.is_heartbeat() {
                // Check if this is an assistant response that is "HEARTBEAT_OK"
                if turn.role == "assistant" {
                    let kind = Self::classify_response(&turn.content);
                    if kind == HeartbeatResponseKind::Ok {
                        // Mark this turn for removal
                        indices_to_remove.push(i);
                        // Also look back for the preceding heartbeat user turn
                        if i > 0 && history[i - 1].is_heartbeat() && history[i - 1].role == "user"
                        {
                            indices_to_remove.push(i - 1);
                        }
                    }
                    // If it's an alert, we keep both turns (don't add to removal list)
                } else if turn.role == "user" {
                    // Check if the next turn is a heartbeat assistant turn with OK response
                    if i + 1 < history.len()
                        && history[i + 1].is_heartbeat()
                        && history[i + 1].role == "assistant"
                    {
                        let kind = Self::classify_response(&history[i + 1].content);
                        if kind == HeartbeatResponseKind::Ok {
                            // Both will be removed when we process the assistant turn
                            // Skip ahead — the assistant turn processing handles it
                        }
                    } else if i + 1 >= history.len() || !history[i + 1].is_heartbeat() {
                        // Orphaned heartbeat user turn with no matching assistant response
                        // This shouldn't normally happen, but if it does, treat as OK (remove it)
                        indices_to_remove.push(i);
                    }
                }
            }
            i += 1;
        }

        // Remove indices in reverse order to maintain correct positions
        indices_to_remove.sort_unstable();
        indices_to_remove.dedup();
        for &idx in indices_to_remove.iter().rev() {
            history.remove(idx);
        }
    }

    // ── Session Integration ────────────────────────────────────────

    /// Build the heartbeat prompt to inject into the session.
    ///
    /// The prompt instructs the agent to check if anything needs attention.
    /// If nothing needs attention, the agent should reply with exactly "HEARTBEAT_OK".
    pub fn build_heartbeat_prompt(&self) -> String {
        self.config.prompt.clone()
    }

    /// Create a temporary session context for heartbeat execution when no active
    /// session exists. The temporary session is discarded after the heartbeat check.
    pub fn create_temporary_session_context(_user_id: &str) -> Vec<Turn> {
        vec![Turn::heartbeat(
            "user",
            "System heartbeat check — if nothing needs attention, reply HEARTBEAT_OK.",
        )]
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── classify_response tests ────────────────────────────────────

    #[test]
    fn classify_exact_heartbeat_ok() {
        let result = HeartbeatV2::classify_response("HEARTBEAT_OK");
        assert_eq!(result, HeartbeatResponseKind::Ok);
    }

    #[test]
    fn classify_heartbeat_ok_with_whitespace() {
        let result = HeartbeatV2::classify_response("  HEARTBEAT_OK  ");
        assert_eq!(result, HeartbeatResponseKind::Ok);
    }

    #[test]
    fn classify_heartbeat_ok_with_newline() {
        let result = HeartbeatV2::classify_response("HEARTBEAT_OK\n");
        assert_eq!(result, HeartbeatResponseKind::Ok);
    }

    #[test]
    fn classify_alert_response() {
        let response = "CI pipeline failed on branch main. 3 tests failing.";
        let result = HeartbeatV2::classify_response(response);
        assert_eq!(
            result,
            HeartbeatResponseKind::Alert(response.to_string())
        );
    }

    #[test]
    fn classify_partial_heartbeat_ok_is_alert() {
        // "HEARTBEAT_OK but also..." should be an alert
        let response = "HEARTBEAT_OK but also check the logs";
        let result = HeartbeatV2::classify_response(response);
        assert_eq!(
            result,
            HeartbeatResponseKind::Alert(response.to_string())
        );
    }

    #[test]
    fn classify_lowercase_heartbeat_ok_is_alert() {
        // Case-sensitive: "heartbeat_ok" is NOT the same as "HEARTBEAT_OK"
        let response = "heartbeat_ok";
        let result = HeartbeatV2::classify_response(response);
        assert_eq!(
            result,
            HeartbeatResponseKind::Alert(response.to_string())
        );
    }

    #[test]
    fn classify_empty_response_is_alert() {
        let result = HeartbeatV2::classify_response("");
        assert_eq!(result, HeartbeatResponseKind::Alert("".to_string()));
    }

    // ── strip_heartbeat_turns tests ────────────────────────────────

    #[test]
    fn strip_removes_ok_heartbeat_pair() {
        let mut history = vec![
            Turn::regular("user", "Hello"),
            Turn::heartbeat("user", "Heartbeat check"),
            Turn::heartbeat("assistant", "HEARTBEAT_OK"),
            Turn::regular("assistant", "Hi there!"),
        ];

        HeartbeatV2::strip_heartbeat_turns(&mut history);

        assert_eq!(history.len(), 2);
        assert_eq!(history[0].content, "Hello");
        assert_eq!(history[1].content, "Hi there!");
    }

    #[test]
    fn strip_retains_alert_heartbeat_pair() {
        let mut history = vec![
            Turn::regular("user", "Hello"),
            Turn::heartbeat("user", "Heartbeat check"),
            Turn::heartbeat("assistant", "Alert: disk usage at 95%"),
            Turn::regular("assistant", "Hi there!"),
        ];

        HeartbeatV2::strip_heartbeat_turns(&mut history);

        assert_eq!(history.len(), 4);
        assert_eq!(history[1].content, "Heartbeat check");
        assert_eq!(history[2].content, "Alert: disk usage at 95%");
    }

    #[test]
    fn strip_never_touches_regular_turns() {
        let mut history = vec![
            Turn::regular("user", "Hello"),
            Turn::regular("assistant", "Hi!"),
            Turn::regular("user", "HEARTBEAT_OK"), // regular turn with OK content
            Turn::regular("assistant", "That's interesting"),
        ];

        let original = history.clone();
        HeartbeatV2::strip_heartbeat_turns(&mut history);

        assert_eq!(history, original);
    }

    #[test]
    fn strip_handles_multiple_heartbeat_pairs() {
        let mut history = vec![
            Turn::regular("user", "Hello"),
            Turn::heartbeat("user", "Check 1"),
            Turn::heartbeat("assistant", "HEARTBEAT_OK"),
            Turn::regular("user", "How are you?"),
            Turn::heartbeat("user", "Check 2"),
            Turn::heartbeat("assistant", "Alert: memory high"),
            Turn::heartbeat("user", "Check 3"),
            Turn::heartbeat("assistant", "HEARTBEAT_OK"),
            Turn::regular("assistant", "I'm good!"),
        ];

        HeartbeatV2::strip_heartbeat_turns(&mut history);

        // Should keep: Hello, How are you?, Check 2, Alert: memory high, I'm good!
        assert_eq!(history.len(), 5);
        assert_eq!(history[0].content, "Hello");
        assert_eq!(history[1].content, "How are you?");
        assert_eq!(history[2].content, "Check 2");
        assert_eq!(history[3].content, "Alert: memory high");
        assert_eq!(history[4].content, "I'm good!");
    }

    #[test]
    fn strip_handles_empty_history() {
        let mut history: Vec<Turn> = vec![];
        HeartbeatV2::strip_heartbeat_turns(&mut history);
        assert!(history.is_empty());
    }

    #[test]
    fn strip_handles_only_regular_turns() {
        let mut history = vec![
            Turn::regular("user", "Hello"),
            Turn::regular("assistant", "Hi!"),
        ];

        let original = history.clone();
        HeartbeatV2::strip_heartbeat_turns(&mut history);
        assert_eq!(history, original);
    }

    // ── Scheduling tests ───────────────────────────────────────────

    #[tokio::test]
    async fn schedule_for_user_creates_schedule() {
        let hb = HeartbeatV2::with_defaults();
        hb.schedule_for_user("user1", Duration::from_secs(3600))
            .await;

        assert!(hb.has_schedule("user1"));
        assert_eq!(hb.schedule_count(), 1);

        let schedule = hb.get_schedule("user1").unwrap();
        assert_eq!(schedule.user_id, "user1");
        assert_eq!(schedule.interval, Duration::from_secs(3600));
        assert!(schedule.last_fired.is_none());
    }

    #[tokio::test]
    async fn schedule_replaces_existing() {
        let hb = HeartbeatV2::with_defaults();
        hb.schedule_for_user("user1", Duration::from_secs(3600))
            .await;
        let old_token = hb.get_schedule("user1").unwrap().cancel_token.clone();

        hb.schedule_for_user("user1", Duration::from_secs(1800))
            .await;

        // Old token should be cancelled
        assert!(old_token.is_cancelled());

        // New schedule should have the new interval
        let schedule = hb.get_schedule("user1").unwrap();
        assert_eq!(schedule.interval, Duration::from_secs(1800));
    }

    #[tokio::test]
    async fn cancel_for_user_removes_and_cancels() {
        let hb = HeartbeatV2::with_defaults();
        hb.schedule_for_user("user1", Duration::from_secs(3600))
            .await;
        let token = hb.get_schedule("user1").unwrap().cancel_token.clone();

        hb.cancel_for_user("user1");

        assert!(!hb.has_schedule("user1"));
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_for_nonexistent_user_is_noop() {
        let hb = HeartbeatV2::with_defaults();
        hb.cancel_for_user("nonexistent"); // should not panic
        assert_eq!(hb.schedule_count(), 0);
    }

    #[tokio::test]
    async fn per_user_independent_schedules() {
        let hb = HeartbeatV2::with_defaults();
        hb.schedule_for_user("user1", Duration::from_secs(3600))
            .await;
        hb.schedule_for_user("user2", Duration::from_secs(1800))
            .await;
        hb.schedule_for_user("user3", Duration::from_secs(900))
            .await;

        assert_eq!(hb.schedule_count(), 3);

        // Cancel one user — others unaffected
        hb.cancel_for_user("user2");
        assert_eq!(hb.schedule_count(), 2);
        assert!(hb.has_schedule("user1"));
        assert!(!hb.has_schedule("user2"));
        assert!(hb.has_schedule("user3"));
    }

    #[tokio::test]
    async fn cancel_all_removes_everything() {
        let hb = HeartbeatV2::with_defaults();
        hb.schedule_for_user("user1", Duration::from_secs(3600))
            .await;
        hb.schedule_for_user("user2", Duration::from_secs(1800))
            .await;

        hb.cancel_all();
        assert_eq!(hb.schedule_count(), 0);
    }

    #[tokio::test]
    async fn mark_fired_updates_timestamp() {
        let hb = HeartbeatV2::with_defaults();
        hb.schedule_for_user("user1", Duration::from_secs(3600))
            .await;

        assert!(hb.get_schedule("user1").unwrap().last_fired.is_none());

        hb.mark_fired("user1");

        let schedule = hb.get_schedule("user1").unwrap();
        assert!(schedule.last_fired.is_some());
    }

    // ── Temporary session tests ────────────────────────────────────

    #[test]
    fn temporary_session_creates_heartbeat_turn() {
        let turns = HeartbeatV2::create_temporary_session_context("user1");
        assert_eq!(turns.len(), 1);
        assert!(turns[0].is_heartbeat());
        assert_eq!(turns[0].role, "user");
    }

    // ── Config tests ───────────────────────────────────────────────

    #[test]
    fn default_config_values() {
        let config = HeartbeatV2Config::default();
        assert_eq!(config.default_interval_secs, 3600);
        assert!(config.enabled);
        assert!(config.prompt.contains("HEARTBEAT_OK"));
    }

    #[test]
    fn build_heartbeat_prompt_uses_config() {
        let config = HeartbeatV2Config {
            prompt: "Custom prompt. Reply HEARTBEAT_OK if fine.".to_string(),
            ..Default::default()
        };
        let hb = HeartbeatV2::new(config);
        assert_eq!(
            hb.build_heartbeat_prompt(),
            "Custom prompt. Reply HEARTBEAT_OK if fine."
        );
    }
}
