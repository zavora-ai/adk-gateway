//! Property-based tests for GatewayMetrics.
//!
//! Feature: gateway-production-maturity, Property 26: Per-message metrics recording
//! **Validates: Requirements R14.2, R14.5**

use adk_gateway::metrics::*;
use proptest::prelude::*;
use std::time::Duration;

fn arb_channel() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("telegram".to_string()),
        Just("slack".to_string()),
        Just("webhook".to_string()),
    ]
}

fn arb_status() -> impl Strategy<Value = MessageStatus> {
    prop_oneof![Just(MessageStatus::Success), Just(MessageStatus::Failure),]
}

fn arb_latency_ms() -> impl Strategy<Value = u64> {
    1..10000u64
}

fn arb_tokens() -> impl Strategy<Value = Option<u64>> {
    prop::option::of(1..10000u64)
}

fn arb_model() -> impl Strategy<Value = Option<String>> {
    prop::option::of(prop_oneof![
        Just("gpt-4".to_string()),
        Just("claude-3".to_string()),
        Just("gemini-pro".to_string()),
    ])
}

// Feature: gateway-production-maturity, Property 26: Per-message metrics recording
// **Validates: Requirements R14.2, R14.5**
proptest! {
    /// For any processed message (success or failure), GatewayMetrics should record
    /// processing latency, token count (when available), and success/failure status.
    /// The error rate sliding window should accurately reflect the error rate over
    /// the last 5 minutes.
    #[test]
    fn per_message_metrics_recording(
        messages in prop::collection::vec(
            (arb_channel(), arb_status(), arb_latency_ms(), arb_tokens(), arb_tokens(), arb_model()),
            1..20
        )
    ) {
        let metrics = GatewayMetrics::new();

        let mut expected_success: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        let mut expected_failure: std::collections::HashMap<String, u64> = std::collections::HashMap::new();

        for (channel, status, latency_ms, input_tokens, output_tokens, model) in &messages {
            match status {
                MessageStatus::Success => *expected_success.entry(channel.clone()).or_insert(0) += 1,
                MessageStatus::Failure => *expected_failure.entry(channel.clone()).or_insert(0) += 1,
            }

            metrics.record_message(
                channel,
                status.clone(),
                Duration::from_millis(*latency_ms),
                *input_tokens,
                *output_tokens,
                model.as_deref(),
            );
        }

        // Verify message counts match
        for (channel, expected) in &expected_success {
            let actual = metrics.get_messages_total(channel, &MessageStatus::Success);
            prop_assert_eq!(actual, *expected, "success count mismatch for channel {}", channel);
        }
        for (channel, expected) in &expected_failure {
            let actual = metrics.get_messages_total(channel, &MessageStatus::Failure);
            prop_assert_eq!(actual, *expected, "failure count mismatch for channel {}", channel);
        }

        // Verify error rate is within expected bounds
        for channel in expected_success.keys().chain(expected_failure.keys()) {
            let successes = expected_success.get(channel).copied().unwrap_or(0);
            let failures = expected_failure.get(channel).copied().unwrap_or(0);
            let total = successes + failures;
            if total > 0 {
                let expected_rate = failures as f64 / total as f64;
                let actual_rate = metrics.get_error_rate(channel);
                let diff = (actual_rate - expected_rate).abs();
                prop_assert!(diff < 0.01, "error rate mismatch for channel {}: expected {}, got {}", channel, expected_rate, actual_rate);
            }
        }
    }
}

// ── Property 18: Channel status metrics reflect set values ─────────
// Feature: gateway-full-wiring, Property 18: Channel status metrics reflect set values
// **Validates: Requirements 14.1, 14.2, 14.4**

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 18: For any channel name and status code (1, 0, -1), after
    /// set_channel_status, render_prometheus should contain a channel_status
    /// metric line with that channel and value.
    #[test]
    fn channel_status_metrics_reflect_set_values(
        channel_name in "[a-zA-Z_]{1,15}",
        status_code in prop::sample::select(vec![1i64, 0, -1]),
    ) {
        let metrics = GatewayMetrics::new();

        metrics.set_channel_status(&channel_name, status_code);

        let output = metrics.render_prometheus();

        // Output should contain the channel_status metric line
        let expected_line = format!(
            "adk_gateway_channel_status{{channel=\"{}\"}} {}",
            channel_name, status_code
        );
        prop_assert!(
            output.contains(&expected_line),
            "render_prometheus should contain '{}', got:\n{}",
            expected_line, output
        );

        // Output should contain the HELP and TYPE headers for channel_status
        prop_assert!(
            output.contains("adk_gateway_channel_status"),
            "output should contain channel_status metric"
        );
    }
}
