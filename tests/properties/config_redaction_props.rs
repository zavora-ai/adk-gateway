//! Property-based tests for config display redaction.
//!
//! Feature: gateway-production-maturity, Property 12: Config display redacts sensitive values
//! **Validates: Requirements R5.4**

use adk_gateway::telemetry::redact_config;
use proptest::prelude::*;
use serde_json::json;

fn arb_non_empty_string() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_-]{1,20}"
}

fn arb_sensitive_key() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("token".to_string()),
        Just("botToken".to_string()),
        Just("appToken".to_string()),
        Just("password".to_string()),
        Just("secret".to_string()),
        Just("api_key".to_string()),
        Just("apiKey".to_string()),
        Just("bot_token".to_string()),
        Just("app_token".to_string()),
        Just("connection_string".to_string()),
        Just("connectionString".to_string()),
    ]
}

fn arb_non_sensitive_key() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("name".to_string()),
        Just("port".to_string()),
        Just("enabled".to_string()),
        Just("model".to_string()),
        Just("channel".to_string()),
        Just("id".to_string()),
    ]
}

// Feature: gateway-production-maturity, Property 12: Config display redacts sensitive values
// **Validates: Requirements R5.4**
proptest! {
    /// For any GatewayConfig containing sensitive fields (tokens, passwords, API keys),
    /// the serialized output for Control Panel display should replace those values with "***".
    #[test]
    fn config_display_redacts_sensitive_values(
        sensitive_key in arb_sensitive_key(),
        sensitive_value in arb_non_empty_string(),
        safe_key in arb_non_sensitive_key(),
        safe_value in arb_non_empty_string(),
    ) {
        let config = json!({
            sensitive_key.clone(): sensitive_value.clone(),
            safe_key.clone(): safe_value.clone(),
        });

        let redacted = redact_config(&config);

        // Sensitive field should be redacted
        prop_assert_eq!(
            redacted[&sensitive_key].as_str().unwrap(),
            "***",
            "sensitive field '{}' should be redacted", sensitive_key
        );

        // Non-sensitive field should be preserved
        prop_assert_eq!(
            redacted[&safe_key].as_str().unwrap(),
            &safe_value,
            "non-sensitive field '{}' should be preserved", safe_key
        );
    }

    /// Nested sensitive values should also be redacted.
    #[test]
    fn config_display_redacts_nested_sensitive_values(
        sensitive_key in arb_sensitive_key(),
        sensitive_value in arb_non_empty_string(),
    ) {
        let config = json!({
            "outer": {
                "inner": {
                    sensitive_key.clone(): sensitive_value.clone(),
                }
            }
        });

        let redacted = redact_config(&config);
        prop_assert_eq!(
            redacted["outer"]["inner"][&sensitive_key].as_str().unwrap(),
            "***",
            "nested sensitive field should be redacted"
        );
    }
}
