//! Phase 2 API endpoints for the control panel (Tasks 15–22).
//!
//! Provides endpoints for:
//! - Stale context configuration (Task 15)
//! - Rate limiter configuration and metrics (Task 16)
//! - ACP agent management (Task 17)
//! - Health monitor (Task 18)
//! - Multi-user management (Task 19)
//! - Config encryption (Task 20)
//! - Log rotation and storage (Task 21)
//! - Deployment / system info (Task 22)

use super::ControlPanelState;
use std::sync::Arc;

// ── Task 15: Stale Context ─────────────────────────────────────────

/// GET /ui/api/settings/stale-context — get stale context configuration.
pub(crate) async fn get_stale_context_config(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let idle_hours = config.stale_context.idle_threshold_secs as f64 / 3600.0;

    axum::Json(serde_json::json!({
        "ok": true,
        "data": {
            "idle_threshold_hours": idle_hours,
            "idle_threshold_secs": config.stale_context.idle_threshold_secs,
        }
    }))
}

/// POST /ui/api/settings/stale-context — save stale context configuration.
pub(crate) async fn save_stale_context_config(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    let idle_hours = payload
        .get("idle_threshold_hours")
        .and_then(|v| v.as_f64())
        .unwrap_or(4.0);

    let idle_secs = (idle_hours * 3600.0) as u64;

    let mut cfg = state.config.load().as_ref().clone();
    cfg.stale_context.idle_threshold_secs = idle_secs;

    let output = match serde_json::to_string_pretty(&cfg) {
        Ok(s) => s,
        Err(e) => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to serialize config: {e}")
            }));
        }
    };

    if let Err(e) = std::fs::write(&config_path, &output) {
        return axum::Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to write config: {e}")
        }));
    }

    state.config.store(Arc::new(cfg));
    tracing::info!(idle_threshold_secs = idle_secs, "Stale context config saved");

    axum::Json(serde_json::json!({
        "ok": true,
        "message": "Stale context configuration saved."
    }))
}

/// GET /ui/api/users/activity — get user activity data (placeholder).
pub(crate) async fn get_user_activities(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!([]))
}

// ── Task 16: Rate Limiter ──────────────────────────────────────────

/// GET /ui/api/settings/rate-limit — get rate limiter configuration.
pub(crate) async fn get_rate_limit_config(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let rl = &config.rate_limiter;

    axum::Json(serde_json::json!({
        "ok": true,
        "data": {
            "max_calls": rl.max_calls,
            "window_secs": rl.window_secs,
            "cooldown_secs": rl.cooldown_secs,
            "max_triggers": rl.max_triggers,
        }
    }))
}

/// POST /ui/api/settings/rate-limit — save rate limiter configuration.
pub(crate) async fn save_rate_limit_config(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    let max_calls = payload.get("max_calls").and_then(|v| v.as_u64()).unwrap_or(10) as u32;
    let window_secs = payload.get("window_secs").and_then(|v| v.as_u64()).unwrap_or(5);
    let cooldown_secs = payload.get("cooldown_secs").and_then(|v| v.as_u64()).unwrap_or(3);
    let max_triggers = payload.get("max_triggers").and_then(|v| v.as_u64()).unwrap_or(3) as u32;

    let mut cfg = state.config.load().as_ref().clone();
    cfg.rate_limiter.max_calls = max_calls;
    cfg.rate_limiter.window_secs = window_secs;
    cfg.rate_limiter.cooldown_secs = cooldown_secs;
    cfg.rate_limiter.max_triggers = max_triggers;

    let output = match serde_json::to_string_pretty(&cfg) {
        Ok(s) => s,
        Err(e) => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to serialize config: {e}")
            }));
        }
    };

    if let Err(e) = std::fs::write(&config_path, &output) {
        return axum::Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to write config: {e}")
        }));
    }

    state.config.store(Arc::new(cfg));
    tracing::info!("Rate limiter config saved via control panel");

    axum::Json(serde_json::json!({
        "ok": true,
        "message": "Rate limiter configuration saved."
    }))
}

/// GET /ui/api/settings/rate-limit/metrics — get rate limit metrics (placeholder).
pub(crate) async fn get_rate_limit_metrics(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({"triggered_today": 0}))
}

// ── Task 17: ACP ───────────────────────────────────────────────────

/// GET /ui/api/acp/agents — list ACP agents (placeholder).
pub(crate) async fn get_acp_agents(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!([]))
}

/// POST /ui/api/acp/agents — add an ACP agent (placeholder).
pub(crate) async fn add_acp_agent(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(_payload): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "ok": true,
        "message": "ACP agent added (placeholder)."
    }))
}

/// DELETE /ui/api/acp/agents/{id} — remove an ACP agent (placeholder).
pub(crate) async fn remove_acp_agent(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
    axum::extract::Path(_id): axum::extract::Path<String>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "ok": true,
        "message": "ACP agent removed (placeholder)."
    }))
}

/// GET /ui/api/acp/enabled — check if ACP feature is enabled.
pub(crate) async fn get_acp_feature_enabled(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({"enabled": cfg!(feature = "acp")}))
}

// ── Task 18: Health Monitor ────────────────────────────────────────

/// GET /ui/api/health/components — list health-monitored components (placeholder).
pub(crate) async fn get_health_components(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!([]))
}

/// GET /ui/api/health/events — list health events (placeholder).
pub(crate) async fn get_health_events(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!([]))
}

/// GET /ui/api/settings/health-monitor — get health monitor configuration.
pub(crate) async fn get_health_monitor_config(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let hm = &config.health_monitor;

    axum::Json(serde_json::json!({
        "ok": true,
        "data": {
            "check_interval_secs": hm.check_interval_secs,
            "failure_threshold": hm.failure_threshold,
            "alert_webhook_url": hm.alert_webhook_url,
            "alert_telegram_admin": hm.alert_telegram_admin,
        }
    }))
}

/// POST /ui/api/settings/health-monitor — save health monitor configuration.
pub(crate) async fn save_health_monitor_config(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    let check_interval_secs = payload
        .get("check_interval_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(60);
    let failure_threshold = payload
        .get("failure_threshold")
        .and_then(|v| v.as_u64())
        .unwrap_or(3) as u32;
    let alert_webhook_url = payload
        .get("alert_webhook_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);
    let alert_telegram_admin = payload
        .get("alert_telegram_admin")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);

    let mut cfg = state.config.load().as_ref().clone();
    cfg.health_monitor.check_interval_secs = check_interval_secs;
    cfg.health_monitor.failure_threshold = failure_threshold;
    cfg.health_monitor.alert_webhook_url = alert_webhook_url;
    cfg.health_monitor.alert_telegram_admin = alert_telegram_admin;

    let output = match serde_json::to_string_pretty(&cfg) {
        Ok(s) => s,
        Err(e) => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to serialize config: {e}")
            }));
        }
    };

    if let Err(e) = std::fs::write(&config_path, &output) {
        return axum::Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to write config: {e}")
        }));
    }

    state.config.store(Arc::new(cfg));
    tracing::info!("Health monitor config saved via control panel");

    axum::Json(serde_json::json!({
        "ok": true,
        "message": "Health monitor configuration saved."
    }))
}

// ── Task 19: Multi-User ────────────────────────────────────────────

/// GET /ui/api/users/paired — list paired users (placeholder).
pub(crate) async fn get_paired_users(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!([]))
}

/// POST /ui/api/users/{id}/unpair — unpair a user (placeholder).
pub(crate) async fn unpair_user(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
    axum::extract::Path(_id): axum::extract::Path<String>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "ok": true,
        "message": "User unpaired (placeholder)."
    }))
}

/// POST /ui/api/users/{id}/heartbeat — update user heartbeat (placeholder).
pub(crate) async fn update_user_heartbeat(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
    axum::extract::Path(_id): axum::extract::Path<String>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "ok": true,
        "message": "Heartbeat updated (placeholder)."
    }))
}

/// GET /ui/api/users/groups — get group assignments (placeholder).
pub(crate) async fn get_group_assignments(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!([]))
}

/// POST /ui/api/users/groups — save a group assignment (placeholder).
pub(crate) async fn save_group_assignment(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(_payload): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "ok": true,
        "message": "Group assignment saved (placeholder)."
    }))
}

// ── Task 20: Config Encryption ─────────────────────────────────────

/// GET /ui/api/encryption/status — check encryption status.
pub(crate) async fn get_encryption_status(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let has_key = config.gateway.encryption.is_some();
    let key_path = config
        .gateway
        .encryption
        .as_ref()
        .map(|e| e.key_file.display().to_string());

    axum::Json(serde_json::json!({
        "ok": true,
        "data": {
            "encryption_configured": has_key,
            "key_path": key_path,
        }
    }))
}

/// GET /ui/api/encryption/sensitive-fields — list sensitive fields in config.
pub(crate) async fn get_sensitive_fields(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let config_json = match serde_json::to_value(config.as_ref()) {
        Ok(v) => v,
        Err(e) => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to serialize config: {e}")
            }));
        }
    };

    let fields = collect_sensitive_fields(&config_json, "");

    axum::Json(serde_json::json!({
        "ok": true,
        "data": fields,
    }))
}

/// POST /ui/api/encryption/encrypt-all — encrypt all sensitive fields.
pub(crate) async fn encrypt_all(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let key_file = match &config.gateway.encryption {
        Some(enc) => enc.key_file.clone(),
        None => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": "No encryption key configured. Set gateway.encryption.keyFile first."
            }));
        }
    };

    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    match crate::config_encryption::encrypt_config_file(&config_path, &key_file) {
        Ok(count) => axum::Json(serde_json::json!({
            "ok": true,
            "message": format!("Encrypted {count} sensitive field(s)."),
            "encrypted_count": count,
        })),
        Err(e) => axum::Json(serde_json::json!({
            "ok": false,
            "message": format!("Encryption failed: {e}")
        })),
    }
}

/// POST /ui/api/encryption/key-path — save encryption key path to config.
pub(crate) async fn save_encryption_key_path(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    let key_path = match payload.get("key_path").and_then(|v| v.as_str()) {
        Some(p) => p.to_string(),
        None => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": "Missing 'key_path' field."
            }));
        }
    };

    let mut cfg = state.config.load().as_ref().clone();
    cfg.gateway.encryption = Some(crate::config::EncryptionConfig {
        key_file: std::path::PathBuf::from(&key_path),
    });

    let output = match serde_json::to_string_pretty(&cfg) {
        Ok(s) => s,
        Err(e) => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to serialize config: {e}")
            }));
        }
    };

    if let Err(e) = std::fs::write(&config_path, &output) {
        return axum::Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to write config: {e}")
        }));
    }

    state.config.store(Arc::new(cfg));
    tracing::info!(key_path = %key_path, "Encryption key path saved");

    axum::Json(serde_json::json!({
        "ok": true,
        "message": "Encryption key path saved."
    }))
}

/// Recursively collect sensitive field paths from a JSON value.
fn collect_sensitive_fields(value: &serde_json::Value, prefix: &str) -> Vec<serde_json::Value> {
    let mut results = Vec::new();
    if let serde_json::Value::Object(map) = value {
        for (key, val) in map {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            if let serde_json::Value::String(s) = val {
                if crate::config_encryption::ConfigEncryption::is_sensitive_field(key) {
                    results.push(serde_json::json!({
                        "path": path,
                        "encrypted": crate::config_encryption::ConfigEncryption::is_encrypted(s),
                    }));
                }
            } else {
                results.extend(collect_sensitive_fields(val, &path));
            }
        }
    }
    results
}

// ── Task 21: Log Rotation ──────────────────────────────────────────

/// GET /ui/api/settings/log-rotation — get log rotation configuration.
pub(crate) async fn get_log_rotation_config(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let lr = &config.telemetry.log_rotation;

    axum::Json(serde_json::json!({
        "ok": true,
        "data": {
            "rotation": format!("{:?}", lr.rotation).to_lowercase(),
            "retention_days": lr.retention_days,
            "max_file_size_mb": lr.max_file_size_mb,
            "log_dir": config.telemetry.log_dir,
        }
    }))
}

/// POST /ui/api/settings/log-rotation — save log rotation configuration.
pub(crate) async fn save_log_rotation_config(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    axum::Json(payload): axum::Json<serde_json::Value>,
) -> axum::Json<serde_json::Value> {
    let config_path = match &state.config_path {
        Some(p) => p.clone(),
        None => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": "Config file path not configured"
            }));
        }
    };

    let retention_days = payload
        .get("retention_days")
        .and_then(|v| v.as_u64())
        .unwrap_or(7) as u32;
    let max_file_size_mb = payload
        .get("max_file_size_mb")
        .and_then(|v| v.as_u64())
        .unwrap_or(100);

    let mut cfg = state.config.load().as_ref().clone();
    cfg.telemetry.log_rotation.retention_days = retention_days;
    cfg.telemetry.log_rotation.max_file_size_mb = max_file_size_mb;

    let output = match serde_json::to_string_pretty(&cfg) {
        Ok(s) => s,
        Err(e) => {
            return axum::Json(serde_json::json!({
                "ok": false,
                "message": format!("Failed to serialize config: {e}")
            }));
        }
    };

    if let Err(e) = std::fs::write(&config_path, &output) {
        return axum::Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to write config: {e}")
        }));
    }

    state.config.store(Arc::new(cfg));
    tracing::info!(retention_days, "Log rotation config saved");

    axum::Json(serde_json::json!({
        "ok": true,
        "message": "Log rotation configuration saved."
    }))
}

/// GET /ui/api/logs/storage — get log storage metrics.
pub(crate) async fn get_log_storage_metrics(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let log_dir = std::path::Path::new("logs");
    let (file_count, total_bytes) = scan_log_directory(log_dir);

    axum::Json(serde_json::json!({
        "ok": true,
        "data": {
            "file_count": file_count,
            "total_size_bytes": total_bytes,
            "total_size_mb": total_bytes as f64 / (1024.0 * 1024.0),
        }
    }))
}

/// GET /ui/api/logs/files — list log files with sizes.
pub(crate) async fn get_log_files(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let log_dir = std::path::Path::new("logs");
    let mut files = Vec::new();

    if let Ok(entries) = std::fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    files.push(serde_json::json!({
                        "name": entry.file_name().to_string_lossy(),
                        "size_bytes": meta.len(),
                    }));
                }
            }
        }
    }

    // Sort by name (which includes date) descending
    files.sort_by(|a, b| {
        let name_a = a["name"].as_str().unwrap_or("");
        let name_b = b["name"].as_str().unwrap_or("");
        name_b.cmp(name_a)
    });

    axum::Json(serde_json::json!(files))
}

/// GET /ui/api/logs/files/{filename} — download a log file.
pub(crate) async fn download_log_file(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
    axum::extract::Path(filename): axum::extract::Path<String>,
) -> axum::Json<serde_json::Value> {
    // Prevent path traversal
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return axum::Json(serde_json::json!({
            "ok": false,
            "message": "Invalid filename."
        }));
    }

    let path = std::path::Path::new("logs").join(&filename);
    match std::fs::read_to_string(&path) {
        Ok(contents) => axum::Json(serde_json::json!({
            "ok": true,
            "filename": filename,
            "contents": contents,
        })),
        Err(e) => axum::Json(serde_json::json!({
            "ok": false,
            "message": format!("Failed to read log file: {e}")
        })),
    }
}

/// POST /ui/api/logs/clear-old — delete log files beyond retention period.
pub(crate) async fn clear_old_logs(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let retention_days = config.telemetry.log_rotation.retention_days;
    let log_dir = std::path::Path::new("logs");

    let cutoff = chrono::Utc::now() - chrono::Duration::days(retention_days as i64);
    let mut deleted = 0u32;

    if let Ok(entries) = std::fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if !meta.is_file() {
                    continue;
                }
                // Use file modified time as proxy for log date
                if let Ok(modified) = meta.modified() {
                    let modified_dt: chrono::DateTime<chrono::Utc> = modified.into();
                    if modified_dt < cutoff {
                        if std::fs::remove_file(entry.path()).is_ok() {
                            deleted += 1;
                        }
                    }
                }
            }
        }
    }

    tracing::info!(deleted, retention_days, "Old log files cleared");

    axum::Json(serde_json::json!({
        "ok": true,
        "message": format!("Deleted {deleted} old log file(s)."),
        "deleted_count": deleted,
    }))
}

/// Scan a directory for file count and total size.
fn scan_log_directory(dir: &std::path::Path) -> (u32, u64) {
    let mut count = 0u32;
    let mut total = 0u64;

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    count += 1;
                    total += meta.len();
                }
            }
        }
    }

    (count, total)
}

// ── Task 22: Deployment / System Info ──────────────────────────────

/// GET /ui/api/system/info — get system information.
pub(crate) async fn get_system_info(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    let config = state.config.load();
    let uptime_secs = state.start_time.elapsed().as_secs();
    let config_path = state
        .config_path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    axum::Json(serde_json::json!({
        "ok": true,
        "data": {
            "version": env!("CARGO_PKG_VERSION"),
            "uptime_secs": uptime_secs,
            "config_path": config_path,
            "drain_timeout_secs": config.gateway.drain_timeout_secs,
            "features": {
                "acp": cfg!(feature = "acp"),
                "redis": cfg!(feature = "redis"),
                "firestore": cfg!(feature = "firestore"),
            }
        }
    }))
}

/// GET /ui/api/system/restart-status — get restart status.
pub(crate) async fn get_restart_status(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "restarting": false,
        "in_flight_requests": 0,
        "phase": "idle"
    }))
}

/// POST /ui/api/system/restart — trigger a graceful restart.
pub(crate) async fn trigger_restart(
    axum::extract::State(_state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    // The ShutdownCoordinator lives on the GatewayState, not ControlPanelState.
    // A full restart requires sending SIGUSR1 to the process on Unix.
    #[cfg(unix)]
    {
        // Safety: sending a signal to our own process is safe.
        let result = unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) };
        if result == 0 {
            tracing::info!("Graceful restart triggered via control panel (SIGUSR1)");
            return axum::Json(serde_json::json!({
                "ok": true,
                "message": "Graceful restart initiated."
            }));
        }
    }

    axum::Json(serde_json::json!({
        "ok": false,
        "message": "Restart not available on this platform."
    }))
}
