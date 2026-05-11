//! Property-based tests for persistent session backend correctness.
//!
//! Feature: full-stack-completion
//! - Property 1: Session persistence round-trip
//!   **Validates: Requirements 1.1, 1.7**
//! - Property 2: Connection failure error reporting
//!   **Validates: Requirements 1.5**

use adk_gateway::config::{SessionBackendType, SessionConfig, SessionResetConfig};
use adk_gateway::session_bridge::{validate_session_backend, SqliteSessionService};
use adk_session::{CreateRequest, GetRequest, SessionService};
use proptest::prelude::*;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

// ── Strategies ─────────────────────────────────────────────────────

/// Generate an arbitrary JSON value (limited depth to avoid stack overflow).
fn arb_json_value() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|n| Value::Number(serde_json::Number::from(n))),
        "[a-zA-Z0-9_ ]{0,50}".prop_map(|s| Value::String(s)),
    ]
}

/// Generate an arbitrary session state as a HashMap<String, Value>.
fn arb_session_state() -> impl Strategy<Value = HashMap<String, Value>> {
    prop::collection::hash_map("[a-zA-Z_][a-zA-Z0-9_]{0,15}", arb_json_value(), 0..10)
}

/// Generate an arbitrary app name.
fn arb_app_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{1,20}"
}

/// Generate an arbitrary user ID.
fn arb_user_id() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_:.-]{1,30}"
}

/// Generate an arbitrary session ID.
fn arb_session_id() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9-]{1,30}"
}

/// Generate an invalid connection string for testing error reporting.
/// For SQLite, the path must have a non-existent parent directory to trigger
/// a connection failure (SQLite will happily create files in existing dirs).
fn arb_invalid_connection_string() -> impl Strategy<Value = String> {
    prop_oneof![
        // Non-existent deep directory paths (parent dir doesn't exist)
        "/nonexistent_[a-z]{3,8}/deep_[a-z]{3,8}/nested_[a-z]{3,8}/sessions\\.db".prop_map(|s| s),
        // Paths under /proc which are not writable
        "/proc/nonexistent_[a-z]{3,8}/sessions\\.db".prop_map(|s| s),
        // Paths under /sys which are not writable
        "/sys/nonexistent_[a-z]{3,8}/sessions\\.db".prop_map(|s| s),
    ]
}

// ── Property 1: Session persistence round-trip ─────────────────────
// Feature: full-stack-completion, Property 1: Session persistence round-trip
// **Validates: Requirements 1.1, 1.7**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn session_persistence_round_trip_sqlite(
        app_name in arb_app_name(),
        user_id in arb_user_id(),
        session_id in arb_session_id(),
        state in arb_session_state(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            // Create an in-memory SQLite session service
            let service = SqliteSessionService::new(":memory:")
                .expect("should create in-memory SQLite session service");
            let service: Arc<dyn SessionService> = Arc::new(service);

            // Store a session with the arbitrary state
            let create_req = CreateRequest {
                app_name: app_name.clone(),
                user_id: user_id.clone(),
                session_id: Some(session_id.clone()),
                state: state.clone(),
            };

            let created = service.create(create_req).await
                .expect("create session should succeed");

            // Verify created session has correct metadata
            prop_assert_eq!(created.id(), session_id.as_str());
            prop_assert_eq!(created.app_name(), app_name.as_str());
            prop_assert_eq!(created.user_id(), user_id.as_str());

            // Retrieve the session
            let get_req = GetRequest {
                app_name: app_name.clone(),
                user_id: user_id.clone(),
                session_id: session_id.clone(),
                num_recent_events: None,
                after: None,
            };

            let retrieved = service.get(get_req).await
                .expect("get session should succeed");

            // Verify round-trip: retrieved state matches original
            let retrieved_state = retrieved.state().all();

            prop_assert_eq!(
                &retrieved_state, &state,
                "Retrieved state should match stored state.\n\
                 Expected: {:?}\n\
                 Got: {:?}",
                state, retrieved_state
            );

            // Verify metadata is preserved
            prop_assert_eq!(
                retrieved.id(), session_id.as_str(),
                "session_id should be preserved"
            );
            prop_assert_eq!(
                retrieved.app_name(), app_name.as_str(),
                "app_name should be preserved"
            );
            prop_assert_eq!(
                retrieved.user_id(), user_id.as_str(),
                "user_id should be preserved"
            );

            Ok(())
        })?;
    }
}

// ── Property 2: Connection failure error reporting ─────────────────
// Feature: full-stack-completion, Property 2: Connection failure error reporting
// **Validates: Requirements 1.5**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn connection_failure_error_reporting(
        invalid_conn_str in arb_invalid_connection_string(),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let config = SessionConfig {
                dm_scope: "per-channel-peer".to_string(),
                reset: SessionResetConfig {
                    mode: "idle".to_string(),
                    at_hour: None,
                    idle_minutes: Some(120),
                },
                backend: SessionBackendType::Sqlite,
                connection_string: Some(invalid_conn_str.clone()),
            };

            let result = validate_session_backend(&config).await;

            // The validation should fail for invalid connection strings
            prop_assert!(
                result.is_err(),
                "validate_session_backend should fail for invalid connection string: '{}'",
                invalid_conn_str
            );

            // The error message should contain the backend name
            let err_msg = result.unwrap_err().to_string();
            prop_assert!(
                err_msg.contains("Sqlite") || err_msg.contains("sqlite"),
                "Error message should contain the backend name 'Sqlite' or 'sqlite'.\n\
                 Got: '{}'",
                err_msg
            );

            // The error message should contain a failure description (not be empty beyond the backend name)
            prop_assert!(
                err_msg.len() > 10,
                "Error message should contain a meaningful failure description.\n\
                 Got: '{}'",
                err_msg
            );

            Ok(())
        })?;
    }
}
