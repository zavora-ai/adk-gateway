//! Authentication guard middleware and session cookie logic.
//!
//! When auth mode is "password" or "token", all /ui/api/* routes
//! (except /ui/api/login and /ui/api/auth/check) require a valid session
//! cookie. Unauthenticated API requests get a 401 JSON error.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::ControlPanelState;
use crate::config::AuthMode;

/// Cookie name for UI sessions.
const COOKIE_NAME: &str = "adk_ui_session";

/// Maximum session lifetime (24 hours).
const SESSION_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// Represents an active UI session.
#[derive(Debug, Clone)]
pub struct UiSession {
    pub token: String,
    pub created_at: Instant,
}

// ── Auth guard middleware ───────────────────────────────────────────

/// Axum middleware that checks for a valid session cookie on protected routes.
///
/// - If auth mode is "none" or auth section is absent → pass through
/// - If route is /ui/api/login or /ui/api/auth/check → pass through (always accessible)
/// - If valid `adk_ui_session` cookie present → pass through
/// - Otherwise → return 401 JSON
pub async fn auth_guard(
    State(state): State<Arc<ControlPanelState>>,
    request: Request,
    next: Next,
) -> Response {
    let config = state.config.load();

    // Check if auth is required
    let auth_required = config.auth.as_ref().map_or(false, |auth| {
        matches!(auth.mode, AuthMode::Password | AuthMode::Token)
            && (auth.password.is_some() || auth.token.is_some())
    });

    if !auth_required {
        return next.run(request).await;
    }

    // Always allow public API routes through
    let path = request.uri().path();
    if path == "/ui/api/login" || path == "/ui/api/auth/check" {
        return next.run(request).await;
    }

    // Check for valid session cookie
    let cookie_header = request
        .headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let session_token = extract_cookie(cookie_header, COOKIE_NAME);

    if let Some(token) = session_token {
        if let Some(session) = state.ui_sessions.get(token) {
            // Validate session: check token matches and session hasn't expired
            if session.token == token && session.created_at.elapsed() < SESSION_MAX_AGE {
                return next.run(request).await;
            }
            // Session expired — remove it from the map
            drop(session);
            state.ui_sessions.remove(token);
        }
    }

    // Unauthenticated — return 401 JSON for all routes
    (
        StatusCode::UNAUTHORIZED,
        axum::Json(serde_json::json!({
            "ok": false,
            "message": "Authentication required"
        })),
    )
        .into_response()
}

// ── Cookie parsing ─────────────────────────────────────────────────

/// Extract a cookie value by name from a Cookie header string.
fn extract_cookie<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        if let Some(value) = pair.strip_prefix(name) {
            if let Some(value) = value.strip_prefix('=') {
                return Some(value);
            }
        }
    }
    None
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_cookie_found() {
        assert_eq!(
            extract_cookie("adk_ui_session=abc123; other=xyz", "adk_ui_session"),
            Some("abc123")
        );
    }

    #[test]
    fn test_extract_cookie_not_found() {
        assert_eq!(extract_cookie("other=xyz", "adk_ui_session"), None);
    }

    #[test]
    fn test_extract_cookie_empty() {
        assert_eq!(extract_cookie("", "adk_ui_session"), None);
    }

    #[test]
    fn test_extract_cookie_with_spaces() {
        // After trim(), the trailing space before `;` is removed
        assert_eq!(
            extract_cookie("  adk_ui_session=token123 ; foo=bar", "adk_ui_session"),
            Some("token123")
        );
    }

    #[test]
    fn test_extract_cookie_partial_name_no_match() {
        // "adk_ui_session_extra" should not match "adk_ui_session"
        assert_eq!(
            extract_cookie("adk_ui_session_extra=val", "adk_ui_session"),
            None
        );
    }
}
