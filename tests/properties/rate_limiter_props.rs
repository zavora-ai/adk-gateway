//! Property-based tests for Rate Limiter sliding window accuracy.
//!
//! Feature: phase-2-complete, Property 3: Rate Limiter Sliding Window Accuracy
//! Validates: Requirements 3.1, 3.2, 3.5

use adk_gateway::config::RateLimitConfig;
use adk_gateway::rate_limiter::{RateLimitDecision, RateLimiter};
use proptest::prelude::*;
use std::time::{Duration, Instant};

// ── Strategies ─────────────────────────────────────────────────────

/// Strategy for a valid RateLimitConfig with reasonable bounds for testing.
fn arb_rate_limit_config() -> impl Strategy<Value = RateLimitConfig> {
    (
        1u32..=20u32,   // max_calls: 1 to 20
        1u64..=10u64,   // window_secs: 1 to 10
        1u64..=5u64,    // cooldown_secs: 1 to 5
        1u32..=5u32,    // max_triggers: 1 to 5
    )
        .prop_map(|(max_calls, window_secs, cooldown_secs, max_triggers)| RateLimitConfig {
            max_calls,
            window_secs,
            cooldown_secs,
            max_triggers,
        })
}

/// Strategy for a sequence of invocation offsets (in milliseconds) within a window.
/// Generates offsets that are monotonically increasing and within the window.
fn arb_invocation_offsets_in_window(
    count: usize,
    window_ms: u64,
) -> impl Strategy<Value = Vec<u64>> {
    proptest::collection::vec(0u64..window_ms, count)
        .prop_map(|mut offsets| {
            offsets.sort();
            offsets
        })
}

/// Strategy for a number of invocations that will stay within the threshold.
fn arb_count_within_threshold(max_calls: u32) -> impl Strategy<Value = u32> {
    1u32..=max_calls
}

/// Strategy for a number of invocations that will exceed the threshold.
fn arb_count_exceeding_threshold(max_calls: u32) -> impl Strategy<Value = u32> {
    (max_calls + 1)..=(max_calls + 10)
}

// ── Property Tests ─────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    // Feature: phase-2-complete, Property 3: Rate Limiter Sliding Window Accuracy
    // **Validates: Requirements 3.1**
    //
    // For any sequence of tool invocation timestamps within the configured window,
    // the sliding window counter SHALL accurately reflect the number of invocations
    // within the window.
    #[test]
    fn sliding_window_counter_accuracy(
        config in arb_rate_limit_config(),
        num_calls in 1u32..=30u32,
    ) {
        let mut limiter = RateLimiter::new(config.clone());
        let now = Instant::now();
        let window_duration = Duration::from_secs(config.window_secs);

        // Record invocations all within the window
        for i in 0..num_calls {
            let offset = Duration::from_millis(i as u64 * 10); // 10ms apart
            // Ensure all invocations are within the window
            if offset < window_duration {
                limiter.record_invocation("test_tool", now + offset);
            }
        }

        // Calculate how many invocations actually fit within the window
        let expected_in_window = std::cmp::min(
            num_calls,
            (config.window_secs * 100) as u32, // window_ms / 10ms spacing
        );

        // Check the window count at the time of the last invocation
        let check_time = now + Duration::from_millis((num_calls.saturating_sub(1)) as u64 * 10);
        let actual_count = limiter.window_count(check_time);

        // The window count should equal the number of invocations we made within the window
        prop_assert_eq!(
            actual_count, expected_in_window,
            "Window count {} should equal expected invocations {} (config: max_calls={}, window_secs={})",
            actual_count, expected_in_window, config.max_calls, config.window_secs
        );
    }

    // Feature: phase-2-complete, Property 3: Rate Limiter Sliding Window Accuracy
    // **Validates: Requirements 3.2**
    //
    // When the count exceeds the threshold (max_calls), the limiter SHALL signal
    // a Pause with the configured cooldown duration.
    #[test]
    fn pause_on_threshold_exceeded(
        config in arb_rate_limit_config(),
    ) {
        let mut limiter = RateLimiter::new(config.clone());
        let now = Instant::now();

        // Make exactly max_calls invocations (all should be Allow)
        for i in 0..config.max_calls {
            let decision = limiter.record_invocation("test_tool", now + Duration::from_millis(i as u64));
            prop_assert_eq!(
                decision,
                RateLimitDecision::Allow,
                "Invocation {} of {} should be allowed (max_calls={})",
                i + 1, config.max_calls, config.max_calls
            );
        }

        // The next invocation (max_calls + 1) should trigger a Pause
        let decision = limiter.record_invocation(
            "test_tool",
            now + Duration::from_millis(config.max_calls as u64),
        );
        let expected_cooldown = Duration::from_secs(config.cooldown_secs);
        prop_assert_eq!(
            decision,
            RateLimitDecision::Pause { duration: expected_cooldown },
            "Invocation {} should trigger Pause with {}s cooldown (max_calls={})",
            config.max_calls + 1, config.cooldown_secs, config.max_calls
        );
    }

    // Feature: phase-2-complete, Property 3: Rate Limiter Sliding Window Accuracy
    // **Validates: Requirements 3.5**
    //
    // When pauses are triggered max_triggers times, the limiter SHALL signal
    // termination. The trigger_count must reach max_triggers for Terminate.
    #[test]
    fn terminate_after_max_triggers(
        config in arb_rate_limit_config(),
    ) {
        let mut limiter = RateLimiter::new(config.clone());
        let now = Instant::now();
        let mut time_offset: u64 = 0;

        // We need to trigger the rate limit max_triggers times.
        // Each trigger requires exceeding max_calls within the window.
        // Since all calls are within the window, after max_calls we get triggers.
        for trigger_num in 0..config.max_triggers {
            // Fill up to max_calls (these are Allow)
            if trigger_num == 0 {
                for _ in 0..config.max_calls {
                    let decision = limiter.record_invocation("test_tool", now + Duration::from_millis(time_offset));
                    time_offset += 1;
                    // First max_calls should be Allow
                    if trigger_num == 0 {
                        prop_assert_eq!(decision, RateLimitDecision::Allow);
                    }
                }
            }

            // The next call exceeds the threshold
            let decision = limiter.record_invocation("test_tool", now + Duration::from_millis(time_offset));
            time_offset += 1;

            if trigger_num < config.max_triggers - 1 {
                // Should be Pause (not yet at max_triggers)
                prop_assert_eq!(
                    decision,
                    RateLimitDecision::Pause { duration: Duration::from_secs(config.cooldown_secs) },
                    "Trigger {} of {} should be Pause",
                    trigger_num + 1, config.max_triggers
                );
            } else {
                // Should be Terminate (reached max_triggers)
                prop_assert!(
                    matches!(decision, RateLimitDecision::Terminate { .. }),
                    "Trigger {} (max_triggers={}) should be Terminate, got {:?}",
                    trigger_num + 1, config.max_triggers, decision
                );
            }
        }

        // Verify trigger_count equals max_triggers
        prop_assert_eq!(
            limiter.trigger_count(),
            config.max_triggers,
            "trigger_count should equal max_triggers ({})",
            config.max_triggers
        );
    }

    // Feature: phase-2-complete, Property 3: Rate Limiter Sliding Window Accuracy
    // **Validates: Requirements 3.1**
    //
    // Invocations outside the sliding window SHALL NOT be counted.
    // After the window expires, old invocations are pruned and the counter
    // reflects only recent invocations.
    #[test]
    fn invocations_outside_window_not_counted(
        config in arb_rate_limit_config(),
        num_old_calls in 1u32..=15u32,
        num_new_calls in 1u32..=15u32,
    ) {
        let mut limiter = RateLimiter::new(config.clone());
        let now = Instant::now();
        let window_duration = Duration::from_secs(config.window_secs);

        // Record old invocations at time 0
        for i in 0..num_old_calls {
            limiter.record_invocation("old_tool", now + Duration::from_millis(i as u64));
        }

        // Move time forward past the window so old invocations expire
        let later = now + window_duration + Duration::from_secs(1);

        // Verify old invocations are no longer counted
        let count_after_expiry = limiter.window_count(later);
        prop_assert_eq!(
            count_after_expiry, 0,
            "After window expires, count should be 0 but got {} (window_secs={}, old_calls={})",
            count_after_expiry, config.window_secs, num_old_calls
        );

        // Record new invocations after the window has passed
        for i in 0..num_new_calls {
            limiter.record_invocation("new_tool", later + Duration::from_millis(i as u64));
        }

        // Only new invocations should be counted
        let check_time = later + Duration::from_millis(num_new_calls as u64);
        let final_count = limiter.window_count(check_time);
        prop_assert_eq!(
            final_count, num_new_calls,
            "After recording {} new calls, window count should be {} but got {} (old calls expired)",
            num_new_calls, num_new_calls, final_count
        );
    }
}
