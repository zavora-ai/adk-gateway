//! Bounded concurrent task queue with slot management.
//!
//! Provides a `TaskQueue` that manages concurrent coding agent task execution
//! with configurable limits. Tasks are enqueued as pending and dequeued into
//! active execution when semaphore slots become available.

use std::collections::VecDeque;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore};
use tracing;

use super::models::{TaskId, TaskRequest};

/// Default maximum number of concurrent coding agent tasks.
const DEFAULT_MAX_CONCURRENT: u32 = 3;

/// A task waiting in the pending queue for an execution slot.
#[derive(Debug)]
pub struct QueuedTask {
    /// Unique task identifier.
    pub task_id: TaskId,
    /// The agent this task is assigned to.
    pub agent_id: String,
    /// The task request details.
    pub request: TaskRequest,
    /// When the task was added to the queue.
    pub queued_at: DateTime<Utc>,
}

/// A task currently being executed (holding a semaphore permit).
#[derive(Debug)]
pub struct ActiveTask {
    /// Unique task identifier.
    pub task_id: TaskId,
    /// The agent executing this task.
    pub agent_id: String,
    /// The task request details.
    pub request: TaskRequest,
    /// When the task started executing.
    pub started_at: DateTime<Utc>,
    /// Holds the semaphore permit — released when the task completes or is cancelled.
    pub _permit: OwnedSemaphorePermit,
}

/// Manages concurrent task execution with configurable limits.
///
/// Tasks flow through two stages:
/// 1. **Pending** — waiting in a FIFO queue for an execution slot
/// 2. **Active** — holding a semaphore permit, currently executing
///
/// The `TaskExecutor` is the single consumer: it calls `try_dequeue()` to move
/// tasks from pending to active and then executes them. The queue itself does
/// NOT spawn a background processor — doing so would create a second consumer
/// that races with the executor and strands tasks in `active` without running
/// them.
pub struct TaskQueue {
    /// Pending tasks waiting for a slot.
    pending: Mutex<VecDeque<QueuedTask>>,
    /// Currently executing tasks.
    active: DashMap<TaskId, ActiveTask>,
    /// Maximum concurrent tasks.
    max_concurrent: u32,
    /// Semaphore for slot management.
    slots: Arc<Semaphore>,
    /// Notifier woken when new tasks are enqueued or slots freed.
    /// The `TaskExecutor` can await this to avoid busy-polling.
    notify: Arc<Notify>,
}

impl TaskQueue {
    /// Creates a new `TaskQueue` with the specified concurrency limit.
    ///
    /// # Arguments
    /// * `max_concurrent` - Maximum number of tasks that can execute simultaneously.
    ///   If `None`, defaults to 3.
    pub fn new(max_concurrent: Option<u32>) -> Arc<Self> {
        let max = max_concurrent.unwrap_or(DEFAULT_MAX_CONCURRENT);
        let queue = Arc::new(Self {
            pending: Mutex::new(VecDeque::new()),
            active: DashMap::new(),
            max_concurrent: max,
            slots: Arc::new(Semaphore::new(max as usize)),
            notify: Arc::new(Notify::new()),
        });

        // NOTE: We intentionally do NOT spawn a background processor here.
        // The TaskExecutor is the single consumer of try_dequeue() — it moves
        // tasks from pending to active AND executes them. A second consumer
        // (a queue-internal processor) would race with the executor, win the
        // dequeue, and leave tasks stranded in `active` holding a permit but
        // never executing. See GAP A in the autonomous-operation investigation.

        queue
    }

    /// Returns a clone of the notifier used to signal new work.
    ///
    /// The `TaskExecutor` can await this to be woken when a task is enqueued
    /// or a slot is freed, avoiding a busy-poll loop.
    pub fn notifier(&self) -> Arc<Notify> {
        self.notify.clone()
    }

    /// Enqueues a task for execution. Returns the generated `TaskId`.
    ///
    /// The task is added to the pending queue and will be moved to active
    /// execution when a slot becomes available.
    pub async fn enqueue(&self, agent_id: String, request: TaskRequest) -> TaskId {
        let task_id = uuid::Uuid::new_v4().to_string();
        let queued_task = QueuedTask {
            task_id: task_id.clone(),
            agent_id,
            request,
            queued_at: Utc::now(),
        };

        {
            let mut pending = self.pending.lock().await;
            pending.push_back(queued_task);
        }

        tracing::debug!(task_id = %task_id, "Task enqueued");
        // Notify the background processor that a new task is available
        self.notify.notify_one();

        task_id
    }

    /// Attempts to dequeue the next pending task into active execution.
    ///
    /// Acquires a semaphore permit and moves the task from pending to active.
    /// Returns `None` if no pending tasks are available or no slots are free.
    pub async fn try_dequeue(&self) -> Option<TaskId> {
        // Try to acquire a permit without blocking
        let permit = match Arc::clone(&self.slots).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return None,
        };

        // Pop the next pending task
        let queued_task = {
            let mut pending = self.pending.lock().await;
            pending.pop_front()
        };

        match queued_task {
            Some(task) => {
                let task_id = task.task_id.clone();
                let active_task = ActiveTask {
                    task_id: task.task_id,
                    agent_id: task.agent_id,
                    request: task.request,
                    started_at: Utc::now(),
                    _permit: permit,
                };
                self.active.insert(task_id.clone(), active_task);
                tracing::debug!(task_id = %task_id, "Task moved to active");
                Some(task_id)
            }
            None => {
                // No pending tasks — drop the permit back
                drop(permit);
                None
            }
        }
    }

    /// Marks a task as completed and releases its execution slot.
    ///
    /// Returns `true` if the task was found and removed, `false` otherwise.
    pub fn complete_task(&self, task_id: &str) -> bool {
        if let Some((_, _task)) = self.active.remove(task_id) {
            tracing::debug!(task_id = %task_id, "Task completed, slot released");
            // The permit is dropped here, releasing the semaphore slot
            // Notify the background processor that a slot is now available
            self.notify.notify_one();
            true
        } else {
            tracing::warn!(task_id = %task_id, "Attempted to complete unknown task");
            false
        }
    }

    /// Cancels a task, removing it from either the pending queue or active set.
    ///
    /// Returns `true` if the task was found and cancelled, `false` otherwise.
    pub async fn cancel_task(&self, task_id: &str) -> bool {
        // First check active tasks
        if let Some((_, _task)) = self.active.remove(task_id) {
            tracing::debug!(task_id = %task_id, "Active task cancelled, slot released");
            // The permit is dropped here, releasing the semaphore slot
            self.notify.notify_one();
            return true;
        }

        // Then check pending queue
        let mut pending = self.pending.lock().await;
        if let Some(pos) = pending.iter().position(|t| t.task_id == task_id) {
            pending.remove(pos);
            tracing::debug!(task_id = %task_id, "Pending task cancelled");
            return true;
        }

        tracing::warn!(task_id = %task_id, "Attempted to cancel unknown task");
        false
    }

    /// Returns the number of currently active (executing) tasks.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Returns the number of pending (queued) tasks.
    pub async fn pending_count(&self) -> usize {
        let pending = self.pending.lock().await;
        pending.len()
    }

    /// Returns the configured maximum concurrent task limit.
    pub fn max_concurrent(&self) -> u32 {
        self.max_concurrent
    }

    /// Returns a reference to the active tasks map for external inspection.
    pub fn active_tasks(&self) -> &DashMap<TaskId, ActiveTask> {
        &self.active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coding_agent::models::{ReplyTarget, TaskRequest, TaskTrigger};

    /// Helper to create a test TaskRequest.
    fn make_request(description: &str) -> TaskRequest {
        TaskRequest {
            description: description.to_string(),
            trigger: TaskTrigger::ControlPanel {
                user_id: "test-user".to_string(),
            },
            workspace: None,
            file_context: None,
            reply_to: ReplyTarget {
                channel_type: "telegram".to_string(),
                channel_id: "12345".to_string(),
                message_id: None,
            },
        }
    }

    #[tokio::test]
    async fn test_new_creates_queue_with_default_concurrency() {
        let queue = TaskQueue::new(None);
        assert_eq!(queue.max_concurrent(), DEFAULT_MAX_CONCURRENT);
        assert_eq!(queue.active_count(), 0);
        assert_eq!(queue.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_new_creates_queue_with_custom_concurrency() {
        let queue = TaskQueue::new(Some(5));
        assert_eq!(queue.max_concurrent(), 5);
    }

    #[tokio::test]
    async fn test_enqueue_adds_task_to_pending() {
        let queue = TaskQueue::new(Some(3));
        let task_id = queue
            .enqueue("agent-1".to_string(), make_request("fix bug"))
            .await;

        assert!(!task_id.is_empty());

        // The queue no longer auto-dequeues — the task stays pending until the
        // TaskExecutor calls try_dequeue(). This is the single-consumer model.
        assert_eq!(queue.pending_count().await, 1);
        assert_eq!(queue.active_count(), 0);
    }

    #[tokio::test]
    async fn test_enqueue_returns_unique_task_ids() {
        let queue = TaskQueue::new(Some(10));
        let id1 = queue
            .enqueue("agent-1".to_string(), make_request("task 1"))
            .await;
        let id2 = queue
            .enqueue("agent-1".to_string(), make_request("task 2"))
            .await;
        let id3 = queue
            .enqueue("agent-1".to_string(), make_request("task 3"))
            .await;

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[tokio::test]
    async fn test_try_dequeue_moves_task_to_active() {
        let queue = TaskQueue::new(Some(3));

        // Manually add a task to pending without triggering background processor
        {
            let mut pending = queue.pending.lock().await;
            pending.push_back(QueuedTask {
                task_id: "test-task-1".to_string(),
                agent_id: "agent-1".to_string(),
                request: make_request("test task"),
                queued_at: Utc::now(),
            });
        }

        let dequeued = queue.try_dequeue().await;
        assert_eq!(dequeued, Some("test-task-1".to_string()));
        assert_eq!(queue.active_count(), 1);
    }

    #[tokio::test]
    async fn test_try_dequeue_returns_none_when_empty() {
        let queue = TaskQueue::new(Some(3));
        let result = queue.try_dequeue().await;
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_try_dequeue_returns_none_when_no_slots() {
        let queue = TaskQueue::new(Some(1));

        // Fill the single slot
        {
            let mut pending = queue.pending.lock().await;
            pending.push_back(QueuedTask {
                task_id: "task-1".to_string(),
                agent_id: "agent-1".to_string(),
                request: make_request("task 1"),
                queued_at: Utc::now(),
            });
        }
        queue.try_dequeue().await; // fills the slot

        // Add another task
        {
            let mut pending = queue.pending.lock().await;
            pending.push_back(QueuedTask {
                task_id: "task-2".to_string(),
                agent_id: "agent-1".to_string(),
                request: make_request("task 2"),
                queued_at: Utc::now(),
            });
        }

        // Should fail — no slots available
        let result = queue.try_dequeue().await;
        assert_eq!(result, None);
        assert_eq!(queue.active_count(), 1);
    }

    #[tokio::test]
    async fn test_complete_task_releases_slot() {
        let queue = TaskQueue::new(Some(1));

        // Add and dequeue a task
        {
            let mut pending = queue.pending.lock().await;
            pending.push_back(QueuedTask {
                task_id: "task-1".to_string(),
                agent_id: "agent-1".to_string(),
                request: make_request("task 1"),
                queued_at: Utc::now(),
            });
        }
        queue.try_dequeue().await;
        assert_eq!(queue.active_count(), 1);

        // Complete the task
        let completed = queue.complete_task("task-1");
        assert!(completed);
        assert_eq!(queue.active_count(), 0);

        // Now another task can be dequeued
        {
            let mut pending = queue.pending.lock().await;
            pending.push_back(QueuedTask {
                task_id: "task-2".to_string(),
                agent_id: "agent-1".to_string(),
                request: make_request("task 2"),
                queued_at: Utc::now(),
            });
        }
        let result = queue.try_dequeue().await;
        assert_eq!(result, Some("task-2".to_string()));
    }

    #[tokio::test]
    async fn test_complete_task_returns_false_for_unknown() {
        let queue = TaskQueue::new(Some(3));
        let result = queue.complete_task("nonexistent");
        assert!(!result);
    }

    #[tokio::test]
    async fn test_cancel_active_task() {
        let queue = TaskQueue::new(Some(3));

        {
            let mut pending = queue.pending.lock().await;
            pending.push_back(QueuedTask {
                task_id: "task-1".to_string(),
                agent_id: "agent-1".to_string(),
                request: make_request("task 1"),
                queued_at: Utc::now(),
            });
        }
        queue.try_dequeue().await;
        assert_eq!(queue.active_count(), 1);

        let cancelled = queue.cancel_task("task-1").await;
        assert!(cancelled);
        assert_eq!(queue.active_count(), 0);
    }

    #[tokio::test]
    async fn test_cancel_pending_task() {
        let queue = TaskQueue::new(Some(0)); // No slots — tasks stay pending

        {
            let mut pending = queue.pending.lock().await;
            pending.push_back(QueuedTask {
                task_id: "task-1".to_string(),
                agent_id: "agent-1".to_string(),
                request: make_request("task 1"),
                queued_at: Utc::now(),
            });
        }
        assert_eq!(queue.pending_count().await, 1);

        let cancelled = queue.cancel_task("task-1").await;
        assert!(cancelled);
        assert_eq!(queue.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_cancel_unknown_task_returns_false() {
        let queue = TaskQueue::new(Some(3));
        let result = queue.cancel_task("nonexistent").await;
        assert!(!result);
    }

    #[tokio::test]
    async fn test_concurrency_limit_enforced() {
        let queue = TaskQueue::new(Some(2));

        // Add 3 tasks manually
        {
            let mut pending = queue.pending.lock().await;
            for i in 0..3 {
                pending.push_back(QueuedTask {
                    task_id: format!("task-{}", i),
                    agent_id: "agent-1".to_string(),
                    request: make_request(&format!("task {}", i)),
                    queued_at: Utc::now(),
                });
            }
        }

        // Dequeue should succeed twice
        assert!(queue.try_dequeue().await.is_some());
        assert!(queue.try_dequeue().await.is_some());
        // Third should fail — no slots
        assert!(queue.try_dequeue().await.is_none());

        assert_eq!(queue.active_count(), 2);
        assert_eq!(queue.pending_count().await, 1);
    }

    #[tokio::test]
    async fn test_executor_is_single_consumer_via_try_dequeue() {
        // The queue no longer auto-dequeues; the TaskExecutor drives try_dequeue.
        // Verify enqueue leaves the task pending until try_dequeue is called.
        let queue = TaskQueue::new(Some(3));

        let task_id = queue
            .enqueue("agent-1".to_string(), make_request("manual-dequeue test"))
            .await;

        // Without a consumer calling try_dequeue, the task stays pending.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        assert_eq!(queue.active_count(), 0);
        assert_eq!(queue.pending_count().await, 1);

        // The executor's dequeue moves it to active.
        let dequeued = queue.try_dequeue().await;
        assert_eq!(dequeued, Some(task_id.clone()));
        assert_eq!(queue.active_count(), 1);
        assert_eq!(queue.pending_count().await, 0);
        assert!(queue.active_tasks().contains_key(&task_id));
    }

    #[tokio::test]
    async fn test_slot_frees_on_completion_allows_next_dequeue() {
        let queue = TaskQueue::new(Some(1));

        // Fill the single slot via try_dequeue (executor behavior)
        let task_id_1 = queue
            .enqueue("agent-1".to_string(), make_request("task 1"))
            .await;
        assert_eq!(queue.try_dequeue().await, Some(task_id_1.clone()));
        assert_eq!(queue.active_count(), 1);

        // Enqueue another — no free slot, so try_dequeue returns None
        let _task_id_2 = queue
            .enqueue("agent-1".to_string(), make_request("task 2"))
            .await;
        assert!(queue.try_dequeue().await.is_none());
        assert_eq!(queue.active_count(), 1);
        assert_eq!(queue.pending_count().await, 1);

        // Complete the first task — slot frees, next dequeue succeeds
        queue.complete_task(&task_id_1);
        assert!(queue.try_dequeue().await.is_some());
        assert_eq!(queue.active_count(), 1);
        assert_eq!(queue.pending_count().await, 0);
    }

    #[tokio::test]
    async fn test_fifo_ordering() {
        let queue = TaskQueue::new(Some(10));

        // Add tasks in order
        {
            let mut pending = queue.pending.lock().await;
            for i in 0..5 {
                pending.push_back(QueuedTask {
                    task_id: format!("task-{}", i),
                    agent_id: "agent-1".to_string(),
                    request: make_request(&format!("task {}", i)),
                    queued_at: Utc::now(),
                });
            }
        }

        // Dequeue should return in FIFO order
        assert_eq!(queue.try_dequeue().await, Some("task-0".to_string()));
        assert_eq!(queue.try_dequeue().await, Some("task-1".to_string()));
        assert_eq!(queue.try_dequeue().await, Some("task-2".to_string()));
        assert_eq!(queue.try_dequeue().await, Some("task-3".to_string()));
        assert_eq!(queue.try_dequeue().await, Some("task-4".to_string()));
    }

    #[tokio::test]
    async fn test_active_task_holds_correct_data() {
        let queue = TaskQueue::new(Some(3));

        {
            let mut pending = queue.pending.lock().await;
            pending.push_back(QueuedTask {
                task_id: "task-abc".to_string(),
                agent_id: "claude-code-1".to_string(),
                request: make_request("fix authentication bug"),
                queued_at: Utc::now(),
            });
        }

        queue.try_dequeue().await;

        let active = queue.active_tasks().get("task-abc").unwrap();
        assert_eq!(active.task_id, "task-abc");
        assert_eq!(active.agent_id, "claude-code-1");
        assert_eq!(active.request.description, "fix authentication bug");
        assert!(active.started_at <= Utc::now());
    }
}
