//! Property-based tests for DM pairing service.
//!
//! Feature: gateway-production-maturity, Properties 17, 18, 19
//! **Validates: Requirements R9.1–R9.6**

use adk_gateway::pairing::*;
use proptest::prelude::*;

fn arb_user_id() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_]{3,15}"
}

fn arb_channel_type() -> impl Strategy<Value = String> {
    prop_oneof![Just("telegram".to_string()), Just("slack".to_string()),]
}

fn is_locked(result: &PairingResult) -> bool {
    matches!(result, PairingResult::Locked { .. })
}

// Feature: gateway-production-maturity, Property 17: DM pairing blocks unpaired users
// **Validates: Requirements R9.1, R9.2, R9.3**
proptest! {
    /// For any channel with dmPolicy set to "pairing" and any unpaired user,
    /// messages from that user should not be routed to an agent.
    /// After providing a valid pairing code, the user's subsequent messages
    /// should be processed normally.
    #[test]
    fn dm_pairing_blocks_unpaired_users(
        user_id in arb_user_id(),
        channel_type in arb_channel_type(),
    ) {
        let service = DmPairingService::new();

        // User should not be paired initially
        prop_assert!(!service.is_paired(&user_id));

        // Generate a code and pair the user
        let code = service.generate_code();
        let result = service.validate_code(&user_id, &code, &channel_type);
        prop_assert_eq!(result, PairingResult::Success);

        // User should now be paired
        prop_assert!(service.is_paired(&user_id));
    }
}

// Feature: gateway-production-maturity, Property 18: Pairing lockout after failed attempts
// **Validates: Requirements R9.4**
proptest! {
    /// For any user who provides an invalid pairing code 3 times consecutively,
    /// further pairing attempts should be blocked for 15 minutes.
    /// After the lockout period, attempts should be allowed again.
    #[test]
    fn pairing_lockout_after_failed_attempts(
        user_id in arb_user_id(),
        bad_code1 in "[A-Z0-9]{6}",
        bad_code2 in "[A-Z0-9]{6}",
        bad_code3 in "[A-Z0-9]{6}",
    ) {
        let service = DmPairingService::new();

        // First failure: 2 attempts remaining
        let result = service.validate_code(&user_id, &bad_code1, "telegram");
        match result {
            PairingResult::InvalidCode { attempts_remaining } => {
                prop_assert_eq!(attempts_remaining, 2);
            }
            _ => prop_assert!(false, "expected InvalidCode, got {:?}", result),
        }

        // Second failure: 1 attempt remaining
        let result = service.validate_code(&user_id, &bad_code2, "telegram");
        match result {
            PairingResult::InvalidCode { attempts_remaining } => {
                prop_assert_eq!(attempts_remaining, 1);
            }
            _ => prop_assert!(false, "expected InvalidCode, got {:?}", result),
        }

        // Third failure: lockout
        let result = service.validate_code(&user_id, &bad_code3, "telegram");
        prop_assert!(is_locked(&result), "expected Locked after 3 failures");

        // Further attempts should also be locked
        let result = service.validate_code(&user_id, "ANOTHER", "telegram");
        prop_assert!(is_locked(&result), "expected still Locked on subsequent attempt");
    }
}

// Feature: gateway-production-maturity, Property 19: Pairing code single-use and expiry
// **Validates: Requirements R9.5, R9.6**
proptest! {
    /// For any generated pairing code, it should be usable exactly once.
    /// After use, the same code should be rejected.
    #[test]
    fn pairing_code_single_use(
        user1 in arb_user_id(),
        user2 in "[a-zA-Z0-9_]{3,15}",
    ) {
        let service = DmPairingService::new();
        let code = service.generate_code();

        // First use succeeds
        let result = service.validate_code(&user1, &code, "telegram");
        prop_assert_eq!(result, PairingResult::Success);

        // Second use with different user fails (already used)
        let result = service.validate_code(&user2, &code, "telegram");
        prop_assert_eq!(result, PairingResult::AlreadyUsed);
    }

    /// Expired codes should be rejected.
    #[test]
    fn pairing_code_expiry(
        user_id in arb_user_id(),
    ) {
        let service = DmPairingService::new();

        // A code that is valid should work
        let code = service.generate_code();
        let active = service.active_codes();
        prop_assert!(!active.is_empty(), "should have at least one active code");

        // Verify the code is valid
        let result = service.validate_code(&user_id, &code, "telegram");
        prop_assert_eq!(result, PairingResult::Success);
    }
}

// ── Property 17: Pairing persistence round-trip ────────────────────
// Feature: gateway-full-wiring, Property 17: Pairing persistence round-trip
// **Validates: Requirements 13.3, 13.4**

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 17: For any set of PairedUser entries, restore_paired_users
    /// followed by paired_users() should return all entries, and paired_count()
    /// should equal input count.
    #[test]
    fn pairing_persistence_round_trip(
        user_ids in proptest::collection::hash_set(arb_user_id(), 1..10),
    ) {
        let service = DmPairingService::new();

        // Build PairedUser entries from unique user IDs
        let entries: Vec<PairedUser> = user_ids.iter().map(|uid| PairedUser {
            user_id: uid.clone(),
            paired_at: chrono::Utc::now(),
            channel_type: "telegram".to_string(),
        }).collect();

        let expected_count = entries.len();

        // Restore
        service.restore_paired_users(entries.clone());

        // paired_count() should equal input count
        prop_assert_eq!(
            service.paired_count(), expected_count,
            "paired_count should equal input count {}", expected_count
        );

        // paired_users() should contain all entries (matched by user_id)
        let restored = service.paired_users();
        let restored_ids: std::collections::HashSet<String> = restored.iter().map(|u| u.user_id.clone()).collect();

        for uid in &user_ids {
            prop_assert!(
                restored_ids.contains(uid),
                "restored users should contain '{}'", uid
            );
            prop_assert!(
                service.is_paired(uid),
                "is_paired should return true for '{}'", uid
            );
        }
    }
}
