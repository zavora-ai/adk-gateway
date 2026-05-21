//! SQLite-backed persistent task history for coding agents.
//!
//! Stores task execution history in a SQLite database so it survives
//! gateway restarts. Wraps the in-memory `TaskHistory` with a persistence
//! layer that writes to disk on every record and loads on startup.

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::params;
use tracing::{info, warn};

use super::models::{TaskHistoryEntry, TaskId, TaskState, TaskTrigger};

/// SQLite-backed persistent task history store.
pub struct PersistentTaskHistory {
    db: Mutex<rusqlite::Connection>,
}

impl PersistentTaskHistory {
    /// Open or create the task history database at the given path.
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open(db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS coding_agent_tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL UNIQUE,
                agent_id TEXT NOT NULL,
                description TEXT NOT NULL,
                trigger_json TEXT NOT NULL,
                state_json TEXT NOT NULL,
                workspace TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_cat_agent_id ON coding_agent_tasks(agent_id, created_at DESC);
            CREATE INDEX IF NOT EXISTS idx_cat_task_id ON coding_agent_tasks(task_id);",
        )?;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM coding_agent_tasks",
            [],
            |row| row.get(0),
        )?;
        info!(count = count, path = %db_path.display(), "coding agent task history DB opened");

        Ok(Self { db: Mutex::new(conn) })
    }

    /// Record a new task history entry (or update if task_id already exists).
    pub fn record(&self, entry: &TaskHistoryEntry) {
        let Ok(conn) = self.db.lock() else { return };

        let trigger_json = serde_json::to_string(&entry.trigger).unwrap_or_default();
        let state_json = serde_json::to_string(&entry.state).unwrap_or_default();
        let workspace = entry.workspace.display().to_string();
        let created_at = entry.created_at.to_rfc3339();

        let result = conn.execute(
            "INSERT OR REPLACE INTO coding_agent_tasks (task_id, agent_id, description, trigger_json, state_json, workspace, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                entry.task_id,
                entry.agent_id,
                entry.description,
                trigger_json,
                state_json,
                workspace,
                created_at,
            ],
        );

        if let Err(e) = result {
            warn!(task_id = %entry.task_id, error = %e, "failed to persist task history entry");
        }
    }

    /// Update the state of an existing task (e.g., from Running → Completed).
    pub fn update_state(&self, task_id: &TaskId, state: &TaskState) {
        let Ok(conn) = self.db.lock() else { return };
        let state_json = serde_json::to_string(state).unwrap_or_default();

        let _ = conn.execute(
            "UPDATE coding_agent_tasks SET state_json = ?1 WHERE task_id = ?2",
            params![state_json, task_id],
        );
    }

    /// Get recent task history entries for a specific agent (most recent first).
    pub fn get_recent(&self, agent_id: &str, limit: usize) -> Vec<TaskHistoryEntry> {
        let conn = match self.db.lock() {
            Ok(c) => c,
            Err(_) => return vec![],
        };

        let mut stmt = match conn.prepare(
            "SELECT task_id, agent_id, description, trigger_json, state_json, workspace, created_at
             FROM coding_agent_tasks
             WHERE agent_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };

        let rows = stmt.query_map(params![agent_id, limit as i64], |row| {
            let task_id: String = row.get(0)?;
            let agent_id: String = row.get(1)?;
            let description: String = row.get(2)?;
            let trigger_json: String = row.get(3)?;
            let state_json: String = row.get(4)?;
            let workspace: String = row.get(5)?;
            let created_at: String = row.get(6)?;

            Ok((task_id, agent_id, description, trigger_json, state_json, workspace, created_at))
        });

        match rows {
            Ok(mapped) => mapped
                .filter_map(|r| r.ok())
                .filter_map(|(task_id, agent_id, description, trigger_json, state_json, workspace, created_at)| {
                    let trigger: TaskTrigger = serde_json::from_str(&trigger_json).ok()?;
                    let state: TaskState = serde_json::from_str(&state_json).ok()?;
                    let created_at: DateTime<Utc> = created_at.parse().ok()?;

                    Some(TaskHistoryEntry {
                        task_id,
                        agent_id,
                        description,
                        trigger,
                        state,
                        workspace: std::path::PathBuf::from(workspace),
                        created_at,
                    })
                })
                .collect(),
            Err(_) => vec![],
        }
    }

    /// Get a specific task by its task_id.
    pub fn get_task(&self, task_id: &TaskId) -> Option<TaskHistoryEntry> {
        let conn = self.db.lock().ok()?;

        let mut stmt = conn.prepare(
            "SELECT task_id, agent_id, description, trigger_json, state_json, workspace, created_at
             FROM coding_agent_tasks
             WHERE task_id = ?1",
        ).ok()?;

        stmt.query_row(params![task_id], |row| {
            let task_id: String = row.get(0)?;
            let agent_id: String = row.get(1)?;
            let description: String = row.get(2)?;
            let trigger_json: String = row.get(3)?;
            let state_json: String = row.get(4)?;
            let workspace: String = row.get(5)?;
            let created_at: String = row.get(6)?;

            Ok((task_id, agent_id, description, trigger_json, state_json, workspace, created_at))
        }).ok()
        .and_then(|(task_id, agent_id, description, trigger_json, state_json, workspace, created_at)| {
            let trigger: TaskTrigger = serde_json::from_str(&trigger_json).ok()?;
            let state: TaskState = serde_json::from_str(&state_json).ok()?;
            let created_at: DateTime<Utc> = created_at.parse().ok()?;

            Some(TaskHistoryEntry {
                task_id,
                agent_id,
                description,
                trigger,
                state,
                workspace: std::path::PathBuf::from(workspace),
                created_at,
            })
        })
    }

    /// Get total task count for an agent.
    pub fn count(&self, agent_id: &str) -> usize {
        let conn = match self.db.lock() {
            Ok(c) => c,
            Err(_) => return 0,
        };

        conn.query_row(
            "SELECT COUNT(*) FROM coding_agent_tasks WHERE agent_id = ?1",
            params![agent_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0) as usize
    }
}
