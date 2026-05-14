//! Per-task activity log store backed by SQLite.
//!
//! Records events for each scheduled task: fired, skipped, delivered, failed, response.

use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Mutex;

/// A single log entry for a scheduled task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskLogEntry {
    pub id: i64,
    pub task_id: String,
    pub timestamp: String,
    pub event_type: String,
    pub message: String,
}

/// Event types for task logs.
pub const EVENT_FIRED: &str = "fired";
pub const EVENT_SKIPPED: &str = "skipped";
pub const EVENT_DELIVERED: &str = "delivered";
pub const EVENT_FAILED: &str = "failed";
pub const EVENT_RESPONSE: &str = "response";

/// SQLite-backed task log store.
pub struct TaskLogStore {
    db: Mutex<rusqlite::Connection>,
}

impl TaskLogStore {
    /// Open or create the task log database at the given path.
    pub fn open(db_path: &Path) -> anyhow::Result<Self> {
        let conn = rusqlite::Connection::open(db_path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS task_logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                task_id TEXT NOT NULL,
                timestamp TEXT NOT NULL,
                event_type TEXT NOT NULL,
                message TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_task_logs_task_id ON task_logs(task_id, timestamp DESC);",
        )?;
        Ok(Self { db: Mutex::new(conn) })
    }

    /// Record a log entry for a task.
    pub fn log(&self, task_id: &str, event_type: &str, message: &str) {
        let timestamp = Utc::now().to_rfc3339();
        if let Ok(conn) = self.db.lock() {
            let _ = conn.execute(
                "INSERT INTO task_logs (task_id, timestamp, event_type, message) VALUES (?1, ?2, ?3, ?4)",
                params![task_id, timestamp, event_type, message],
            );
        }
    }

    /// Get recent logs for a specific task (most recent first).
    pub fn get_logs(&self, task_id: &str, limit: usize) -> Vec<TaskLogEntry> {
        let conn = match self.db.lock() {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        let mut stmt = match conn.prepare(
            "SELECT id, task_id, timestamp, event_type, message FROM task_logs WHERE task_id = ?1 ORDER BY id DESC LIMIT ?2",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let rows = stmt.query_map(params![task_id, limit as i64], |row| {
            Ok(TaskLogEntry {
                id: row.get(0)?,
                task_id: row.get(1)?,
                timestamp: row.get(2)?,
                event_type: row.get(3)?,
                message: row.get(4)?,
            })
        });
        match rows {
            Ok(mapped) => mapped.filter_map(|r| r.ok()).collect(),
            Err(_) => vec![],
        }
    }

    /// Get all logs across all tasks (most recent first).
    #[allow(dead_code)] // Available for admin diagnostics
    pub fn get_all_logs(&self, limit: usize) -> Vec<TaskLogEntry> {
        let conn = match self.db.lock() {
            Ok(c) => c,
            Err(_) => return vec![],
        };
        let mut stmt = match conn.prepare(
            "SELECT id, task_id, timestamp, event_type, message FROM task_logs ORDER BY id DESC LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(TaskLogEntry {
                id: row.get(0)?,
                task_id: row.get(1)?,
                timestamp: row.get(2)?,
                event_type: row.get(3)?,
                message: row.get(4)?,
            })
        });
        match rows {
            Ok(mapped) => mapped.filter_map(|r| r.ok()).collect(),
            Err(_) => vec![],
        }
    }

    /// Prune old logs (keep only the most recent N per task).
    #[allow(dead_code)] // Available for periodic maintenance
    pub fn prune(&self, keep_per_task: usize) {
        if let Ok(conn) = self.db.lock() {
            let _ = conn.execute(
                "DELETE FROM task_logs WHERE id NOT IN (
                    SELECT id FROM task_logs t2
                    WHERE t2.task_id = task_logs.task_id
                    ORDER BY t2.id DESC LIMIT ?1
                )",
                params![keep_per_task as i64],
            );
        }
    }
}
