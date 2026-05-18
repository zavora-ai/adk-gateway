//! Progress reporting for long-running coding agent tasks.
//!
//! The `ProgressReporter` sends periodic status updates to users during task
//! execution, integrating with the existing `DeliveryStrategy` system. It handles:
//!
//! - Periodic progress updates every 30s (configurable) with elapsed time
//! - 80% timeout warnings when a task approaches its time limit (R6.5)
//! - Completion messages with duration and modified file count (R6.2)
//! - Failure messages with error category and elapsed time (R6.3)

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::coding_agent::models::{TaskError, TaskId, TaskResult};
use crate::delivery::{DeliveryStrategy, MessageRef};

/// Sends progress updates to users during task execution.
/// Integrates with the existing `DeliveryStrategy` system.
pub struct ProgressReporter {
    delivery: Arc<dyn DeliveryStrategy>,
    /// Interval between progress updates (default: 30s).
    interval: Duration,
}

impl ProgressReporter {
    /// Create a new progress reporter with the given delivery strategy and default 30s interval.
    pub fn new(delivery: Arc<dyn DeliveryStrategy>) -> Self {
        Self {
            delivery,
            interval: Duration::from_secs(30),
        }
    }

    /// Create a new progress reporter with a custom interval.
    pub fn with_interval(delivery: Arc<dyn DeliveryStrategy>, interval: Duration) -> Self {
        Self { delivery, interval }
    }

    /// Start reporting progress for a task.
    ///
    /// Spawns a background tokio task that sends periodic updates via the
    /// delivery strategy. Returns a `ProgressHandle` that can be used to
    /// stop reporting and send completion/failure messages.
    pub fn start(
        &self,
        task_id: TaskId,
        msg_ref: MessageRef,
        timeout_secs: u64,
    ) -> ProgressHandle {
        let delivery = self.delivery.clone();
        let interval = self.interval;
        let (stop_tx, stop_rx) = watch::channel(false);
        let start_time = Instant::now();

        let handle = tokio::spawn(progress_loop(
            delivery.clone(),
            task_id.clone(),
            msg_ref.clone(),
            timeout_secs,
            interval,
            stop_rx,
            start_time,
        ));

        ProgressHandle {
            delivery,
            task_id,
            msg_ref,
            start_time,
            stop_tx,
            join_handle: Some(handle),
        }
    }
}

/// Background loop that sends periodic progress updates.
async fn progress_loop(
    delivery: Arc<dyn DeliveryStrategy>,
    task_id: TaskId,
    msg_ref: MessageRef,
    timeout_secs: u64,
    interval: Duration,
    mut stop_rx: watch::Receiver<bool>,
    start_time: Instant,
) {
    let mut tick_interval = tokio::time::interval(interval);
    // Skip the first immediate tick
    tick_interval.tick().await;

    let mut warning_sent = false;
    let timeout_threshold = Duration::from_secs_f64(timeout_secs as f64 * 0.8);

    loop {
        tokio::select! {
            _ = tick_interval.tick() => {
                let elapsed = start_time.elapsed();
                let elapsed_secs = elapsed.as_secs();

                // Check if we should send the 80% timeout warning
                if !warning_sent && elapsed >= timeout_threshold {
                    let warning_msg = format_timeout_warning(&task_id, elapsed_secs, timeout_secs);
                    let _ = delivery.on_complete(&warning_msg, &msg_ref).await;
                    warning_sent = true;
                }

                // Send regular progress update
                let progress_msg = format_progress_update(&task_id, elapsed_secs, None);
                let _ = delivery.on_complete(&progress_msg, &msg_ref).await;
            }
            _ = stop_rx.changed() => {
                // Stop signal received
                break;
            }
        }
    }
}

/// Handle to a running progress reporter. Use `stop()` to abort the
/// background task, then call `send_completion` or `send_failure` to
/// deliver the final status message.
pub struct ProgressHandle {
    delivery: Arc<dyn DeliveryStrategy>,
    task_id: TaskId,
    msg_ref: MessageRef,
    start_time: Instant,
    stop_tx: watch::Sender<bool>,
    join_handle: Option<JoinHandle<()>>,
}

impl ProgressHandle {
    /// Stop the background progress reporting task.
    pub fn stop(&mut self) {
        // Signal the loop to stop
        let _ = self.stop_tx.send(true);
        // Abort the task if it hasn't stopped yet
        if let Some(handle) = self.join_handle.take() {
            handle.abort();
        }
    }

    /// Get the elapsed time since the task started.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Send a completion message with duration and modified file count.
    ///
    /// Requirement 6.2: completion message includes duration and number of files modified.
    pub async fn send_completion(&mut self, result: &TaskResult) -> anyhow::Result<()> {
        self.stop();
        let duration_secs = result.duration_ms / 1000;
        let file_count = result.modified_files.len();
        let msg = format_completion_message(&self.task_id, duration_secs, file_count);
        self.delivery.on_complete(&msg, &self.msg_ref).await
    }

    /// Send a failure message with error category and elapsed time.
    ///
    /// Requirement 6.3: failure notification includes error category and elapsed time.
    pub async fn send_failure(&mut self, error: &TaskError) -> anyhow::Result<()> {
        self.stop();
        let elapsed_secs = self.start_time.elapsed().as_secs();
        let msg = format_failure_message(&self.task_id, error, elapsed_secs);
        self.delivery.on_complete(&msg, &self.msg_ref).await
    }
}

impl Drop for ProgressHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

// --- Message formatting helpers ---

/// Format a periodic progress update message.
///
/// Requirement 6.4: progress updates include elapsed time and estimated
/// completion percentage when available.
pub fn format_progress_update(
    task_id: &str,
    elapsed_secs: u64,
    completion_percent: Option<u8>,
) -> String {
    let elapsed_str = format_duration(elapsed_secs);
    match completion_percent {
        Some(pct) => format!(
            "⏳ Task `{}` in progress — {} elapsed ({}% complete)",
            task_id, elapsed_str, pct
        ),
        None => format!(
            "⏳ Task `{}` in progress — {} elapsed",
            task_id, elapsed_str
        ),
    }
}

/// Format the 80% timeout warning message.
///
/// Requirement 6.5: warning when task exceeds 80% of configured timeout.
pub fn format_timeout_warning(task_id: &str, elapsed_secs: u64, timeout_secs: u64) -> String {
    let remaining_secs = timeout_secs.saturating_sub(elapsed_secs);
    let remaining_str = format_duration(remaining_secs);
    format!(
        "⚠️ Task `{}` is approaching its time limit — {} remaining before timeout ({}s limit)",
        task_id, remaining_str, timeout_secs
    )
}

/// Format a task completion message.
///
/// Requirement 6.2: completion message includes duration and number of files modified.
pub fn format_completion_message(task_id: &str, duration_secs: u64, file_count: usize) -> String {
    let duration_str = format_duration(duration_secs);
    match file_count {
        0 => format!("✅ Task `{}` completed in {} — no files modified", task_id, duration_str),
        1 => format!("✅ Task `{}` completed in {} — 1 file modified", task_id, duration_str),
        n => format!(
            "✅ Task `{}` completed in {} — {} files modified",
            task_id, duration_str, n
        ),
    }
}

/// Format a task failure message.
///
/// Requirement 6.3: failure notification includes error category and elapsed time.
pub fn format_failure_message(task_id: &str, error: &TaskError, elapsed_secs: u64) -> String {
    let elapsed_str = format_duration(elapsed_secs);
    let category = error_category_name(error);
    format!(
        "❌ Task `{}` failed after {} — {}",
        task_id, elapsed_str, category
    )
}

/// Get a human-readable error category name from a TaskError.
fn error_category_name(error: &TaskError) -> &'static str {
    match error {
        TaskError::Timeout { .. } => "timeout",
        TaskError::CostCap { .. } => "cost cap exceeded",
        TaskError::RateLimit { .. } => "rate limit",
        TaskError::ExecutionError { .. } => "execution error",
        TaskError::AgentDisconnected { .. } => "agent disconnected",
        TaskError::WorkspaceViolation { .. } => "workspace violation",
    }
}

/// Format seconds into a human-readable duration string (e.g., "2m 30s", "1h 5m").
fn format_duration(secs: u64) -> String {
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        let mins = secs / 60;
        let remaining = secs % 60;
        if remaining == 0 {
            format!("{}m", mins)
        } else {
            format!("{}m {}s", mins, remaining)
        }
    } else {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        if mins == 0 {
            format!("{}h", hours)
        } else {
            format!("{}h {}m", hours, mins)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::ChannelType;
    use crate::coding_agent::models::{FileChange, FileChangeType};
    use async_trait::async_trait;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// A mock delivery strategy that records all messages sent.
    struct MockDelivery {
        messages: Mutex<Vec<String>>,
    }

    impl MockDelivery {
        fn new() -> Self {
            Self {
                messages: Mutex::new(Vec::new()),
            }
        }

        fn messages(&self) -> Vec<String> {
            self.messages.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DeliveryStrategy for MockDelivery {
        async fn on_partial(&self, _text: &str, _msg_ref: &MessageRef) -> anyhow::Result<()> {
            Ok(())
        }

        async fn on_complete(&self, text: &str, _msg_ref: &MessageRef) -> anyhow::Result<()> {
            self.messages.lock().unwrap().push(text.to_string());
            Ok(())
        }
    }

    fn test_msg_ref() -> MessageRef {
        MessageRef {
            channel_type: ChannelType::Telegram,
            account_id: "default".to_string(),
            recipient_id: "user123".to_string(),
            message_id: Some("msg_1".to_string()),
            reply_to: Some("orig_msg".to_string()),
        }
    }

    // --- Unit tests for formatting helpers ---

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(59), "59s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(60), "1m");
        assert_eq!(format_duration(90), "1m 30s");
        assert_eq!(format_duration(120), "2m");
        assert_eq!(format_duration(3599), "59m 59s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(3600), "1h");
        assert_eq!(format_duration(3660), "1h 1m");
        assert_eq!(format_duration(7200), "2h");
    }

    #[test]
    fn test_format_progress_update_without_percentage() {
        let msg = format_progress_update("task-123", 45, None);
        assert!(msg.contains("task-123"));
        assert!(msg.contains("45s"));
        assert!(msg.contains("in progress"));
        assert!(!msg.contains("%"));
    }

    #[test]
    fn test_format_progress_update_with_percentage() {
        let msg = format_progress_update("task-456", 120, Some(65));
        assert!(msg.contains("task-456"));
        assert!(msg.contains("2m"));
        assert!(msg.contains("65%"));
    }

    #[test]
    fn test_format_timeout_warning() {
        let msg = format_timeout_warning("task-789", 1440, 1800);
        assert!(msg.contains("task-789"));
        assert!(msg.contains("approaching"));
        assert!(msg.contains("time limit"));
        assert!(msg.contains("1800s"));
        // 1800 - 1440 = 360s = 6m remaining
        assert!(msg.contains("6m"));
    }

    #[test]
    fn test_format_completion_message_no_files() {
        let msg = format_completion_message("task-1", 90, 0);
        assert!(msg.contains("task-1"));
        assert!(msg.contains("1m 30s"));
        assert!(msg.contains("no files modified"));
    }

    #[test]
    fn test_format_completion_message_one_file() {
        let msg = format_completion_message("task-2", 60, 1);
        assert!(msg.contains("task-2"));
        assert!(msg.contains("1m"));
        assert!(msg.contains("1 file modified"));
    }

    #[test]
    fn test_format_completion_message_multiple_files() {
        let msg = format_completion_message("task-3", 300, 7);
        assert!(msg.contains("task-3"));
        assert!(msg.contains("5m"));
        assert!(msg.contains("7 files modified"));
    }

    #[test]
    fn test_format_failure_message_timeout() {
        let error = TaskError::Timeout {
            elapsed_secs: 1800,
            limit_secs: 1800,
        };
        let msg = format_failure_message("task-4", &error, 1800);
        assert!(msg.contains("task-4"));
        assert!(msg.contains("30m"));
        assert!(msg.contains("timeout"));
    }

    #[test]
    fn test_format_failure_message_cost_cap() {
        let error = TaskError::CostCap {
            spent_usd: 5.50,
            cap_usd: 5.00,
        };
        let msg = format_failure_message("task-5", &error, 600);
        assert!(msg.contains("task-5"));
        assert!(msg.contains("10m"));
        assert!(msg.contains("cost cap exceeded"));
    }

    #[test]
    fn test_format_failure_message_rate_limit() {
        let error = TaskError::RateLimit {
            retry_after_secs: Some(60),
        };
        let msg = format_failure_message("task-6", &error, 120);
        assert!(msg.contains("task-6"));
        assert!(msg.contains("2m"));
        assert!(msg.contains("rate limit"));
    }

    #[test]
    fn test_format_failure_message_execution_error() {
        let error = TaskError::ExecutionError {
            message: "segfault".to_string(),
            partial_output: None,
        };
        let msg = format_failure_message("task-7", &error, 45);
        assert!(msg.contains("task-7"));
        assert!(msg.contains("45s"));
        assert!(msg.contains("execution error"));
    }

    #[test]
    fn test_format_failure_message_agent_disconnected() {
        let error = TaskError::AgentDisconnected {
            agent_id: "claude-1".to_string(),
        };
        let msg = format_failure_message("task-8", &error, 200);
        assert!(msg.contains("task-8"));
        assert!(msg.contains("agent disconnected"));
    }

    #[test]
    fn test_format_failure_message_workspace_violation() {
        let error = TaskError::WorkspaceViolation {
            attempted_path: PathBuf::from("/etc/passwd"),
            allowed_workspaces: vec![PathBuf::from("/home/user/project")],
        };
        let msg = format_failure_message("task-9", &error, 10);
        assert!(msg.contains("task-9"));
        assert!(msg.contains("workspace violation"));
    }

    // --- Async tests for ProgressReporter and ProgressHandle ---

    #[tokio::test]
    async fn test_progress_reporter_sends_updates() {
        let delivery = Arc::new(MockDelivery::new());
        let reporter = ProgressReporter::with_interval(delivery.clone(), Duration::from_millis(50));
        let msg_ref = test_msg_ref();

        let mut handle = reporter.start("test-task".to_string(), msg_ref, 1800);

        // Wait for a couple of ticks
        tokio::time::sleep(Duration::from_millis(130)).await;

        handle.stop();

        let messages = delivery.messages();
        // Should have received at least 1 progress update
        assert!(!messages.is_empty(), "Expected at least one progress message");
        assert!(messages[0].contains("test-task"));
        assert!(messages[0].contains("in progress"));
    }

    #[tokio::test]
    async fn test_progress_handle_stop_aborts_task() {
        let delivery = Arc::new(MockDelivery::new());
        let reporter = ProgressReporter::with_interval(delivery.clone(), Duration::from_millis(50));
        let msg_ref = test_msg_ref();

        let mut handle = reporter.start("stop-test".to_string(), msg_ref, 1800);

        // Stop immediately
        handle.stop();

        // Wait to ensure no more messages are sent
        tokio::time::sleep(Duration::from_millis(150)).await;

        let messages = delivery.messages();
        // Should have 0 or at most 1 message (race condition with first tick)
        assert!(messages.len() <= 1);
    }

    #[tokio::test]
    async fn test_progress_reporter_timeout_warning() {
        let delivery = Arc::new(MockDelivery::new());
        // Use a very short interval and timeout to test the 80% warning
        let reporter = ProgressReporter::with_interval(delivery.clone(), Duration::from_millis(20));
        let msg_ref = test_msg_ref();

        // timeout_secs = 1 means 80% threshold at 0.8s
        // But we use millis-scale intervals for testing, so set timeout very low
        // Actually, the elapsed time is real wall-clock time, so we need to wait
        // Let's use a timeout of 0 to trigger warning immediately on first tick
        let mut handle = reporter.start("warn-test".to_string(), msg_ref, 0);

        // Wait for at least one tick
        tokio::time::sleep(Duration::from_millis(50)).await;

        handle.stop();

        let messages = delivery.messages();
        // With timeout=0, the 80% threshold is 0s, so warning should fire on first tick
        let has_warning = messages.iter().any(|m| m.contains("approaching"));
        assert!(has_warning, "Expected timeout warning message, got: {:?}", messages);
    }

    #[tokio::test]
    async fn test_progress_handle_send_completion() {
        let delivery = Arc::new(MockDelivery::new());
        let reporter = ProgressReporter::with_interval(delivery.clone(), Duration::from_secs(30));
        let msg_ref = test_msg_ref();

        let mut handle = reporter.start("complete-test".to_string(), msg_ref, 1800);

        let result = TaskResult {
            output: "Done".to_string(),
            modified_files: vec![
                FileChange {
                    path: PathBuf::from("src/main.rs"),
                    change_type: FileChangeType::Modified,
                    lines_added: 10,
                    lines_removed: 5,
                },
                FileChange {
                    path: PathBuf::from("src/lib.rs"),
                    change_type: FileChangeType::Added,
                    lines_added: 20,
                    lines_removed: 0,
                },
            ],
            duration_ms: 45000,
            token_usage: None,
        };

        handle.send_completion(&result).await.unwrap();

        let messages = delivery.messages();
        let completion_msg = messages.last().unwrap();
        assert!(completion_msg.contains("complete-test"));
        assert!(completion_msg.contains("45s"));
        assert!(completion_msg.contains("2 files modified"));
    }

    #[tokio::test]
    async fn test_progress_handle_send_failure() {
        let delivery = Arc::new(MockDelivery::new());
        let reporter = ProgressReporter::with_interval(delivery.clone(), Duration::from_secs(30));
        let msg_ref = test_msg_ref();

        let mut handle = reporter.start("fail-test".to_string(), msg_ref, 1800);

        let error = TaskError::Timeout {
            elapsed_secs: 1800,
            limit_secs: 1800,
        };

        handle.send_failure(&error).await.unwrap();

        let messages = delivery.messages();
        let failure_msg = messages.last().unwrap();
        assert!(failure_msg.contains("fail-test"));
        assert!(failure_msg.contains("timeout"));
    }

    #[tokio::test]
    async fn test_progress_handle_elapsed() {
        let delivery = Arc::new(MockDelivery::new());
        let reporter = ProgressReporter::with_interval(delivery.clone(), Duration::from_secs(30));
        let msg_ref = test_msg_ref();

        let handle = reporter.start("elapsed-test".to_string(), msg_ref, 1800);

        tokio::time::sleep(Duration::from_millis(50)).await;

        let elapsed = handle.elapsed();
        assert!(elapsed >= Duration::from_millis(50));
    }

    #[tokio::test]
    async fn test_progress_reporter_new_default_interval() {
        let delivery = Arc::new(MockDelivery::new());
        let reporter = ProgressReporter::new(delivery);
        assert_eq!(reporter.interval, Duration::from_secs(30));
    }
}
