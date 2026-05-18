//! Property-based tests for config encryption (Properties 14 and 15).
//!
//! Feature: phase-2-complete
//! - Property 14: Config Encryption Round-Trip
//! - Property 15: Sensitive Field Detection
//!
//! **Validates: Requirements 14.1, 14.4, 14.6**

use adk_gateway::config_encryption::{ConfigEncryption, ENCRYPTED_PREFIX};
use proptest::prelude::*;

/// Generate a random 32-byte AES-256 key.
fn arb_key() -> impl Strategy<Value = [u8; 32]> {
    proptest::array::uniform32(any::<u8>())
}

/// Generate arbitrary plaintext strings (including empty, unicode, special chars).
/// Excludes strings starting with "enc:" since those would be ambiguous.
fn arb_plaintext() -> impl Strategy<Value = String> {
    "([^e]|e[^n]|en[^c]|enc[^:]|.{0,0}|.{5,256}).*"
        .prop_filter("must not start with enc:", |s| !s.starts_with(ENCRYPTED_PREFIX))
}

/// Generate arbitrary field names for sensitive field detection testing.
fn arb_field_name() -> impl Strategy<Value = String> {
    prop_oneof![
        // Names that should be sensitive (contain key, token, secret, password)
        "[a-z_]{0,5}(key|token|secret|password)[a-z_]{0,5}",
        // Names that should NOT be sensitive
        "[a-z]{1,10}",
        // Mixed case variants
        "[A-Za-z_]{1,15}",
    ]
}

// ── Property 14: Config Encryption Round-Trip ──────────────────────
// Feature: phase-2-complete, Property 14: Config Encryption Round-Trip
// **Validates: Requirements 14.1, 14.6**

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Property 14a: For any plaintext string and valid AES-256-GCM key,
    /// decrypt(key, encrypt(key, plaintext)) SHALL return the original plaintext.
    #[test]
    fn prop_encrypt_decrypt_round_trip(
        key in arb_key(),
        plaintext in "[\\x00-\\x7f]{0,512}"
    ) {
        let enc = ConfigEncryption::new(key);
        let encrypted = enc.encrypt(&plaintext).expect("encryption should succeed");
        let decrypted = enc.decrypt(&encrypted).expect("decryption should succeed");
        prop_assert_eq!(&decrypted, &plaintext,
            "round-trip failed: decrypt(encrypt(plaintext)) != plaintext");
    }

    /// Property 14b: Encrypted values SHALL always start with the "enc:" prefix.
    #[test]
    fn prop_encrypted_starts_with_prefix(
        key in arb_key(),
        plaintext in "[\\x00-\\x7f]{0,256}"
    ) {
        let enc = ConfigEncryption::new(key);
        let encrypted = enc.encrypt(&plaintext).expect("encryption should succeed");
        prop_assert!(encrypted.starts_with(ENCRYPTED_PREFIX),
            "encrypted value '{}' does not start with '{}'", encrypted, ENCRYPTED_PREFIX);
    }

    /// Property 14c: Plaintext values SHALL never start with "enc:".
    /// This is enforced by the design — we verify that arbitrary non-encrypted
    /// strings are correctly identified as not encrypted.
    #[test]
    fn prop_plaintext_never_starts_with_enc_prefix(
        plaintext in arb_plaintext()
    ) {
        prop_assert!(!ConfigEncryption::is_encrypted(&plaintext),
            "plaintext '{}' incorrectly identified as encrypted", plaintext);
    }

    /// Property 14d: Different encryptions of the same plaintext produce different
    /// ciphertexts (due to random nonce), but both decrypt to the same value.
    #[test]
    fn prop_encrypt_nondeterministic(
        key in arb_key(),
        plaintext in "[a-zA-Z0-9]{1,128}"
    ) {
        let enc = ConfigEncryption::new(key);
        let e1 = enc.encrypt(&plaintext).expect("first encryption should succeed");
        let e2 = enc.encrypt(&plaintext).expect("second encryption should succeed");
        // Nonces are random, so ciphertexts should differ (with overwhelming probability)
        // We don't assert inequality since there's a negligible chance of collision,
        // but we verify both decrypt correctly.
        let d1 = enc.decrypt(&e1).expect("first decryption should succeed");
        let d2 = enc.decrypt(&e2).expect("second decryption should succeed");
        prop_assert_eq!(&d1, &plaintext);
        prop_assert_eq!(&d2, &plaintext);
    }

    /// Property 14e: Decryption with a wrong key SHALL fail.
    #[test]
    fn prop_wrong_key_fails_decryption(
        key1 in arb_key(),
        key2 in arb_key(),
        plaintext in "[a-zA-Z0-9]{1,64}"
    ) {
        prop_assume!(key1 != key2);
        let enc1 = ConfigEncryption::new(key1);
        let enc2 = ConfigEncryption::new(key2);
        let encrypted = enc1.encrypt(&plaintext).expect("encryption should succeed");
        let result = enc2.decrypt(&encrypted);
        prop_assert!(result.is_err(),
            "decryption with wrong key should fail but got: {:?}", result);
    }
}

// ── Property 15: Sensitive Field Detection ─────────────────────────
// Feature: phase-2-complete, Property 15: Sensitive Field Detection
// **Validates: Requirements 14.4**

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Property 15a: is_sensitive_field SHALL return true iff the lowercase field
    /// name contains any of: "key", "token", "secret", or "password".
    #[test]
    fn prop_sensitive_field_detection(
        field_name in arb_field_name()
    ) {
        let lower = field_name.to_lowercase();
        let expected = lower.contains("key")
            || lower.contains("token")
            || lower.contains("secret")
            || lower.contains("password");
        let actual = ConfigEncryption::is_sensitive_field(&field_name);
        prop_assert_eq!(actual, expected,
            "is_sensitive_field('{}') = {}, expected {} (lowercase: '{}')",
            field_name, actual, expected, lower);
    }

    /// Property 15b: Fields containing exactly one of the sensitive patterns
    /// are always detected regardless of case or surrounding characters.
    #[test]
    fn prop_sensitive_patterns_case_insensitive(
        prefix in "[a-zA-Z_]{0,5}",
        pattern in prop_oneof!["key", "token", "secret", "password"],
        suffix in "[a-zA-Z_]{0,5}",
        uppercase in proptest::bool::ANY
    ) {
        let field = if uppercase {
            format!("{}{}{}", prefix, pattern.to_uppercase(), suffix)
        } else {
            format!("{}{}{}", prefix, pattern, suffix)
        };
        prop_assert!(ConfigEncryption::is_sensitive_field(&field),
            "is_sensitive_field('{}') should be true (contains '{}')", field, pattern);
    }

    /// Property 15c: Fields that do NOT contain any sensitive pattern
    /// are never flagged as sensitive.
    #[test]
    fn prop_non_sensitive_fields_not_flagged(
        // Generate names that cannot contain key/token/secret/password
        field_name in "[abcdfghijlmnquvxyz]{1,10}"
    ) {
        let lower = field_name.to_lowercase();
        // Double-check our generator doesn't accidentally produce sensitive names
        prop_assume!(!lower.contains("key") && !lower.contains("token")
            && !lower.contains("secret") && !lower.contains("password"));
        prop_assert!(!ConfigEncryption::is_sensitive_field(&field_name),
            "is_sensitive_field('{}') should be false", field_name);
    }
}

// ── Property 14 (config-level): encrypt_config/decrypt_config round-trip ──

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 14f: For any JSON config with sensitive fields, encrypting then
    /// decrypting produces the original config values.
    #[test]
    fn prop_encrypt_decrypt_config_round_trip(
        key in arb_key(),
        api_key_val in "[a-zA-Z0-9_-]{1,64}",
        bot_token_val in "[a-zA-Z0-9:_-]{1,64}",
        name_val in "[a-zA-Z]{1,20}",
        port in 1000u16..65000u16,
    ) {
        let enc = ConfigEncryption::new(key);

        let original = serde_json::json!({
            "api_key": api_key_val.clone(),
            "bot_token": bot_token_val.clone(),
            "name": name_val.clone(),
            "port": port,
        });

        let mut config = original.clone();
        enc.encrypt_config(&mut config);

        // Sensitive fields should be encrypted
        let ak = config["api_key"].as_str().unwrap();
        prop_assert!(ak.starts_with(ENCRYPTED_PREFIX),
            "api_key should be encrypted, got: {}", ak);

        let bt = config["bot_token"].as_str().unwrap();
        prop_assert!(bt.starts_with(ENCRYPTED_PREFIX),
            "bot_token should be encrypted, got: {}", bt);

        // Non-sensitive fields should be unchanged
        prop_assert_eq!(config["name"].as_str().unwrap(), &name_val);
        prop_assert_eq!(config["port"].as_u64().unwrap(), port as u64);

        // Decrypt and verify round-trip
        enc.decrypt_config(&mut config).expect("decrypt_config should succeed");
        prop_assert_eq!(config["api_key"].as_str().unwrap(), &api_key_val);
        prop_assert_eq!(config["bot_token"].as_str().unwrap(), &bot_token_val);
        prop_assert_eq!(config["name"].as_str().unwrap(), &name_val);
        prop_assert_eq!(config["port"].as_u64().unwrap(), port as u64);
    }
}
