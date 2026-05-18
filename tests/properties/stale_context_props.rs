//! Property-based tests for Stale Context Detection Threshold.
//!
//! Feature: phase-2-complete, Property 2: Stale Context Detection Threshold
//! **Validates: Requirements 2.1, 2.3, 2.4, 2.5**

use adk_gateway::config::StaleContextConfig;
use adk_gateway::stale_context::{HeartbeatAlert, PendingTaskResult, StaleContextDetector};
use chrono::{Duration, Utc};
use proptest::prelude::*;

// ── Strategies ─────────────────────────────────────────────────────

/// Strategy for a valid StaleContextConfig with reasonable bounds.
fn arb_stale_context_config() -> impl Strategy<Value = StaleContextConfig> {
    (1u64..=86400u64).prop_map(|idle_threshold_secs| StaleContextConfig {
        idle_threshold_secs,
    })
}

/// Strategy for a timestamp offset in seconds (positive, representing time in the past).
fn arb_idle_seconds() -> impl Strategy<Value = i64> {
    0i64..=172800i64 // 0 to 48 hours
}

/// Strategy for generating pending tasks.
fn arb_pending_tasks() -> impl Strategy<Value = Vec<PendingTaskResult>> {
    proptest::collection::vec(
        "[a-zA-Z0-9 ]{1,50}".prop_map(|desc| PendingTaskResult { description: desc }),
        0..=5,
    )
}

/// Strategy for generating heartbeat alerts.
fn arb_heartbeat_alerts() -> impl Strategy<Value = Vec<HeartbeatAlert>> {
    proptest::collection::vec(
        "[a-zA-Z0-9 ]{1,50}".prop_map(|msg| HeartbeatAlert { message: msg }),
        0..=5,
    )
}

// ── Property Tests ─────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    // Feature: phase-2-complete, Property 2: Stale Context Detection Threshold
    // **Validates: Requirements 2.1**
    //
    // For any last-activity timestamp, current timestamp, and idle threshold,
    // the `is_stale` function SHALL return `true` if and only if
    // `(current - last_activity) > threshold`.
    #[test]
    fn is_stale_iff_idle_exceeds_threshold(
        config in arb_stale_context_config(),
        idle_secs in arb_idle_seconds(),
    ) {
        let detector = StaleContextDetector::new(config.clone());
        let now = Utc::now();
        let last_activity = now - Duration::seconds(idle_secs);

        let result = detector.is_stale(last_activity, now);
        let expected = idle_secs > config.idle_threshold_secs as i64;

        prop_assert_eq!(
            result, expected,
            "is_stale({} idle secs, {} threshold) = {} but expected {}",
            idle_secs, config.idle_threshold_secs, result, expected
        );
    }

    // Feature: phase-2-complete, Property 2: Stale Context Detection Threshold
    // **Validates: Requirements 2.1**
    //
    // Exactly at the threshold boundary, is_stale SHALL return false.
    // One second past the threshold, is_stale SHALL return true.
    #[test]
    fn is_stale_boundary_behavior(
        config in arb_stale_context_config(),
    ) {
        let detector = StaleContextDetector::new(config.clone());
        let now = Utc::now();

        // Exactly at threshold: NOT stale
        let at_threshold = now - Duration::seconds(config.idle_threshold_secs as i64);
        prop_assert!(
            !detector.is_stale(at_threshold, now),
            "Exactly at threshold ({} secs) should NOT be stale",
            config.idle_threshold_secs
        );

        // One second past threshold: IS stale
        let past_threshold = now - Duration::seconds(config.idle_threshold_secs as i64 + 1);
        prop_assert!(
            detector.is_stale(past_threshold, now),
            "One second past threshold ({} + 1 secs) should be stale",
            config.idle_threshold_secs
        );
    }

    // Feature: phase-2-complete, Property 2: Stale Context Detection Threshold
    // **Validates: Requirements 2.3**
    //
    // Custom idle threshold supported via config: any threshold value in [1, 86400]
    // should be respected by the detector.
    #[test]
    fn custom_threshold_respected(
        threshold_secs in 1u64..=86400u64,
        idle_secs in arb_idle_seconds(),
    ) {
        let config = StaleContextConfig { idle_threshold_secs: threshold_secs };
        let detector = StaleContextDetector::new(config);
        let now = Utc::now();
        let last_activity = now - Duration::seconds(idle_secs);

        let result = detector.is_stale(last_activity, now);
        let expected = idle_secs > threshold_secs as i64;

        prop_assert_eq!(
            result, expected,
            "Custom threshold {} with idle {} should yield {}",
            threshold_secs, idle_secs, expected
        );
    }

    // Feature: phase-2-complete, Property 2: Stale Context Detection Threshold
    // **Validates: Requirements 2.4**
    //
    // The welcome-back message SHALL contain all specified fields (idle duration,
    // pending task count, alert count) when pending items exist.
    #[test]
    fn welcome_back_contains_all_fields_when_pending(
        config in arb_stale_context_config(),
        idle_hours in 1u32..=48u32,
        tasks in proptest::collection::vec(
            "[a-zA-Z0-9 ]{1,30}".prop_map(|desc| PendingTaskResult { description: desc }),
            1..=5,
        ),
        alerts in proptest::collection::vec(
            "[a-zA-Z0-9 ]{1,30}".prop_map(|msg| HeartbeatAlert { message: msg }),
            1..=5,
        ),
    ) {
        let detector = StaleContextDetector::new(config);
        let idle_duration = Duration::hours(idle_hours as i64);

        let msg = detector.build_welcome_back(idle_duration, &tasks, &alerts);

        // Must contain idle duration representation
        prop_assert!(
            msg.contains(&format!("{}h", idle_hours)),
            "Message should contain idle duration '{}h', got: {}",
            idle_hours, msg
        );

        // Must contain pending task count
        prop_assert!(
            msg.contains(&format!("{} pending task(s)", tasks.len())),
            "Message should contain '{} pending task(s)', got: {}",
            tasks.len(), msg
        );

        // Must contain alert count
        prop_assert!(
            msg.contains(&format!("{} heartbeat alert(s)", alerts.len())),
            "Message should contain '{} heartbeat alert(s)', got: {}",
            alerts.len(), msg
        );
    }

    // Feature: phase-2-complete, Property 2: Stale Context Detection Threshold
    // **Validates: Requirements 2.5**
    //
    // The welcome-back message SHALL be a brief acknowledgment when no pending
    // items exist.
    #[test]
    fn welcome_back_brief_when_no_pending(
        config in arb_stale_context_config(),
        idle_hours in 1u32..=48u32,
    ) {
        let detector = StaleContextDetector::new(config);
        let idle_duration = Duration::hours(idle_hours as i64);

        let msg = detector.build_welcome_back(idle_duration, &[], &[]);

        // Should be a brief acknowledgment
        prop_assert!(
            msg.contains("Nothing pending"),
            "Message with no pending items should contain 'Nothing pending', got: {}",
            msg
        );

        // Should NOT contain detailed task/alert sections
        prop_assert!(
            !msg.contains("pending task(s)"),
            "Brief message should not contain task count section, got: {}",
            msg
        );
        prop_assert!(
            !msg.contains("heartbeat alert(s)"),
            "Brief message should not contain alert count section, got: {}",
            msg
        );
    }

    // Feature: phase-2-complete, Property 2: Stale Context Detection Threshold
    // **Validates: Requirements 2.4**
    //
    // When only tasks exist (no alerts), the message should include task info
    // but not alert info.
    #[test]
    fn welcome_back_tasks_only(
        config in arb_stale_context_config(),
        idle_hours in 1u32..=48u32,
        tasks in proptest::collection::vec(
            "[a-zA-Z0-9 ]{1,30}".prop_map(|desc| PendingTaskResult { description: desc }),
            1..=5,
        ),
    ) {
        let detector = StaleContextDetector::new(config);
        let idle_duration = Duration::hours(idle_hours as i64);

        let msg = detector.build_welcome_back(idle_duration, &tasks, &[]);

        // Should contain task count
        prop_assert!(
            msg.contains(&format!("{} pending task(s)", tasks.len())),
            "Message should contain task count, got: {}",
            msg
        );

        // Should NOT contain alert section
        prop_assert!(
            !msg.contains("heartbeat alert(s)"),
            "Message with no alerts should not contain alert section, got: {}",
            msg
        );
    }

    // Feature: phase-2-complete, Property 2: Stale Context Detection Threshold
    // **Validates: Requirements 2.4**
    //
    // When only alerts exist (no tasks), the message should include alert info
    // but not task info.
    #[test]
    fn welcome_back_alerts_only(
        config in arb_stale_context_config(),
        idle_hours in 1u32..=48u32,
        alerts in proptest::collection::vec(
            "[a-zA-Z0-9 ]{1,30}".prop_map(|msg| HeartbeatAlert { message: msg }),
            1..=5,
        ),
    ) {
        let detector = StaleContextDetector::new(config);
        let idle_duration = Duration::hours(idle_hours as i64);

        let msg = detector.build_welcome_back(idle_duration, &[], &alerts);

        // Should contain alert count
        prop_assert!(
            msg.contains(&format!("{} heartbeat alert(s)", alerts.len())),
            "Message should contain alert count, got: {}",
            msg
        );

        // Should NOT contain task section
        prop_assert!(
            !msg.contains("pending task(s)"),
            "Message with no tasks should not contain task section, got: {}",
            msg
        );
    }
}
