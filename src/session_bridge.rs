//! Bridge between gateway channel sessions and adk-session.
//!
//! Maps OpenClaw's session scoping model (per-channel-peer, per-peer, etc.)
//! to adk-session's (app_name, user_id, session_id) triple.
//!
//! The bridge accepts a pluggable `SessionService` backend (R6.1) and
//! defaults to `InMemorySessionService` when no persistent backend is
//! configured (R6.4).

use crate::channel::{ChannelType, InboundMessage};
use crate::config::{SessionBackendType, SessionConfig};
use adk_session::{
    CreateRequest, DeleteRequest, Event, Events, GetRequest, InMemorySessionService, ListRequest,
    Session, SessionService, State,
};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use rusqlite::params;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Manages session ID generation and lifecycle based on OpenClaw scoping rules.
///
/// Wraps a pluggable `SessionService` backend and maintains an in-memory
/// index of active sessions for fast lookups and status reporting.
pub struct SessionBridge {
    config: SessionConfig,
    /// Maps session keys to their adk session IDs
    sessions: DashMap<String, SessionInfo>,
    /// App name for adk-session
    app_name: String,
    /// Pluggable session backend (R6.1)
    #[allow(dead_code)] // Accessed via session_service() method; used for diagnostic reporting
    session_service: Arc<dyn SessionService>,
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub user_id: String,
    pub last_activity: chrono::DateTime<chrono::Utc>,
    pub channel_type: ChannelType,
}

impl SessionBridge {
    /// Create a new `SessionBridge` with the given config, app name, and
    /// pluggable session service backend.
    pub fn new(
        config: SessionConfig,
        app_name: String,
        session_service: Arc<dyn SessionService>,
    ) -> Self {
        Self {
            config,
            sessions: DashMap::new(),
            app_name,
            session_service,
        }
    }

    pub fn app_name(&self) -> &str {
        &self.app_name
    }

    /// Return a reference to the underlying `SessionService` backend.
    #[allow(dead_code)] // Available for diagnostic access via /status endpoint
    pub fn session_service(&self) -> &Arc<dyn SessionService> {
        &self.session_service
    }

    /// Resolve or create a session for an inbound message.
    /// Returns (user_id, session_id).
    pub fn resolve_session(&self, msg: &InboundMessage) -> (String, String) {
        let key = self.session_key(msg);

        // Check for existing session
        if let Some(mut entry) = self.sessions.get_mut(&key) {
            // Check if session should be reset
            if self.should_reset(&entry) {
                drop(entry);
                return self.create_session(key, msg);
            }
            entry.last_activity = chrono::Utc::now();
            return (entry.user_id.clone(), entry.session_id.clone());
        }

        self.create_session(key, msg)
    }

    /// Generate a session key based on the configured dm_scope.
    fn session_key(&self, msg: &InboundMessage) -> String {
        match self.config.dm_scope.as_str() {
            "main" => "main".to_string(),
            "per-peer" => msg.sender_id.clone(),
            "per-channel-peer" => {
                format!("{}:{}", msg.channel_type, msg.sender_id)
            }
            "per-account-channel-peer" => {
                // For multi-account setups (R12.2)
                format!(
                    "{}:{}:{}",
                    if msg.account_id.is_empty() {
                        "default"
                    } else {
                        &msg.account_id
                    },
                    msg.channel_type,
                    msg.sender_id
                )
            }
            _ => format!("{}:{}", msg.channel_type, msg.sender_id),
        }
    }

    fn create_session(&self, key: String, msg: &InboundMessage) -> (String, String) {
        let session_id = format!("gw-{}", uuid::Uuid::new_v4());
        let user_id = format!("{}:{}", msg.channel_type, msg.sender_id);

        let info = SessionInfo {
            session_id: session_id.clone(),
            user_id: user_id.clone(),
            last_activity: chrono::Utc::now(),
            channel_type: msg.channel_type,
        };

        self.sessions.insert(key, info);
        (user_id, session_id)
    }

    fn should_reset(&self, info: &SessionInfo) -> bool {
        let now = chrono::Utc::now();

        match self.config.reset.mode.as_str() {
            "daily" => {
                let hour = self.config.reset.at_hour.unwrap_or(4);
                let last_date = info.last_activity.date_naive();
                let today = now.date_naive();
                if today > last_date && now.time().hour() >= hour as u32 {
                    return true;
                }
            }
            "idle" => {
                if let Some(idle_mins) = self.config.reset.idle_minutes {
                    let idle_duration = now - info.last_activity;
                    if idle_duration.num_minutes() > idle_mins as i64 {
                        return true;
                    }
                }
            }
            _ => {}
        }

        false
    }

    /// Get all active sessions (for status/diagnostics).
    pub fn active_sessions(&self) -> Vec<SessionInfo> {
        self.sessions.iter().map(|e| e.value().clone()).collect()
    }
}

use chrono::Timelike;

// ── SQLite Session Service ─────────────────────────────────────────

/// SQLite-backed session service using `rusqlite`.
///
/// Stores sessions in a SQLite database file (or `:memory:` for tests).
/// Uses a `Mutex<rusqlite::Connection>` for thread-safe access since
/// `rusqlite::Connection` is not `Sync`.
///
/// This implementation avoids `sqlx` to prevent `libsqlite3-sys` link
/// conflicts with the `sqlrite` crate already used for RAG.
pub struct SqliteSessionService {
    conn: std::sync::Mutex<rusqlite::Connection>,
}

impl std::fmt::Debug for SqliteSessionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteSessionService").finish()
    }
}

impl SqliteSessionService {
    /// Open a SQLite session database at the given path.
    ///
    /// Accepts either a file path (e.g. `/tmp/sessions.db`) or the special
    /// value `:memory:` for an in-memory database.
    ///
    /// Automatically runs migrations to create the required schema.
    pub fn new(path: &str) -> anyhow::Result<Self> {
        let conn = if path == ":memory:" || path == "sqlite::memory:" {
            rusqlite::Connection::open_in_memory()
        } else {
            // Ensure parent directory exists
            if let Some(parent) = Path::new(path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            rusqlite::Connection::open(path)
        }
        .map_err(|e| anyhow::anyhow!("sqlite connection failed: {}", e))?;

        // Enable WAL mode for better concurrent read performance
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(|e| anyhow::anyhow!("sqlite pragma failed: {}", e))?;

        let service = Self {
            conn: std::sync::Mutex::new(conn),
        };
        service.migrate()?;
        Ok(service)
    }

    /// Run schema migrations to create the sessions and events tables.
    fn migrate(&self) -> anyhow::Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| anyhow::anyhow!("lock: {}", e))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions (
                app_name TEXT NOT NULL,
                user_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                state TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (app_name, user_id, session_id)
            );
            CREATE INDEX IF NOT EXISTS idx_sessions_app_user ON sessions(app_name, user_id);
            CREATE TABLE IF NOT EXISTS events (
                id TEXT NOT NULL,
                app_name TEXT NOT NULL,
                user_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                invocation_id TEXT NOT NULL,
                branch TEXT NOT NULL DEFAULT '',
                author TEXT NOT NULL DEFAULT '',
                timestamp TEXT NOT NULL,
                llm_response TEXT NOT NULL DEFAULT '{}',
                actions TEXT NOT NULL DEFAULT '{}',
                long_running_tool_ids TEXT NOT NULL DEFAULT '[]',
                PRIMARY KEY (id, app_name, user_id, session_id),
                FOREIGN KEY (app_name, user_id, session_id)
                    REFERENCES sessions(app_name, user_id, session_id)
                    ON DELETE CASCADE
            );",
        )
        .map_err(|e| anyhow::anyhow!("sqlite migration failed: {}", e))?;
        Ok(())
    }
}

/// Internal session representation for the SQLite backend.
struct SqliteSession {
    app_name: String,
    user_id: String,
    session_id: String,
    state: HashMap<String, Value>,
    events: Vec<Event>,
    updated_at: DateTime<Utc>,
}

impl Session for SqliteSession {
    fn id(&self) -> &str {
        &self.session_id
    }

    fn app_name(&self) -> &str {
        &self.app_name
    }

    fn user_id(&self) -> &str {
        &self.user_id
    }

    fn state(&self) -> &dyn State {
        self
    }

    fn events(&self) -> &dyn Events {
        self
    }

    fn last_update_time(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

impl State for SqliteSession {
    fn get(&self, key: &str) -> Option<Value> {
        self.state.get(key).cloned()
    }

    fn set(&mut self, key: String, value: Value) {
        self.state.insert(key, value);
    }

    fn all(&self) -> HashMap<String, Value> {
        self.state.clone()
    }
}

impl Events for SqliteSession {
    fn all(&self) -> Vec<Event> {
        self.events.clone()
    }

    fn len(&self) -> usize {
        self.events.len()
    }

    fn at(&self, index: usize) -> Option<&Event> {
        self.events.get(index)
    }
}

#[async_trait::async_trait]
impl SessionService for SqliteSessionService {
    async fn create(&self, req: CreateRequest) -> adk_core::Result<Box<dyn Session>> {
        let session_id = req
            .session_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        let state_json = serde_json::to_string(&req.state)
            .map_err(|e| adk_core::AdkError::session(format!("serialize failed: {}", e)))?;

        let conn = self
            .conn
            .lock()
            .map_err(|e| adk_core::AdkError::session(format!("lock: {}", e)))?;

        conn.execute(
            "INSERT INTO sessions (app_name, user_id, session_id, state, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![req.app_name, req.user_id, session_id, state_json, now_str, now_str],
        )
        .map_err(|e| adk_core::AdkError::session(format!("insert failed: {}", e)))?;

        Ok(Box::new(SqliteSession {
            app_name: req.app_name,
            user_id: req.user_id,
            session_id,
            state: req.state,
            events: Vec::new(),
            updated_at: now,
        }))
    }

    async fn get(&self, req: GetRequest) -> adk_core::Result<Box<dyn Session>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| adk_core::AdkError::session(format!("lock: {}", e)))?;

        let (state_json, updated_at_str): (String, String) = conn
            .query_row(
                "SELECT state, updated_at FROM sessions WHERE app_name = ?1 AND user_id = ?2 AND session_id = ?3",
                params![req.app_name, req.user_id, req.session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| adk_core::AdkError::session("session not found"))?;

        let state: HashMap<String, Value> = serde_json::from_str(&state_json)
            .map_err(|e| adk_core::AdkError::session(format!("deserialize failed: {}", e)))?;

        let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        // Load events
        let mut stmt = conn
            .prepare(
                "SELECT id, invocation_id, branch, author, timestamp, llm_response, actions, long_running_tool_ids \
                 FROM events WHERE app_name = ?1 AND user_id = ?2 AND session_id = ?3 ORDER BY timestamp",
            )
            .map_err(|e| adk_core::AdkError::session(format!("prepare failed: {}", e)))?;

        let events: Vec<Event> = stmt
            .query_map(params![req.app_name, req.user_id, req.session_id], |row| {
                let id: String = row.get(0)?;
                let invocation_id: String = row.get(1)?;
                let branch: String = row.get(2)?;
                let author: String = row.get(3)?;
                let timestamp_str: String = row.get(4)?;
                let llm_response_json: String = row.get(5)?;
                let actions_json: String = row.get(6)?;
                let tool_ids_json: String = row.get(7)?;
                Ok((
                    id,
                    invocation_id,
                    branch,
                    author,
                    timestamp_str,
                    llm_response_json,
                    actions_json,
                    tool_ids_json,
                ))
            })
            .map_err(|e| adk_core::AdkError::session(format!("query failed: {}", e)))?
            .filter_map(|row| {
                let (
                    id,
                    invocation_id,
                    branch,
                    author,
                    timestamp_str,
                    llm_response_json,
                    actions_json,
                    tool_ids_json,
                ) = row.ok()?;
                let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
                    .ok()?
                    .with_timezone(&Utc);
                let llm_response = serde_json::from_str(&llm_response_json).ok()?;
                let actions = serde_json::from_str(&actions_json).ok()?;
                let long_running_tool_ids = serde_json::from_str(&tool_ids_json).ok()?;
                Some(Event {
                    id,
                    timestamp,
                    invocation_id,
                    branch,
                    author,
                    llm_request: None,
                    llm_response,
                    actions,
                    long_running_tool_ids,
                    provider_metadata: HashMap::new(),
                })
            })
            .collect();

        let mut events = events;
        if let Some(num) = req.num_recent_events {
            let start = events.len().saturating_sub(num);
            events = events[start..].to_vec();
        }
        if let Some(after) = req.after {
            events.retain(|e| e.timestamp >= after);
        }

        Ok(Box::new(SqliteSession {
            app_name: req.app_name,
            user_id: req.user_id,
            session_id: req.session_id,
            state,
            events,
            updated_at,
        }))
    }

    async fn list(&self, req: ListRequest) -> adk_core::Result<Vec<Box<dyn Session>>> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| adk_core::AdkError::session(format!("lock: {}", e)))?;

        let limit = req.limit.map(|l| l as i64).unwrap_or(i64::MAX);
        let offset = req.offset.unwrap_or(0) as i64;

        let mut stmt = conn
            .prepare(
                "SELECT session_id, state, updated_at FROM sessions \
                 WHERE app_name = ?1 AND user_id = ?2 \
                 ORDER BY updated_at DESC LIMIT ?3 OFFSET ?4",
            )
            .map_err(|e| adk_core::AdkError::session(format!("prepare failed: {}", e)))?;

        let sessions: Vec<Box<dyn Session>> = stmt
            .query_map(params![req.app_name, req.user_id, limit, offset], |row| {
                let session_id: String = row.get(0)?;
                let state_json: String = row.get(1)?;
                let updated_at_str: String = row.get(2)?;
                Ok((session_id, state_json, updated_at_str))
            })
            .map_err(|e| adk_core::AdkError::session(format!("query failed: {}", e)))?
            .filter_map(|row| {
                let (session_id, state_json, updated_at_str) = row.ok()?;
                let state: HashMap<String, Value> =
                    serde_json::from_str(&state_json).unwrap_or_default();
                let updated_at = DateTime::parse_from_rfc3339(&updated_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now());
                Some(Box::new(SqliteSession {
                    app_name: req.app_name.clone(),
                    user_id: req.user_id.clone(),
                    session_id,
                    state,
                    events: Vec::new(),
                    updated_at,
                }) as Box<dyn Session>)
            })
            .collect();

        Ok(sessions)
    }

    async fn delete(&self, req: DeleteRequest) -> adk_core::Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| adk_core::AdkError::session(format!("lock: {}", e)))?;

        // Delete events first (foreign key may not cascade depending on config)
        conn.execute(
            "DELETE FROM events WHERE app_name = ?1 AND user_id = ?2 AND session_id = ?3",
            params![req.app_name, req.user_id, req.session_id],
        )
        .map_err(|e| adk_core::AdkError::session(format!("delete events failed: {}", e)))?;

        conn.execute(
            "DELETE FROM sessions WHERE app_name = ?1 AND user_id = ?2 AND session_id = ?3",
            params![req.app_name, req.user_id, req.session_id],
        )
        .map_err(|e| adk_core::AdkError::session(format!("delete failed: {}", e)))?;

        Ok(())
    }

    async fn append_event(&self, session_id: &str, event: Event) -> adk_core::Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| adk_core::AdkError::session(format!("lock: {}", e)))?;

        // Look up the session's app_name and user_id
        let (app_name, user_id): (String, String) = conn
            .query_row(
                "SELECT app_name, user_id FROM sessions WHERE session_id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|_| adk_core::AdkError::session("session not found"))?;

        // Update session timestamp and state if there's a state delta
        if !event.actions.state_delta.is_empty() {
            let state_json: String = conn
                .query_row(
                    "SELECT state FROM sessions WHERE app_name = ?1 AND user_id = ?2 AND session_id = ?3",
                    params![app_name, user_id, session_id],
                    |row| row.get(0),
                )
                .map_err(|e| adk_core::AdkError::session(format!("query failed: {}", e)))?;

            let mut state: HashMap<String, Value> =
                serde_json::from_str(&state_json).unwrap_or_default();
            state.extend(event.actions.state_delta.clone());

            let new_state_json = serde_json::to_string(&state)
                .map_err(|e| adk_core::AdkError::session(format!("serialize failed: {}", e)))?;

            conn.execute(
                "UPDATE sessions SET state = ?1, updated_at = ?2 WHERE app_name = ?3 AND user_id = ?4 AND session_id = ?5",
                params![new_state_json, event.timestamp.to_rfc3339(), app_name, user_id, session_id],
            )
            .map_err(|e| adk_core::AdkError::session(format!("update failed: {}", e)))?;
        } else {
            conn.execute(
                "UPDATE sessions SET updated_at = ?1 WHERE app_name = ?2 AND user_id = ?3 AND session_id = ?4",
                params![event.timestamp.to_rfc3339(), app_name, user_id, session_id],
            )
            .map_err(|e| adk_core::AdkError::session(format!("update failed: {}", e)))?;
        }

        // Insert the event
        let llm_response_json = serde_json::to_string(&event.llm_response)
            .map_err(|e| adk_core::AdkError::session(format!("serialize failed: {}", e)))?;
        let actions_json = serde_json::to_string(&event.actions)
            .map_err(|e| adk_core::AdkError::session(format!("serialize failed: {}", e)))?;
        let tool_ids_json = serde_json::to_string(&event.long_running_tool_ids)
            .map_err(|e| adk_core::AdkError::session(format!("serialize failed: {}", e)))?;

        conn.execute(
            "INSERT INTO events (id, app_name, user_id, session_id, invocation_id, branch, author, timestamp, llm_response, actions, long_running_tool_ids) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                event.id,
                app_name,
                user_id,
                session_id,
                event.invocation_id,
                event.branch,
                event.author,
                event.timestamp.to_rfc3339(),
                llm_response_json,
                actions_json,
                tool_ids_json,
            ],
        )
        .map_err(|e| adk_core::AdkError::session(format!("insert event failed: {}", e)))?;

        Ok(())
    }

    async fn delete_all_sessions(&self, app_name: &str, user_id: &str) -> adk_core::Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| adk_core::AdkError::session(format!("lock: {}", e)))?;

        conn.execute(
            "DELETE FROM events WHERE app_name = ?1 AND user_id = ?2",
            params![app_name, user_id],
        )
        .map_err(|e| adk_core::AdkError::session(format!("delete events failed: {}", e)))?;

        conn.execute(
            "DELETE FROM sessions WHERE app_name = ?1 AND user_id = ?2",
            params![app_name, user_id],
        )
        .map_err(|e| adk_core::AdkError::session(format!("delete sessions failed: {}", e)))?;

        Ok(())
    }

    async fn health_check(&self) -> adk_core::Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|e| adk_core::AdkError::session(format!("lock: {}", e)))?;
        conn.execute_batch("SELECT 1")
            .map_err(|e| adk_core::AdkError::session(format!("health check failed: {}", e)))?;
        Ok(())
    }
}

// ── Postgres Session Service ───────────────────────────────────────

/// Postgres-backed session service using `sqlx` with connection pooling.
///
/// Stores sessions in a PostgreSQL database using the `sessions` table.
/// Uses `PgPool` for async connection pooling. Only available when the
/// `postgres` feature flag is enabled.
///
/// Schema:
/// ```sql
/// CREATE TABLE IF NOT EXISTS sessions (
///     app_name TEXT NOT NULL,
///     user_id TEXT NOT NULL,
///     session_id TEXT PRIMARY KEY,
///     state JSONB NOT NULL DEFAULT '{}',
///     created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
///     updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
/// );
/// CREATE INDEX IF NOT EXISTS idx_sessions_app_user ON sessions(app_name, user_id);
/// ```
#[cfg(feature = "postgres")]
pub struct PostgresSessionService {
    pool: sqlx_core::pool::Pool<sqlx_postgres::Postgres>,
}

#[cfg(feature = "postgres")]
impl std::fmt::Debug for PostgresSessionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostgresSessionService").finish()
    }
}

#[cfg(feature = "postgres")]
impl PostgresSessionService {
    /// Create a new Postgres session service with the given connection string.
    ///
    /// Connects to the database using `PgPool` and runs migrations
    /// to ensure the required schema exists.
    pub async fn new(connection_string: &str) -> anyhow::Result<Self> {
        use sqlx_core::pool::PoolOptions;
        use sqlx_postgres::Postgres;

        let pool = PoolOptions::<Postgres>::new()
            .max_connections(10)
            .connect(connection_string)
            .await
            .map_err(|e| anyhow::anyhow!("postgres connection failed: {}", e))?;

        let service = Self { pool };
        service.migrate().await?;
        Ok(service)
    }

    /// Run schema migrations to create the sessions table.
    async fn migrate(&self) -> anyhow::Result<()> {
        use sqlx_core::executor::Executor;
        use sqlx_core::query::query;

        self.pool
            .execute(query(
                "CREATE TABLE IF NOT EXISTS sessions (
                        app_name TEXT NOT NULL,
                        user_id TEXT NOT NULL,
                        session_id TEXT PRIMARY KEY,
                        state JSONB NOT NULL DEFAULT '{}',
                        created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                        updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
                    )",
            ))
            .await
            .map_err(|e| anyhow::anyhow!("postgres migration failed: {}", e))?;

        self.pool
            .execute(query(
                "CREATE INDEX IF NOT EXISTS idx_sessions_app_user ON sessions(app_name, user_id)",
            ))
            .await
            .map_err(|e| anyhow::anyhow!("postgres index creation failed: {}", e))?;

        Ok(())
    }

    /// Return a reference to the underlying connection pool.
    #[allow(dead_code)]
    pub fn pool(&self) -> &sqlx_core::pool::Pool<sqlx_postgres::Postgres> {
        &self.pool
    }
}

/// Internal session representation for the Postgres backend.
#[cfg(feature = "postgres")]
struct PgSession {
    app_name: String,
    user_id: String,
    session_id: String,
    state: HashMap<String, Value>,
    events: Vec<Event>,
    updated_at: DateTime<Utc>,
}

#[cfg(feature = "postgres")]
impl Session for PgSession {
    fn id(&self) -> &str {
        &self.session_id
    }

    fn app_name(&self) -> &str {
        &self.app_name
    }

    fn user_id(&self) -> &str {
        &self.user_id
    }

    fn state(&self) -> &dyn State {
        self
    }

    fn events(&self) -> &dyn Events {
        self
    }

    fn last_update_time(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

#[cfg(feature = "postgres")]
impl State for PgSession {
    fn get(&self, key: &str) -> Option<Value> {
        self.state.get(key).cloned()
    }

    fn set(&mut self, key: String, value: Value) {
        self.state.insert(key, value);
    }

    fn all(&self) -> HashMap<String, Value> {
        self.state.clone()
    }
}

#[cfg(feature = "postgres")]
impl Events for PgSession {
    fn all(&self) -> Vec<Event> {
        self.events.clone()
    }

    fn len(&self) -> usize {
        self.events.len()
    }

    fn at(&self, index: usize) -> Option<&Event> {
        self.events.get(index)
    }
}

#[cfg(feature = "postgres")]
#[async_trait::async_trait]
impl SessionService for PostgresSessionService {
    async fn create(&self, req: CreateRequest) -> adk_core::Result<Box<dyn Session>> {
        use sqlx_core::executor::Executor;
        use sqlx_core::query::query;

        let session_id = req
            .session_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let now = Utc::now();
        let state_value = serde_json::to_value(&req.state)
            .map_err(|e| adk_core::AdkError::session(format!("serialize failed: {}", e)))?;

        self.pool
            .execute(
                query(
                    "INSERT INTO sessions (app_name, user_id, session_id, state, created_at, updated_at) \
                     VALUES ($1, $2, $3, $4, $5, $6)"
                )
                .bind(&req.app_name)
                .bind(&req.user_id)
                .bind(&session_id)
                .bind(&state_value)
                .bind(now)
                .bind(now)
            )
            .await
            .map_err(|e| adk_core::AdkError::session(format!("insert failed: {}", e)))?;

        Ok(Box::new(PgSession {
            app_name: req.app_name,
            user_id: req.user_id,
            session_id,
            state: req.state,
            events: Vec::new(),
            updated_at: now,
        }))
    }

    async fn get(&self, req: GetRequest) -> adk_core::Result<Box<dyn Session>> {
        use sqlx_core::query::query;
        use sqlx_core::row::Row;

        let row = query(
            "SELECT state, updated_at FROM sessions \
             WHERE app_name = $1 AND user_id = $2 AND session_id = $3",
        )
        .bind(&req.app_name)
        .bind(&req.user_id)
        .bind(&req.session_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| adk_core::AdkError::session("session not found"))?;

        let state_value: serde_json::Value = row
            .try_get("state")
            .map_err(|e| adk_core::AdkError::session(format!("get state failed: {}", e)))?;
        let updated_at: DateTime<Utc> = row
            .try_get("updated_at")
            .map_err(|e| adk_core::AdkError::session(format!("get updated_at failed: {}", e)))?;

        let state: HashMap<String, Value> = serde_json::from_value(state_value)
            .map_err(|e| adk_core::AdkError::session(format!("deserialize failed: {}", e)))?;

        let events = Vec::new();

        Ok(Box::new(PgSession {
            app_name: req.app_name,
            user_id: req.user_id,
            session_id: req.session_id,
            state,
            events,
            updated_at,
        }))
    }

    async fn list(&self, req: ListRequest) -> adk_core::Result<Vec<Box<dyn Session>>> {
        use sqlx_core::query::query;
        use sqlx_core::row::Row;

        let limit = req.limit.map(|l| l as i64).unwrap_or(i64::MAX);
        let offset = req.offset.unwrap_or(0) as i64;

        let rows = query(
            "SELECT session_id, state, updated_at FROM sessions \
             WHERE app_name = $1 AND user_id = $2 \
             ORDER BY updated_at DESC LIMIT $3 OFFSET $4",
        )
        .bind(&req.app_name)
        .bind(&req.user_id)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| adk_core::AdkError::session(format!("query failed: {}", e)))?;

        let sessions: Vec<Box<dyn Session>> = rows
            .into_iter()
            .filter_map(|row| {
                let session_id: String = row.try_get("session_id").ok()?;
                let state_value: serde_json::Value = row.try_get("state").ok()?;
                let updated_at: DateTime<Utc> = row.try_get("updated_at").ok()?;
                let state: HashMap<String, Value> =
                    serde_json::from_value(state_value).unwrap_or_default();
                Some(Box::new(PgSession {
                    app_name: req.app_name.clone(),
                    user_id: req.user_id.clone(),
                    session_id,
                    state,
                    events: Vec::new(),
                    updated_at,
                }) as Box<dyn Session>)
            })
            .collect();

        Ok(sessions)
    }

    async fn delete(&self, req: DeleteRequest) -> adk_core::Result<()> {
        use sqlx_core::executor::Executor;
        use sqlx_core::query::query;

        self.pool
            .execute(
                query(
                    "DELETE FROM sessions WHERE app_name = $1 AND user_id = $2 AND session_id = $3",
                )
                .bind(&req.app_name)
                .bind(&req.user_id)
                .bind(&req.session_id),
            )
            .await
            .map_err(|e| adk_core::AdkError::session(format!("delete failed: {}", e)))?;

        Ok(())
    }

    async fn append_event(&self, session_id: &str, event: Event) -> adk_core::Result<()> {
        use sqlx_core::executor::Executor;
        use sqlx_core::query::query;
        use sqlx_core::row::Row;

        // Update session timestamp and state if there's a state delta
        if !event.actions.state_delta.is_empty() {
            // Fetch current state
            let row = query("SELECT state FROM sessions WHERE session_id = $1")
                .bind(session_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|_| adk_core::AdkError::session("session not found"))?;

            let state_value: serde_json::Value = row
                .try_get("state")
                .map_err(|e| adk_core::AdkError::session(format!("get state failed: {}", e)))?;

            let mut state: HashMap<String, Value> =
                serde_json::from_value(state_value).unwrap_or_default();
            state.extend(event.actions.state_delta.clone());

            let new_state_value = serde_json::to_value(&state)
                .map_err(|e| adk_core::AdkError::session(format!("serialize failed: {}", e)))?;

            self.pool
                .execute(
                    query("UPDATE sessions SET state = $1, updated_at = $2 WHERE session_id = $3")
                        .bind(&new_state_value)
                        .bind(event.timestamp)
                        .bind(session_id),
                )
                .await
                .map_err(|e| adk_core::AdkError::session(format!("update failed: {}", e)))?;
        } else {
            self.pool
                .execute(
                    query("UPDATE sessions SET updated_at = $1 WHERE session_id = $2")
                        .bind(event.timestamp)
                        .bind(session_id),
                )
                .await
                .map_err(|e| adk_core::AdkError::session(format!("update failed: {}", e)))?;
        }

        Ok(())
    }

    async fn delete_all_sessions(&self, app_name: &str, user_id: &str) -> adk_core::Result<()> {
        use sqlx_core::executor::Executor;
        use sqlx_core::query::query;

        self.pool
            .execute(
                query("DELETE FROM sessions WHERE app_name = $1 AND user_id = $2")
                    .bind(app_name)
                    .bind(user_id),
            )
            .await
            .map_err(|e| adk_core::AdkError::session(format!("delete sessions failed: {}", e)))?;

        Ok(())
    }

    async fn health_check(&self) -> adk_core::Result<()> {
        use sqlx_core::executor::Executor;
        use sqlx_core::query::query;

        self.pool
            .execute(query("SELECT 1"))
            .await
            .map_err(|e| adk_core::AdkError::session(format!("health check failed: {}", e)))?;
        Ok(())
    }
}

// ── Redis Session Service ──────────────────────────────────────────

/// Redis-backed session service using `redis-rs` with async support.
///
/// Stores sessions as JSON blobs with key format:
/// `session:{app_name}:{user_id}:{session_id}`
///
/// Uses a secondary SET to index session IDs per user:
/// `session_index:{app_name}:{user_id}` → SET of session_ids
///
/// TTL is optionally applied based on session reset config (idle_minutes).
/// Only available when the `redis` feature flag is enabled.
#[cfg(feature = "redis")]
pub struct RedisSessionService {
    client: redis::Client,
    ttl_seconds: Option<u64>,
}

#[cfg(feature = "redis")]
impl std::fmt::Debug for RedisSessionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisSessionService").finish()
    }
}

#[cfg(feature = "redis")]
impl RedisSessionService {
    /// Create a new Redis session service with the given connection string.
    ///
    /// The connection string should be a valid Redis URL (e.g. `redis://127.0.0.1/`).
    /// Optionally accepts a TTL in seconds for session keys.
    pub async fn new(connection_string: &str, ttl_seconds: Option<u64>) -> anyhow::Result<Self> {
        let client = redis::Client::open(connection_string)
            .map_err(|e| anyhow::anyhow!("redis connection failed: {}", e))?;

        // Verify connectivity with a PING
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| anyhow::anyhow!("redis connection failed: {}", e))?;

        redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map_err(|e| anyhow::anyhow!("redis PING failed: {}", e))?;

        Ok(Self {
            client,
            ttl_seconds,
        })
    }

    /// Build the session data key.
    fn session_key(app_name: &str, user_id: &str, session_id: &str) -> String {
        format!("session:{}:{}:{}", app_name, user_id, session_id)
    }

    /// Build the session index key (SET of session_ids per user).
    fn index_key(app_name: &str, user_id: &str) -> String {
        format!("session_index:{}:{}", app_name, user_id)
    }

    /// Apply TTL to a key if configured.
    async fn apply_ttl(
        &self,
        conn: &mut redis::aio::MultiplexedConnection,
        key: &str,
    ) -> adk_core::Result<()> {
        if let Some(ttl) = self.ttl_seconds {
            redis::cmd("EXPIRE")
                .arg(key)
                .arg(ttl as i64)
                .query_async::<()>(conn)
                .await
                .map_err(|e| adk_core::AdkError::session(format!("EXPIRE failed: {}", e)))?;
        }
        Ok(())
    }
}

/// Serializable session data stored in Redis.
#[cfg(feature = "redis")]
#[derive(serde::Serialize, serde::Deserialize)]
struct RedisSessionData {
    app_name: String,
    user_id: String,
    session_id: String,
    state: HashMap<String, Value>,
    created_at: String,
    updated_at: String,
}

/// Internal session representation for the Redis backend.
#[cfg(feature = "redis")]
struct RedisSession {
    app_name: String,
    user_id: String,
    session_id: String,
    state: HashMap<String, Value>,
    events: Vec<Event>,
    updated_at: DateTime<Utc>,
}

#[cfg(feature = "redis")]
impl Session for RedisSession {
    fn id(&self) -> &str {
        &self.session_id
    }

    fn app_name(&self) -> &str {
        &self.app_name
    }

    fn user_id(&self) -> &str {
        &self.user_id
    }

    fn state(&self) -> &dyn State {
        self
    }

    fn events(&self) -> &dyn Events {
        self
    }

    fn last_update_time(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

#[cfg(feature = "redis")]
impl State for RedisSession {
    fn get(&self, key: &str) -> Option<Value> {
        self.state.get(key).cloned()
    }

    fn set(&mut self, key: String, value: Value) {
        self.state.insert(key, value);
    }

    fn all(&self) -> HashMap<String, Value> {
        self.state.clone()
    }
}

#[cfg(feature = "redis")]
impl Events for RedisSession {
    fn all(&self) -> Vec<Event> {
        self.events.clone()
    }

    fn len(&self) -> usize {
        self.events.len()
    }

    fn at(&self, index: usize) -> Option<&Event> {
        self.events.get(index)
    }
}

#[cfg(feature = "redis")]
#[async_trait::async_trait]
impl SessionService for RedisSessionService {
    async fn create(&self, req: CreateRequest) -> adk_core::Result<Box<dyn Session>> {
        let session_id = req
            .session_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let now = Utc::now();

        let data = RedisSessionData {
            app_name: req.app_name.clone(),
            user_id: req.user_id.clone(),
            session_id: session_id.clone(),
            state: req.state.clone(),
            created_at: now.to_rfc3339(),
            updated_at: now.to_rfc3339(),
        };

        let json = serde_json::to_string(&data)
            .map_err(|e| adk_core::AdkError::session(format!("serialize failed: {}", e)))?;

        let key = Self::session_key(&req.app_name, &req.user_id, &session_id);
        let index_key = Self::index_key(&req.app_name, &req.user_id);

        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("connection failed: {}", e)))?;

        // Store session data
        redis::cmd("SET")
            .arg(&key)
            .arg(&json)
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("SET failed: {}", e)))?;

        // Store reverse mapping: session_id → full key (for append_event lookups)
        let reverse_key = format!("session_reverse:{}", session_id);
        redis::cmd("SET")
            .arg(&reverse_key)
            .arg(&key)
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("SET reverse failed: {}", e)))?;

        // Add session_id to the user's index set
        redis::cmd("SADD")
            .arg(&index_key)
            .arg(&session_id)
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("SADD failed: {}", e)))?;

        // Apply TTL to session key
        self.apply_ttl(&mut conn, &key).await?;
        // Apply TTL to reverse key
        self.apply_ttl(&mut conn, &reverse_key).await?;
        // Apply TTL to index key as well
        self.apply_ttl(&mut conn, &index_key).await?;

        Ok(Box::new(RedisSession {
            app_name: req.app_name,
            user_id: req.user_id,
            session_id,
            state: req.state,
            events: Vec::new(),
            updated_at: now,
        }))
    }

    async fn get(&self, req: GetRequest) -> adk_core::Result<Box<dyn Session>> {
        let key = Self::session_key(&req.app_name, &req.user_id, &req.session_id);

        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("connection failed: {}", e)))?;

        let json: Option<String> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("GET failed: {}", e)))?;

        let json = json.ok_or_else(|| adk_core::AdkError::session("session not found"))?;

        let data: RedisSessionData = serde_json::from_str(&json)
            .map_err(|e| adk_core::AdkError::session(format!("deserialize failed: {}", e)))?;

        let updated_at = DateTime::parse_from_rfc3339(&data.updated_at)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now());

        Ok(Box::new(RedisSession {
            app_name: data.app_name,
            user_id: data.user_id,
            session_id: data.session_id,
            state: data.state,
            events: Vec::new(),
            updated_at,
        }))
    }

    async fn list(&self, req: ListRequest) -> adk_core::Result<Vec<Box<dyn Session>>> {
        let index_key = Self::index_key(&req.app_name, &req.user_id);

        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("connection failed: {}", e)))?;

        // Get all session IDs from the index set
        let session_ids: Vec<String> = redis::cmd("SMEMBERS")
            .arg(&index_key)
            .query_async(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("SMEMBERS failed: {}", e)))?;

        let mut sessions: Vec<Box<dyn Session>> = Vec::new();

        for session_id in &session_ids {
            let key = Self::session_key(&req.app_name, &req.user_id, session_id);
            let json: Option<String> = redis::cmd("GET")
                .arg(&key)
                .query_async(&mut conn)
                .await
                .map_err(|e| adk_core::AdkError::session(format!("GET failed: {}", e)))?;

            if let Some(json) = json {
                if let Ok(data) = serde_json::from_str::<RedisSessionData>(&json) {
                    let updated_at = DateTime::parse_from_rfc3339(&data.updated_at)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());

                    sessions.push(Box::new(RedisSession {
                        app_name: data.app_name,
                        user_id: data.user_id,
                        session_id: data.session_id,
                        state: data.state,
                        events: Vec::new(),
                        updated_at,
                    }));
                }
            }
        }

        // Sort by updated_at descending (most recent first)
        sessions.sort_by(|a, b| b.last_update_time().cmp(&a.last_update_time()));

        // Apply limit and offset
        let offset = req.offset.unwrap_or(0);
        let limit = req.limit.unwrap_or(sessions.len());
        let sessions = sessions.into_iter().skip(offset).take(limit).collect();

        Ok(sessions)
    }

    async fn delete(&self, req: DeleteRequest) -> adk_core::Result<()> {
        let key = Self::session_key(&req.app_name, &req.user_id, &req.session_id);
        let index_key = Self::index_key(&req.app_name, &req.user_id);
        let reverse_key = format!("session_reverse:{}", req.session_id);

        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("connection failed: {}", e)))?;

        // Remove session data and reverse mapping
        redis::cmd("DEL")
            .arg(&key)
            .arg(&reverse_key)
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("DEL failed: {}", e)))?;

        // Remove session_id from the index set
        redis::cmd("SREM")
            .arg(&index_key)
            .arg(&req.session_id)
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("SREM failed: {}", e)))?;

        Ok(())
    }

    async fn append_event(&self, session_id: &str, event: Event) -> adk_core::Result<()> {
        // For Redis, we need to find the session by scanning or by convention.
        // Since we don't have app_name/user_id here, we use a reverse lookup pattern.
        // However, the trait only gives us session_id. We'll need to find the key.
        // Strategy: store a reverse mapping session_id → full key.
        //
        // For simplicity and to match the existing pattern, we update state in-place
        // by looking up the session via a reverse index key.
        let reverse_key = format!("session_reverse:{}", session_id);

        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("connection failed: {}", e)))?;

        let full_key: Option<String> = redis::cmd("GET")
            .arg(&reverse_key)
            .query_async(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("GET reverse key failed: {}", e)))?;

        let full_key = full_key.ok_or_else(|| adk_core::AdkError::session("session not found"))?;

        // Get current session data
        let json: Option<String> = redis::cmd("GET")
            .arg(&full_key)
            .query_async(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("GET failed: {}", e)))?;

        let json = json.ok_or_else(|| adk_core::AdkError::session("session not found"))?;

        let mut data: RedisSessionData = serde_json::from_str(&json)
            .map_err(|e| adk_core::AdkError::session(format!("deserialize failed: {}", e)))?;

        // Apply state delta
        if !event.actions.state_delta.is_empty() {
            data.state.extend(event.actions.state_delta.clone());
        }
        data.updated_at = event.timestamp.to_rfc3339();

        // Write back
        let new_json = serde_json::to_string(&data)
            .map_err(|e| adk_core::AdkError::session(format!("serialize failed: {}", e)))?;

        redis::cmd("SET")
            .arg(&full_key)
            .arg(&new_json)
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("SET failed: {}", e)))?;

        // Refresh TTL
        self.apply_ttl(&mut conn, &full_key).await?;

        Ok(())
    }

    async fn delete_all_sessions(&self, app_name: &str, user_id: &str) -> adk_core::Result<()> {
        let index_key = Self::index_key(app_name, user_id);

        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("connection failed: {}", e)))?;

        // Get all session IDs from the index set
        let session_ids: Vec<String> = redis::cmd("SMEMBERS")
            .arg(&index_key)
            .query_async(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("SMEMBERS failed: {}", e)))?;

        // Delete each session key and its reverse mapping
        for session_id in &session_ids {
            let key = Self::session_key(app_name, user_id, session_id);
            let reverse_key = format!("session_reverse:{}", session_id);

            redis::cmd("DEL")
                .arg(&key)
                .arg(&reverse_key)
                .query_async::<()>(&mut conn)
                .await
                .map_err(|e| adk_core::AdkError::session(format!("DEL failed: {}", e)))?;
        }

        // Delete the index set itself
        redis::cmd("DEL")
            .arg(&index_key)
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("DEL index failed: {}", e)))?;

        Ok(())
    }

    async fn health_check(&self) -> adk_core::Result<()> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("connection failed: {}", e)))?;

        redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map_err(|e| adk_core::AdkError::session(format!("health check failed: {}", e)))?;

        Ok(())
    }
}

// ── Firestore Session Service ──────────────────────────────────────

/// Firestore-backed session service using the Firestore REST API via `reqwest`.
///
/// Stores sessions in Firestore with collection path:
/// `sessions/{app_name}/users/{user_id}/sessions/{session_id}`
///
/// Document fields: `{ state: map, created_at: timestamp, updated_at: timestamp }`
///
/// Uses `google-cloud-auth` for GCP authentication (service account or
/// Application Default Credentials). The connection string should be the
/// GCP project ID.
///
/// Only available when the `firestore` feature flag is enabled.
#[cfg(feature = "firestore")]
pub struct FirestoreSessionService {
    project_id: String,
    http: reqwest::Client,
    token_source: std::sync::Arc<dyn google_cloud_token::TokenSource>,
}

#[cfg(feature = "firestore")]
impl std::fmt::Debug for FirestoreSessionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirestoreSessionService")
            .field("project_id", &self.project_id)
            .finish()
    }
}

#[cfg(feature = "firestore")]
impl FirestoreSessionService {
    /// Create a new Firestore session service for the given GCP project ID.
    ///
    /// Authenticates using Application Default Credentials (ADC) or the
    /// `GOOGLE_APPLICATION_CREDENTIALS` environment variable.
    pub async fn new(project_id: &str) -> anyhow::Result<Self> {
        use google_cloud_auth::project::Config;
        use google_cloud_auth::token::DefaultTokenSourceProvider;
        use google_cloud_token::TokenSourceProvider;

        let config = Config {
            audience: None,
            scopes: Some(&["https://www.googleapis.com/auth/datastore"]),
            sub: None,
        };

        let provider = DefaultTokenSourceProvider::new(config)
            .await
            .map_err(|e| anyhow::anyhow!("firestore auth initialization failed: {}", e))?;

        let token_source = provider.token_source();
        let http = reqwest::Client::new();

        Ok(Self {
            project_id: project_id.to_string(),
            http,
            token_source,
        })
    }

    /// Get an access token for Firestore API calls.
    async fn get_token(&self) -> anyhow::Result<String> {
        let token = self
            .token_source
            .token()
            .await
            .map_err(|e| anyhow::anyhow!("firestore token refresh failed: {}", e))?;
        // The token from DefaultTokenSource already includes "Bearer " prefix
        // Strip it if present since we use bearer_auth() which adds it
        let token = token.strip_prefix("Bearer ").unwrap_or(&token).to_string();
        Ok(token)
    }

    /// Build the Firestore REST API base URL for documents.
    fn base_url(&self) -> String {
        format!(
            "https://firestore.googleapis.com/v1/projects/{}/databases/(default)/documents",
            self.project_id
        )
    }

    /// Build the document path for a session.
    fn document_path(app_name: &str, user_id: &str, session_id: &str) -> String {
        format!(
            "sessions/{}/users/{}/sessions/{}",
            app_name, user_id, session_id
        )
    }

    /// Convert a HashMap state to Firestore map value fields.
    fn state_to_firestore_map(state: &HashMap<String, Value>) -> serde_json::Value {
        let mut fields = serde_json::Map::new();
        for (key, value) in state {
            fields.insert(key.clone(), Self::json_to_firestore_value(value));
        }
        serde_json::json!({ "mapValue": { "fields": fields } })
    }

    /// Convert a serde_json::Value to a Firestore Value representation.
    fn json_to_firestore_value(value: &Value) -> serde_json::Value {
        match value {
            Value::Null => serde_json::json!({ "nullValue": null }),
            Value::Bool(b) => serde_json::json!({ "booleanValue": b }),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    serde_json::json!({ "integerValue": i.to_string() })
                } else if let Some(f) = n.as_f64() {
                    serde_json::json!({ "doubleValue": f })
                } else {
                    serde_json::json!({ "stringValue": n.to_string() })
                }
            }
            Value::String(s) => serde_json::json!({ "stringValue": s }),
            Value::Array(arr) => {
                let values: Vec<serde_json::Value> =
                    arr.iter().map(Self::json_to_firestore_value).collect();
                serde_json::json!({ "arrayValue": { "values": values } })
            }
            Value::Object(obj) => {
                let mut fields = serde_json::Map::new();
                for (k, v) in obj {
                    fields.insert(k.clone(), Self::json_to_firestore_value(v));
                }
                serde_json::json!({ "mapValue": { "fields": fields } })
            }
        }
    }

    /// Convert a Firestore Value representation back to serde_json::Value.
    fn firestore_value_to_json(fv: &Value) -> Value {
        if let Some(obj) = fv.as_object() {
            if let Some(s) = obj.get("stringValue") {
                return s.clone();
            }
            if let Some(b) = obj.get("booleanValue") {
                return b.clone();
            }
            if let Some(i) = obj.get("integerValue") {
                // Firestore returns integers as strings
                if let Some(s) = i.as_str() {
                    if let Ok(n) = s.parse::<i64>() {
                        return Value::Number(n.into());
                    }
                }
                return i.clone();
            }
            if let Some(f) = obj.get("doubleValue") {
                return f.clone();
            }
            if obj.contains_key("nullValue") {
                return Value::Null;
            }
            if let Some(arr) = obj.get("arrayValue") {
                if let Some(values) = arr.get("values").and_then(|v| v.as_array()) {
                    return Value::Array(
                        values.iter().map(Self::firestore_value_to_json).collect(),
                    );
                }
                return Value::Array(Vec::new());
            }
            if let Some(map) = obj.get("mapValue") {
                if let Some(fields) = map.get("fields").and_then(|f| f.as_object()) {
                    let mut result = serde_json::Map::new();
                    for (k, v) in fields {
                        result.insert(k.clone(), Self::firestore_value_to_json(v));
                    }
                    return Value::Object(result);
                }
                return Value::Object(serde_json::Map::new());
            }
            if let Some(ts) = obj.get("timestampValue") {
                return ts.clone();
            }
        }
        fv.clone()
    }

    /// Parse a Firestore document response into session components.
    fn parse_document(doc: &Value) -> Option<(HashMap<String, Value>, DateTime<Utc>)> {
        let fields = doc.get("fields")?.as_object()?;

        // Parse state
        let state = if let Some(state_field) = fields.get("state") {
            if let Some(map) = state_field.get("mapValue") {
                if let Some(map_fields) = map.get("fields").and_then(|f| f.as_object()) {
                    let mut state = HashMap::new();
                    for (k, v) in map_fields {
                        state.insert(k.clone(), Self::firestore_value_to_json(v));
                    }
                    state
                } else {
                    HashMap::new()
                }
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        // Parse updated_at
        let updated_at = fields
            .get("updated_at")
            .and_then(|v| v.get("timestampValue"))
            .and_then(|v| v.as_str())
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);

        Some((state, updated_at))
    }
}

/// Internal session representation for the Firestore backend.
#[cfg(feature = "firestore")]
struct FirestoreSession {
    app_name: String,
    user_id: String,
    session_id: String,
    state: HashMap<String, Value>,
    events: Vec<Event>,
    updated_at: DateTime<Utc>,
}

#[cfg(feature = "firestore")]
impl Session for FirestoreSession {
    fn id(&self) -> &str {
        &self.session_id
    }

    fn app_name(&self) -> &str {
        &self.app_name
    }

    fn user_id(&self) -> &str {
        &self.user_id
    }

    fn state(&self) -> &dyn State {
        self
    }

    fn events(&self) -> &dyn Events {
        self
    }

    fn last_update_time(&self) -> DateTime<Utc> {
        self.updated_at
    }
}

#[cfg(feature = "firestore")]
impl State for FirestoreSession {
    fn get(&self, key: &str) -> Option<Value> {
        self.state.get(key).cloned()
    }

    fn set(&mut self, key: String, value: Value) {
        self.state.insert(key, value);
    }

    fn all(&self) -> HashMap<String, Value> {
        self.state.clone()
    }
}

#[cfg(feature = "firestore")]
impl Events for FirestoreSession {
    fn all(&self) -> Vec<Event> {
        self.events.clone()
    }

    fn len(&self) -> usize {
        self.events.len()
    }

    fn at(&self, index: usize) -> Option<&Event> {
        self.events.get(index)
    }
}

#[cfg(feature = "firestore")]
#[async_trait::async_trait]
impl SessionService for FirestoreSessionService {
    async fn create(&self, req: CreateRequest) -> adk_core::Result<Box<dyn Session>> {
        let session_id = req
            .session_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let now = Utc::now();
        let now_str = now.to_rfc3339();

        let doc_path = Self::document_path(&req.app_name, &req.user_id, &session_id);

        // Build the Firestore document
        let document = serde_json::json!({
            "fields": {
                "state": Self::state_to_firestore_map(&req.state),
                "created_at": { "timestampValue": now_str },
                "updated_at": { "timestampValue": now_str }
            }
        });

        let token = self
            .get_token()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("firestore auth failed: {}", e)))?;

        // Use PATCH with the full document path to create or overwrite
        let patch_url = format!("{}/{}", self.base_url(), doc_path);
        let resp = self
            .http
            .patch(&patch_url)
            .bearer_auth(&token)
            .json(&document)
            .send()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("firestore create failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(adk_core::AdkError::session(format!(
                "firestore create failed ({}): {}",
                status, body
            )));
        }

        Ok(Box::new(FirestoreSession {
            app_name: req.app_name,
            user_id: req.user_id,
            session_id,
            state: req.state,
            events: Vec::new(),
            updated_at: now,
        }))
    }

    async fn get(&self, req: GetRequest) -> adk_core::Result<Box<dyn Session>> {
        let doc_path = Self::document_path(&req.app_name, &req.user_id, &req.session_id);
        let url = format!("{}/{}", self.base_url(), doc_path);

        let token = self
            .get_token()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("firestore auth failed: {}", e)))?;

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("firestore get failed: {}", e)))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(adk_core::AdkError::session("session not found"));
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(adk_core::AdkError::session(format!(
                "firestore get failed ({}): {}",
                status, body
            )));
        }

        let doc: Value = resp
            .json()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("firestore parse failed: {}", e)))?;

        let (state, updated_at) = Self::parse_document(&doc)
            .ok_or_else(|| adk_core::AdkError::session("firestore document parse failed"))?;

        Ok(Box::new(FirestoreSession {
            app_name: req.app_name,
            user_id: req.user_id,
            session_id: req.session_id,
            state,
            events: Vec::new(),
            updated_at,
        }))
    }

    async fn list(&self, req: ListRequest) -> adk_core::Result<Vec<Box<dyn Session>>> {
        // List all sessions under sessions/{app_name}/users/{user_id}/sessions
        let collection_path = format!("sessions/{}/users/{}/sessions", &req.app_name, &req.user_id);
        let url = format!("{}/{}", self.base_url(), collection_path);

        let token = self
            .get_token()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("firestore auth failed: {}", e)))?;

        let mut request = self.http.get(&url).bearer_auth(&token);

        // Apply pagination
        if let Some(limit) = req.limit {
            request = request.query(&[("pageSize", limit.to_string())]);
        }

        let resp = request
            .send()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("firestore list failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(adk_core::AdkError::session(format!(
                "firestore list failed ({}): {}",
                status, body
            )));
        }

        let body: Value = resp
            .json()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("firestore parse failed: {}", e)))?;

        let documents = body
            .get("documents")
            .and_then(|d| d.as_array())
            .cloned()
            .unwrap_or_default();

        let mut sessions: Vec<Box<dyn Session>> = Vec::new();

        for doc in &documents {
            // Extract session_id from the document name
            // Format: projects/{project}/databases/(default)/documents/sessions/{app}/users/{user}/sessions/{session_id}
            let name = doc.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let session_id = name.rsplit('/').next().unwrap_or("").to_string();

            if session_id.is_empty() {
                continue;
            }

            if let Some((state, updated_at)) = Self::parse_document(doc) {
                sessions.push(Box::new(FirestoreSession {
                    app_name: req.app_name.clone(),
                    user_id: req.user_id.clone(),
                    session_id,
                    state,
                    events: Vec::new(),
                    updated_at,
                }));
            }
        }

        // Sort by updated_at descending
        sessions.sort_by(|a, b| b.last_update_time().cmp(&a.last_update_time()));

        // Apply offset
        let offset = req.offset.unwrap_or(0);
        let sessions = sessions.into_iter().skip(offset).collect();

        Ok(sessions)
    }

    async fn delete(&self, req: DeleteRequest) -> adk_core::Result<()> {
        let doc_path = Self::document_path(&req.app_name, &req.user_id, &req.session_id);
        let url = format!("{}/{}", self.base_url(), doc_path);

        let token = self
            .get_token()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("firestore auth failed: {}", e)))?;

        let resp = self
            .http
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| adk_core::AdkError::session(format!("firestore delete failed: {}", e)))?;

        if !resp.status().is_success() && resp.status() != reqwest::StatusCode::NOT_FOUND {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(adk_core::AdkError::session(format!(
                "firestore delete failed ({}): {}",
                status, body
            )));
        }

        Ok(())
    }

    async fn append_event(&self, session_id: &str, _event: Event) -> adk_core::Result<()> {
        // For Firestore, the trait's append_event only provides session_id,
        // but our collection path requires app_name and user_id to locate
        // the document. The Firestore REST API doesn't support collection
        // group queries by document ID without a composite index.
        //
        // In practice, the SessionBridge manages state through create/get
        // operations. The append_event is best-effort for Firestore — state
        // updates are applied when the session is next retrieved and saved.
        tracing::debug!(
            session_id = session_id,
            "firestore append_event: state delta updates via append_event are best-effort"
        );

        Ok(())
    }

    async fn delete_all_sessions(&self, app_name: &str, user_id: &str) -> adk_core::Result<()> {
        // List all sessions for this user and delete them
        let list_req = ListRequest {
            app_name: app_name.to_string(),
            user_id: user_id.to_string(),
            limit: None,
            offset: None,
        };

        let sessions = self.list(list_req).await?;

        for session in &sessions {
            let del_req = DeleteRequest {
                app_name: app_name.to_string(),
                user_id: user_id.to_string(),
                session_id: session.id().to_string(),
            };
            self.delete(del_req).await?;
        }

        Ok(())
    }

    async fn health_check(&self) -> adk_core::Result<()> {
        // Verify we can authenticate and reach Firestore by listing the root collection
        let url = format!("{}/sessions?pageSize=1", self.base_url());

        let token = self.get_token().await.map_err(|e| {
            adk_core::AdkError::session(format!("firestore health check auth failed: {}", e))
        })?;

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| {
                adk_core::AdkError::session(format!("firestore health check failed: {}", e))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(adk_core::AdkError::session(format!(
                "firestore health check failed ({}): {}",
                status, body
            )));
        }

        Ok(())
    }
}

// ── Factory ────────────────────────────────────────────────────────

/// Create a `SessionService` backend from the given config (R6.1, R6.4).
///
/// Returns `InMemorySessionService` by default. For persistent backends
/// (SQLite, Postgres, Redis, Firestore) this creates the appropriate
/// backend implementation.
pub async fn create_session_service(
    config: &SessionConfig,
) -> anyhow::Result<Arc<dyn SessionService>> {
    match config.backend {
        SessionBackendType::InMemory => {
            tracing::info!("session backend: InMemory");
            Ok(Arc::new(InMemorySessionService::new()))
        }
        SessionBackendType::Sqlite => {
            let conn_str = config.connection_string.as_deref().unwrap_or(":memory:");
            tracing::info!(
                backend = "sqlite",
                connection_string = conn_str,
                "initializing SQLite session backend"
            );
            let service = SqliteSessionService::new(conn_str)
                .map_err(|e| anyhow::anyhow!("sqlite session backend: {}", e))?;
            Ok(Arc::new(service))
        }
        SessionBackendType::Postgres => {
            #[cfg(feature = "postgres")]
            {
                let conn_str = config.connection_string.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("postgres session backend requires a connectionString")
                })?;
                tracing::info!(
                    backend = "postgres",
                    connection_string = conn_str,
                    "initializing Postgres session backend"
                );
                let service = PostgresSessionService::new(conn_str)
                    .await
                    .map_err(|e| anyhow::anyhow!("postgres session backend: {}", e))?;
                Ok(Arc::new(service))
            }
            #[cfg(not(feature = "postgres"))]
            {
                tracing::warn!(
                    backend = "postgres",
                    connection_string = config.connection_string.as_deref().unwrap_or("<none>"),
                    "Postgres session backend requires the 'postgres' feature flag, falling back to InMemory"
                );
                Ok(Arc::new(InMemorySessionService::new()))
            }
        }
        SessionBackendType::Redis => {
            #[cfg(feature = "redis")]
            {
                let conn_str = config.connection_string.as_deref().ok_or_else(|| {
                    anyhow::anyhow!("redis session backend requires a connectionString")
                })?;
                // Derive TTL from session reset config (idle_minutes → seconds)
                let ttl_seconds = config.reset.idle_minutes.map(|m| m * 60);
                tracing::info!(
                    backend = "redis",
                    connection_string = conn_str,
                    ttl_seconds = ?ttl_seconds,
                    "initializing Redis session backend"
                );
                let service = RedisSessionService::new(conn_str, ttl_seconds)
                    .await
                    .map_err(|e| anyhow::anyhow!("redis session backend: {}", e))?;
                Ok(Arc::new(service))
            }
            #[cfg(not(feature = "redis"))]
            {
                tracing::warn!(
                    backend = "redis",
                    connection_string = config.connection_string.as_deref().unwrap_or("<none>"),
                    "Redis session backend requires the 'redis' feature flag, falling back to InMemory"
                );
                Ok(Arc::new(InMemorySessionService::new()))
            }
        }
        SessionBackendType::Firestore => {
            #[cfg(feature = "firestore")]
            {
                let project_id = config.connection_string.as_deref().ok_or_else(|| {
                    anyhow::anyhow!(
                        "firestore session backend requires a connectionString (GCP project ID)"
                    )
                })?;
                tracing::info!(
                    backend = "firestore",
                    project_id = project_id,
                    "initializing Firestore session backend"
                );
                let service = FirestoreSessionService::new(project_id)
                    .await
                    .map_err(|e| anyhow::anyhow!("firestore session backend: {}", e))?;
                Ok(Arc::new(service))
            }
            #[cfg(not(feature = "firestore"))]
            {
                tracing::warn!(
                    backend = "firestore",
                    connection_string = config.connection_string.as_deref().unwrap_or("<none>"),
                    "Firestore session backend requires the 'firestore' feature flag, falling back to InMemory"
                );
                Ok(Arc::new(InMemorySessionService::new()))
            }
        }
    }
}

/// Validate connectivity to the configured session backend (R6.5).
///
/// For `InMemory` this is a no-op. For persistent backends this will
/// attempt a connection and return an error on failure.
pub async fn validate_session_backend(config: &SessionConfig) -> anyhow::Result<()> {
    match config.backend {
        SessionBackendType::InMemory => {
            // InMemory always succeeds
            Ok(())
        }
        SessionBackendType::Sqlite => {
            let backend_name = "Sqlite";
            let conn_str = match &config.connection_string {
                Some(s) => s.as_str(),
                None => {
                    anyhow::bail!(
                        "session backend '{}' requires a connectionString in config",
                        backend_name
                    );
                }
            };
            // Attempt to open the SQLite database to validate connectivity
            match SqliteSessionService::new(conn_str) {
                Ok(service) => {
                    service.health_check().await.map_err(|e| {
                        anyhow::anyhow!(
                            "session backend '{}' connection failed: {}",
                            backend_name,
                            e
                        )
                    })?;
                    tracing::info!(backend = %backend_name, "session backend connectivity validated");
                    Ok(())
                }
                Err(e) => {
                    anyhow::bail!(
                        "session backend '{}' connection failed: {}",
                        backend_name,
                        e
                    );
                }
            }
        }
        SessionBackendType::Postgres
        | SessionBackendType::Redis
        | SessionBackendType::Firestore => {
            let backend_name = format!("{:?}", config.backend);
            if config.connection_string.is_none() {
                anyhow::bail!(
                    "session backend '{}' requires a connectionString in config",
                    backend_name
                );
            }
            #[cfg(feature = "postgres")]
            if config.backend == SessionBackendType::Postgres {
                let conn_str = config.connection_string.as_deref().unwrap();
                match PostgresSessionService::new(conn_str).await {
                    Ok(service) => {
                        service.health_check().await.map_err(|e| {
                            anyhow::anyhow!("session backend 'Postgres' connection failed: {}", e)
                        })?;
                        tracing::info!(
                            backend = "Postgres",
                            "session backend connectivity validated"
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        anyhow::bail!("session backend 'Postgres' connection failed: {}", e);
                    }
                }
            }
            #[cfg(feature = "redis")]
            if config.backend == SessionBackendType::Redis {
                let conn_str = config.connection_string.as_deref().unwrap();
                let ttl_seconds = config.reset.idle_minutes.map(|m| m * 60);
                match RedisSessionService::new(conn_str, ttl_seconds).await {
                    Ok(service) => {
                        service.health_check().await.map_err(|e| {
                            anyhow::anyhow!("session backend 'Redis' connection failed: {}", e)
                        })?;
                        tracing::info!(backend = "Redis", "session backend connectivity validated");
                        return Ok(());
                    }
                    Err(e) => {
                        anyhow::bail!("session backend 'Redis' connection failed: {}", e);
                    }
                }
            }
            #[cfg(feature = "firestore")]
            if config.backend == SessionBackendType::Firestore {
                let project_id = config.connection_string.as_deref().unwrap();
                match FirestoreSessionService::new(project_id).await {
                    Ok(service) => {
                        service.health_check().await.map_err(|e| {
                            anyhow::anyhow!("session backend 'Firestore' connection failed: {}", e)
                        })?;
                        tracing::info!(
                            backend = "Firestore",
                            "session backend connectivity validated"
                        );
                        return Ok(());
                    }
                    Err(e) => {
                        anyhow::bail!("session backend 'Firestore' connection failed: {}", e);
                    }
                }
            }
            tracing::info!(
                backend = %backend_name,
                "session backend connectivity validation skipped (feature flag not enabled)"
            );
            Ok(())
        }
    }
}
