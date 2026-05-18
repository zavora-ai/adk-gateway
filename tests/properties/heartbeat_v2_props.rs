//! Property-based tests for Heartbeat Turn Filtering.
//!
//! Feature: phase-2-complete, Property 8: Heartbeat Turn Filtering
//! **Validates: Requirements 7.3, 7.4, 7.5**

use adk_gateway::heartbeat_v2::{HeartbeatResponseKind, HeartbeatV2, Turn};
use proptest::prelude::*;

// ── Strategies ─────────────────────────────────────────────────────

/// Strategy for generating a regular (non-heartbeat) turn.
fn arb_regular_turn() -> impl Strategy<Value = Turn> {
    (
        prop_oneof!["user", "assistant", "system"],
        "[a-zA-Z0-9 !?.]{1,100}",
    )
        .prop_map(|(role, content)| Turn::regular(role, content))
}

/// Strategy for generating a heartbeat OK pair (user prompt + assistant "HEARTBEAT_OK").
fn arb_heartbeat_ok_pair() -> impl Strategy<Value = (Turn, Turn)> {
    "[a-zA-Z0-9 ]{1,50}".prop_map(|prompt_content| {
        (
            Turn::heartbeat("user", prompt_content),
            Turn::heartbeat("assistant", "HEARTBEAT_OK"),
        )
    })
}

/// Strategy for generating a heartbeat alert pair (user prompt + assistant alert).
fn arb_heartbeat_alert_pair() -> impl Strategy<Value = (Turn, Turn)> {
    (
        "[a-zA-Z0-9 ]{1,50}",
        // Alert content must NOT be exactly "HEARTBEAT_OK" (trimmed)
        "[a-zA-Z0-9 !?.]{1,100}".prop_filter(
            "Alert content must not be exactly HEARTBEAT_OK",
            |s| s.trim() != "HEARTBEAT_OK",
        ),
    )
        .prop_map(|(prompt, alert)| {
            (
                Turn::heartbeat("user", prompt),
                Turn::heartbeat("assistant", alert),
            )
        })
}

/// Strategy for generating a mixed session history with regular turns,
/// heartbeat OK pairs, and heartbeat alert pairs.
fn arb_mixed_history() -> impl Strategy<Value = Vec<Turn>> {
    // Generate a sequence of "segments" that are either:
    // - A regular turn
    // - A heartbeat OK pair
    // - A heartbeat alert pair
    proptest::collection::vec(
        prop_oneof![
            3 => arb_regular_turn().prop_map(|t| vec![t]),
            2 => arb_heartbeat_ok_pair().prop_map(|(u, a)| vec![u, a]),
            2 => arb_heartbeat_alert_pair().prop_map(|(u, a)| vec![u, a]),
        ],
        0..=10,
    )
    .prop_map(|segments| segments.into_iter().flatten().collect())
}

/// Strategy for generating a response string.
fn arb_response() -> impl Strategy<Value = String> {
    prop_oneof![
        // Exact HEARTBEAT_OK (with optional whitespace)
        Just("HEARTBEAT_OK".to_string()),
        Just("  HEARTBEAT_OK  ".to_string()),
        Just("HEARTBEAT_OK\n".to_string()),
        // Alert responses (anything else)
        "[a-zA-Z0-9 !?.]{1,100}".prop_filter(
            "Must not be HEARTBEAT_OK when trimmed",
            |s| s.trim() != "HEARTBEAT_OK"
        ),
    ]
}

// ── Property Tests ─────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    // Feature: phase-2-complete, Property 8: Heartbeat Turn Filtering
    // **Validates: Requirements 7.3, 7.4**
    //
    // For any session history containing a mix of regular turns and heartbeat turns,
    // the `strip_heartbeat_turns` function SHALL remove all heartbeat turns where
    // the response is exactly "HEARTBEAT_OK" and SHALL retain all heartbeat turns
    // where the response contains an actionable alert.
    #[test]
    fn strip_removes_ok_and_retains_alerts(
        history in arb_mixed_history(),
    ) {
        let mut working_history = history.clone();
        HeartbeatV2::strip_heartbeat_turns(&mut working_history);

        // Count expected remaining turns:
        // - All regular turns should remain
        // - Heartbeat alert pairs should remain
        // - Heartbeat OK pairs should be removed

        // Verify: no remaining heartbeat turn has "HEARTBEAT_OK" content
        for turn in &working_history {
            if turn.is_heartbeat() && turn.role == "assistant" {
                let kind = HeartbeatV2::classify_response(&turn.content);
                prop_assert_ne!(
                    kind,
                    HeartbeatResponseKind::Ok,
                    "Found a HEARTBEAT_OK turn that should have been stripped: {:?}",
                    turn
                );
            }
        }

        // Verify: all heartbeat alert turns from original are still present
        let original_alert_turns: Vec<&Turn> = history
            .iter()
            .filter(|t| {
                t.is_heartbeat()
                    && t.role == "assistant"
                    && HeartbeatV2::classify_response(&t.content) != HeartbeatResponseKind::Ok
            })
            .collect();

        let remaining_alert_turns: Vec<&Turn> = working_history
            .iter()
            .filter(|t| {
                t.is_heartbeat()
                    && t.role == "assistant"
                    && HeartbeatV2::classify_response(&t.content) != HeartbeatResponseKind::Ok
            })
            .collect();

        prop_assert_eq!(
            original_alert_turns.len(),
            remaining_alert_turns.len(),
            "Alert heartbeat turns should be retained. Original: {}, Remaining: {}",
            original_alert_turns.len(),
            remaining_alert_turns.len()
        );
    }

    // Feature: phase-2-complete, Property 8: Heartbeat Turn Filtering
    // **Validates: Requirements 7.5**
    //
    // Regular (non-heartbeat) turns SHALL never be affected by strip_heartbeat_turns.
    #[test]
    fn strip_never_affects_regular_turns(
        history in arb_mixed_history(),
    ) {
        let original_regular_turns: Vec<Turn> = history
            .iter()
            .filter(|t| !t.is_heartbeat())
            .cloned()
            .collect();

        let mut working_history = history.clone();
        HeartbeatV2::strip_heartbeat_turns(&mut working_history);

        let remaining_regular_turns: Vec<Turn> = working_history
            .iter()
            .filter(|t| !t.is_heartbeat())
            .cloned()
            .collect();

        prop_assert_eq!(
            original_regular_turns.len(),
            remaining_regular_turns.len(),
            "Regular turn count changed: {} -> {}",
            original_regular_turns.len(),
            remaining_regular_turns.len()
        );

        // Verify content and order preserved
        for (orig, remaining) in original_regular_turns.iter().zip(remaining_regular_turns.iter()) {
            prop_assert_eq!(
                orig, remaining,
                "Regular turn content/order changed"
            );
        }
    }

    // Feature: phase-2-complete, Property 8: Heartbeat Turn Filtering
    // **Validates: Requirements 7.3, 7.4**
    //
    // The classify_response function SHALL return Ok if and only if the trimmed
    // response is exactly "HEARTBEAT_OK". All other responses SHALL be classified
    // as Alert.
    #[test]
    fn classify_response_correctness(
        response in arb_response(),
    ) {
        let kind = HeartbeatV2::classify_response(&response);
        let trimmed = response.trim();

        if trimmed == "HEARTBEAT_OK" {
            prop_assert_eq!(
                kind,
                HeartbeatResponseKind::Ok,
                "Response '{}' (trimmed: '{}') should be Ok",
                response, trimmed
            );
        } else {
            prop_assert!(
                matches!(kind, HeartbeatResponseKind::Alert(_)),
                "Response '{}' (trimmed: '{}') should be Alert, got {:?}",
                response, trimmed, kind
            );
        }
    }

    // Feature: phase-2-complete, Property 8: Heartbeat Turn Filtering
    // **Validates: Requirements 7.5**
    //
    // For a history containing ONLY regular turns, strip_heartbeat_turns SHALL
    // return the history completely unchanged (identity operation).
    #[test]
    fn strip_is_identity_on_regular_only_history(
        turns in proptest::collection::vec(arb_regular_turn(), 0..=20),
    ) {
        let mut working = turns.clone();
        HeartbeatV2::strip_heartbeat_turns(&mut working);

        prop_assert_eq!(
            turns.len(),
            working.len(),
            "History length changed on regular-only history"
        );

        for (orig, result) in turns.iter().zip(working.iter()) {
            prop_assert_eq!(orig, result, "Turn content changed on regular-only history");
        }
    }

    // Feature: phase-2-complete, Property 8: Heartbeat Turn Filtering
    // **Validates: Requirements 7.3**
    //
    // For a history containing ONLY heartbeat OK pairs, strip_heartbeat_turns
    // SHALL produce an empty history.
    #[test]
    fn strip_removes_all_ok_pairs(
        pairs in proptest::collection::vec(arb_heartbeat_ok_pair(), 1..=10),
    ) {
        let mut history: Vec<Turn> = pairs.into_iter().flat_map(|(u, a)| vec![u, a]).collect();
        HeartbeatV2::strip_heartbeat_turns(&mut history);

        prop_assert!(
            history.is_empty(),
            "History should be empty after stripping all OK pairs, but has {} turns",
            history.len()
        );
    }

    // Feature: phase-2-complete, Property 8: Heartbeat Turn Filtering
    // **Validates: Requirements 7.4**
    //
    // For a history containing ONLY heartbeat alert pairs, strip_heartbeat_turns
    // SHALL retain all turns (no removals).
    #[test]
    fn strip_retains_all_alert_pairs(
        pairs in proptest::collection::vec(arb_heartbeat_alert_pair(), 1..=10),
    ) {
        let original_len = pairs.len() * 2;
        let mut history: Vec<Turn> = pairs.into_iter().flat_map(|(u, a)| vec![u, a]).collect();
        HeartbeatV2::strip_heartbeat_turns(&mut history);

        prop_assert_eq!(
            history.len(),
            original_len,
            "All alert pairs should be retained, expected {} turns but got {}",
            original_len,
            history.len()
        );
    }
}
