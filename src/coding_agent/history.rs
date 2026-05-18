//! Task history storage and retrieval for audit trail.
//!
//! Provides in-memory, thread-safe storage of task execution history
//! per coding agent. Each agent's history is capped at 200 entries,
//! with a default display limit of 50 entries.

use dashmap::DashMap;

use super::models::{TaskHistoryEntry, TaskId};

/// Maximum number of history entries stored per agent.
const MAX_ENTRIES_PER_AGENT: usize = 200;

/// Default number of entries returned when no limit is specified.
pub const DEFAULT_DISPLAY_LIMIT: usize = 50;

/// Thread-safe in-memory storage for task history entries.
///
/// Stores entries per agent using a `DashMap` for concurrent access.
/// Each agent's history is capped at [`MAX_ENTRIES_PER_AGENT`] entries;
/// when the cap is exceeded, the oldest entries are removed.
pub struct TaskHistory {
    /// Per-agent history storage, keyed by agent_id.
    entries: DashMap<String, Vec<TaskHistoryEntry>>,
}

impl TaskHistory {
    /// Creates a new empty `TaskHistory`.
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
        }
    }

    /// Records a new task history entry.
    ///
    /// The entry is appended to the agent's history list. If the list
    /// exceeds [`MAX_ENTRIES_PER_AGENT`], the oldest entries are removed
    /// to maintain the cap.
    pub fn record(&self, entry: TaskHistoryEntry) {
        let agent_id = entry.agent_id.clone();
        let mut agent_entries = self.entries.entry(agent_id).or_default();

        agent_entries.push(entry);

        // Trim oldest entries if we exceed the cap
        if agent_entries.len() > MAX_ENTRIES_PER_AGENT {
            let excess = agent_entries.len() - MAX_ENTRIES_PER_AGENT;
            agent_entries.drain(0..excess);
        }
    }

    /// Returns the most recent N entries for a given agent, sorted by
    /// `created_at` descending (most recent first).
    ///
    /// If `limit` exceeds the number of stored entries, all entries are returned.
    /// The default display limit is [`DEFAULT_DISPLAY_LIMIT`] (50 entries).
    pub fn get_recent(&self, agent_id: &str, limit: usize) -> Vec<TaskHistoryEntry> {
        let Some(agent_entries) = self.entries.get(agent_id) else {
            return Vec::new();
        };

        let entries = agent_entries.value();
        let count = limit.min(entries.len());

        // Entries are stored in insertion order (oldest first).
        // Take the last `count` entries and reverse for descending order.
        let mut recent: Vec<TaskHistoryEntry> = entries
            .iter()
            .rev()
            .take(count)
            .cloned()
            .collect();

        // Sort by created_at descending to ensure correct ordering
        // even if entries were not inserted in strict chronological order.
        recent.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        recent
    }

    /// Returns a specific task by its task_id, searching across all agents.
    ///
    /// Returns `None` if no task with the given ID is found.
    pub fn get_task(&self, task_id: &TaskId) -> Option<TaskHistoryEntry> {
        for entry in self.entries.iter() {
            if let Some(task) = entry.value().iter().find(|e| e.task_id == *task_id) {
                return Some(task.clone());
            }
        }
        None
    }
}

impl Default for TaskHistory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;

    use crate::coding_agent::models::{TaskState, TaskTrigger};

    /// Helper to create a test TaskHistoryEntry with a given agent_id and task_id.
    fn make_entry(agent_id: &str, task_id: &str, offset_secs: i64) -> TaskHistoryEntry {
        TaskHistoryEntry {
            task_id: task_id.to_string(),
            agent_id: agent_id.to_string(),
            description: format!("Task {task_id}"),
            trigger: TaskTrigger::ControlPanel {
                user_id: "user-1".to_string(),
            },
            state: TaskState::Queued {
                queued_at: Utc::now(),
            },
            workspace: PathBuf::from("/workspace"),
            created_at: Utc::now() + chrono::Duration::seconds(offset_secs),
        }
    }

    #[test]
    fn test_new_history_is_empty() {
        let history = TaskHistory::new();
        let recent = history.get_recent("agent-1", DEFAULT_DISPLAY_LIMIT);
        assert!(recent.is_empty());
    }

    #[test]
    fn test_record_and_retrieve() {
        let history = TaskHistory::new();
        let entry = make_entry("agent-1", "task-1", 0);
        history.record(entry.clone());

        let recent = history.get_recent("agent-1", DEFAULT_DISPLAY_LIMIT);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].task_id, "task-1");
    }

    #[test]
    fn test_get_recent_returns_most_recent_first() {
        let history = TaskHistory::new();

        for i in 0..10 {
            let entry = make_entry("agent-1", &format!("task-{i}"), i as i64);
            history.record(entry);
        }

        let recent = history.get_recent("agent-1", 5);
        assert_eq!(recent.len(), 5);
        // Most recent first (highest offset_secs)
        assert_eq!(recent[0].task_id, "task-9");
        assert_eq!(recent[1].task_id, "task-8");
        assert_eq!(recent[2].task_id, "task-7");
        assert_eq!(recent[3].task_id, "task-6");
        assert_eq!(recent[4].task_id, "task-5");
    }

    #[test]
    fn test_get_recent_with_limit_larger_than_entries() {
        let history = TaskHistory::new();

        for i in 0..3 {
            let entry = make_entry("agent-1", &format!("task-{i}"), i as i64);
            history.record(entry);
        }

        let recent = history.get_recent("agent-1", 100);
        assert_eq!(recent.len(), 3);
    }

    #[test]
    fn test_default_display_limit_is_50() {
        assert_eq!(DEFAULT_DISPLAY_LIMIT, 50);
    }

    #[test]
    fn test_cap_at_200_entries() {
        let history = TaskHistory::new();

        // Insert 250 entries
        for i in 0..250 {
            let entry = make_entry("agent-1", &format!("task-{i}"), i as i64);
            history.record(entry);
        }

        // Should only have 200 entries stored
        let all = history.get_recent("agent-1", 300);
        assert_eq!(all.len(), MAX_ENTRIES_PER_AGENT);

        // The oldest 50 entries (task-0 through task-49) should have been removed
        assert!(history.get_task(&"task-0".to_string()).is_none());
        assert!(history.get_task(&"task-49".to_string()).is_none());
        // task-50 should still exist
        assert!(history.get_task(&"task-50".to_string()).is_some());
        // task-249 (most recent) should exist
        assert!(history.get_task(&"task-249".to_string()).is_some());
    }

    #[test]
    fn test_get_task_found() {
        let history = TaskHistory::new();
        let entry = make_entry("agent-1", "task-42", 0);
        history.record(entry);

        let result = history.get_task(&"task-42".to_string());
        assert!(result.is_some());
        assert_eq!(result.unwrap().task_id, "task-42");
    }

    #[test]
    fn test_get_task_not_found() {
        let history = TaskHistory::new();
        let entry = make_entry("agent-1", "task-1", 0);
        history.record(entry);

        let result = history.get_task(&"nonexistent".to_string());
        assert!(result.is_none());
    }

    #[test]
    fn test_get_task_searches_across_agents() {
        let history = TaskHistory::new();
        history.record(make_entry("agent-1", "task-a", 0));
        history.record(make_entry("agent-2", "task-b", 1));
        history.record(make_entry("agent-3", "task-c", 2));

        // Should find tasks regardless of which agent they belong to
        assert!(history.get_task(&"task-a".to_string()).is_some());
        assert!(history.get_task(&"task-b".to_string()).is_some());
        assert!(history.get_task(&"task-c".to_string()).is_some());
    }

    #[test]
    fn test_separate_agent_histories() {
        let history = TaskHistory::new();

        for i in 0..5 {
            history.record(make_entry("agent-1", &format!("a1-task-{i}"), i as i64));
        }
        for i in 0..3 {
            history.record(make_entry("agent-2", &format!("a2-task-{i}"), i as i64));
        }

        let agent1_recent = history.get_recent("agent-1", DEFAULT_DISPLAY_LIMIT);
        let agent2_recent = history.get_recent("agent-2", DEFAULT_DISPLAY_LIMIT);

        assert_eq!(agent1_recent.len(), 5);
        assert_eq!(agent2_recent.len(), 3);

        // Verify no cross-contamination
        assert!(agent1_recent.iter().all(|e| e.agent_id == "agent-1"));
        assert!(agent2_recent.iter().all(|e| e.agent_id == "agent-2"));
    }

    #[test]
    fn test_get_recent_unknown_agent_returns_empty() {
        let history = TaskHistory::new();
        history.record(make_entry("agent-1", "task-1", 0));

        let recent = history.get_recent("unknown-agent", DEFAULT_DISPLAY_LIMIT);
        assert!(recent.is_empty());
    }
}
