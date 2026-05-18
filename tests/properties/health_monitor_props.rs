//! Property-based tests for the Health Monitor state machine.
//!
//! Feature: phase-2-complete, Property 11: Health Monitor State Machine
//!
//! *For any* sequence of health check results for a component, an alert SHALL be
//! emitted if and only if there are 3 or more consecutive failures. A recovery
//! notification SHALL be emitted if and only if the component transitions from an
//! alerted state (3+ consecutive failures) to a passing state. No duplicate alerts
//! or recoveries SHALL be emitted for the same state.
//!
//! **Validates: Requirements 11.2, 11.3**

use adk_gateway::config::HealthMonitorConfig;
use adk_gateway::health_monitor::{HealthEvent, HealthMonitor};
use proptest::prelude::*;

/// Generate a random sequence of health check results (true = healthy, false = failed).
fn health_check_sequence() -> impl Strategy<Value = Vec<bool>> {
    prop::collection::vec(prop::bool::ANY, 1..100)
}

/// Generate a random failure threshold in [1, 10].
fn failure_threshold() -> impl Strategy<Value = u32> {
    1u32..=10u32
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Property 11: Alert emitted if and only if consecutive failures reach threshold.
    ///
    /// For any sequence of health checks and any threshold, an alert is emitted
    /// exactly when the consecutive failure count first reaches the threshold.
    /// No duplicate alerts are emitted for the same failure streak.
    ///
    /// **Validates: Requirements 11.2**
    #[test]
    fn prop_alert_emitted_iff_threshold_reached(
        checks in health_check_sequence(),
        threshold in failure_threshold(),
    ) {
        let config = HealthMonitorConfig {
            check_interval_secs: 60,
            failure_threshold: threshold,
            alert_webhook_url: None,
            alert_telegram_admin: None,
        };
        let monitor = HealthMonitor::new(config);

        let mut consecutive_failures: u32 = 0;
        let mut alerted = false;

        for healthy in &checks {
            let event = monitor.record_check("test_component", *healthy);

            if *healthy {
                consecutive_failures = 0;
                alerted = false;
            } else {
                consecutive_failures += 1;
            }

            match event {
                Some(HealthEvent::Alert { component, failures }) => {
                    // Alert should only fire when threshold is first reached
                    prop_assert_eq!(&component, "test_component");
                    prop_assert!(consecutive_failures >= threshold,
                        "Alert emitted at {} failures but threshold is {}",
                        consecutive_failures, threshold);
                    prop_assert!(!alerted,
                        "Duplicate alert emitted for same failure streak");
                    prop_assert_eq!(failures, consecutive_failures);
                    alerted = true;
                }
                Some(HealthEvent::Recovery { .. }) => {
                    // Recovery is handled in the next property test
                }
                None => {
                    // No alert: either healthy, below threshold, or already alerted
                    if !*healthy && consecutive_failures >= threshold && !alerted {
                        // This should not happen — alert should have been emitted
                        prop_assert!(false,
                            "No alert emitted at {} consecutive failures (threshold: {})",
                            consecutive_failures, threshold);
                    }
                }
            }

            // Track alerted state for duplicate detection
            if *healthy {
                alerted = false;
            }
        }
    }

    /// Property 11: Recovery emitted if and only if transitioning from alerted to healthy.
    ///
    /// A recovery notification is emitted when a component that was in an alerted
    /// state (3+ consecutive failures with alert emitted) transitions to healthy.
    /// No duplicate recoveries are emitted.
    ///
    /// **Validates: Requirements 11.3**
    #[test]
    fn prop_recovery_emitted_iff_alerted_to_healthy(
        checks in health_check_sequence(),
        threshold in failure_threshold(),
    ) {
        let config = HealthMonitorConfig {
            check_interval_secs: 60,
            failure_threshold: threshold,
            alert_webhook_url: None,
            alert_telegram_admin: None,
        };
        let monitor = HealthMonitor::new(config);

        let mut consecutive_failures: u32 = 0;
        let mut alerted = false;

        for healthy in &checks {
            let event = monitor.record_check("test_component", *healthy);

            if *healthy {
                match event {
                    Some(HealthEvent::Recovery { ref component }) => {
                        // Recovery should only fire when transitioning from alerted state
                        prop_assert_eq!(component, "test_component");
                        prop_assert!(alerted,
                            "Recovery emitted but component was not in alerted state");
                    }
                    None => {
                        // No recovery: component was not in alerted state
                        // (This is correct behavior when not previously alerted)
                    }
                    Some(HealthEvent::Alert { .. }) => {
                        prop_assert!(false, "Alert emitted on healthy check");
                    }
                }
                // If was alerted and now healthy, recovery should have been emitted
                if alerted {
                    prop_assert!(
                        matches!(event, Some(HealthEvent::Recovery { .. })),
                        "Expected recovery event when transitioning from alerted to healthy"
                    );
                }
                consecutive_failures = 0;
                alerted = false;
            } else {
                consecutive_failures += 1;
                if consecutive_failures >= threshold && !alerted {
                    alerted = true;
                }
            }
        }
    }

    /// Property 11: No duplicate alerts or recoveries for the same state.
    ///
    /// Once an alert is emitted, no further alerts are emitted until recovery.
    /// Once a recovery is emitted, no further recoveries are emitted until a new alert.
    ///
    /// **Validates: Requirements 11.2, 11.3**
    #[test]
    fn prop_no_duplicate_events(
        checks in health_check_sequence(),
        threshold in failure_threshold(),
    ) {
        let config = HealthMonitorConfig {
            check_interval_secs: 60,
            failure_threshold: threshold,
            alert_webhook_url: None,
            alert_telegram_admin: None,
        };
        let monitor = HealthMonitor::new(config);

        let mut last_event_was_alert = false;
        let mut last_event_was_recovery = false;

        for healthy in &checks {
            let event = monitor.record_check("test_component", *healthy);

            match event {
                Some(HealthEvent::Alert { .. }) => {
                    // Should not get two alerts in a row without a recovery in between
                    prop_assert!(!last_event_was_alert,
                        "Duplicate alert emitted without intervening recovery");
                    last_event_was_alert = true;
                    last_event_was_recovery = false;
                }
                Some(HealthEvent::Recovery { .. }) => {
                    // Should not get two recoveries in a row without an alert in between
                    prop_assert!(!last_event_was_recovery,
                        "Duplicate recovery emitted without intervening alert");
                    // Recovery must follow an alert
                    prop_assert!(last_event_was_alert,
                        "Recovery emitted without prior alert");
                    last_event_was_recovery = true;
                    last_event_was_alert = false;
                }
                None => {
                    // No event — state unchanged
                }
            }
        }
    }

    /// Property 11: Alert fires at exactly the threshold count.
    ///
    /// For any threshold T, the alert fires on exactly the T-th consecutive failure,
    /// not before and not after (unless already alerted).
    ///
    /// **Validates: Requirements 11.2**
    #[test]
    fn prop_alert_fires_at_exact_threshold(
        threshold in failure_threshold(),
        extra_failures in 0u32..20u32,
    ) {
        let config = HealthMonitorConfig {
            check_interval_secs: 60,
            failure_threshold: threshold,
            alert_webhook_url: None,
            alert_telegram_admin: None,
        };
        let monitor = HealthMonitor::new(config);

        // Failures before threshold: no alert
        for i in 1..threshold {
            let event = monitor.record_check("test_component", false);
            prop_assert_eq!(event, None,
                "Alert emitted at failure {} before threshold {}",
                i, threshold);
        }

        // Exactly at threshold: alert
        let event = monitor.record_check("test_component", false);
        prop_assert_eq!(
            event,
            Some(HealthEvent::Alert {
                component: "test_component".to_string(),
                failures: threshold,
            }),
            "Alert not emitted at exact threshold {}",
            threshold
        );

        // After threshold: no more alerts
        for _ in 0..extra_failures {
            let event = monitor.record_check("test_component", false);
            prop_assert_eq!(event, None,
                "Duplicate alert emitted after threshold");
        }
    }
}
