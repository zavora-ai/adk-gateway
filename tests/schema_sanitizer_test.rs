//! Property-based tests for schema sanitization (Properties 4, 5, 6).
//!
//! Feature: phase-2-complete
//! - Property 4: Schema Sanitization Provider Correctness
//!   **Validates: Requirements 5.1, 5.2, 5.3, 5.4**
//! - Property 5: Gemini Exclusive Bound Conversion
//!   **Validates: Requirements 5.7, 5.8**
//! - Property 6: Schema Storage Immutability
//!   **Validates: Requirements 5.6**

use adk_gateway::schema_sanitizer::{GeminiSanitizer, IdentitySanitizer, SchemaSanitizer};
use proptest::prelude::*;
use serde_json::{json, Value};

// ── Strategies ─────────────────────────────────────────────────────

/// JSON Schema type names that Gemini supports as strings.
fn json_type_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("string".to_string()),
        Just("integer".to_string()),
        Just("number".to_string()),
        Just("boolean".to_string()),
        Just("object".to_string()),
        Just("array".to_string()),
    ]
}

/// Generate a simple JSON Schema object with optional forbidden fields for Gemini.
/// This produces schemas that may contain exclusiveMinimum, exclusiveMaximum,
/// propertyNames, array-typed items, or array-typed type fields.
fn schema_with_forbidden_fields_strategy() -> impl Strategy<Value = Value> {
    (
        json_type_strategy(),
        any::<bool>(), // include propertyNames?
        any::<bool>(), // include exclusiveMinimum?
        any::<bool>(), // include exclusiveMaximum?
        any::<bool>(), // use array-typed type?
        any::<bool>(), // use array-typed items?
        -1000i64..1000i64, // exclusiveMinimum value
        -1000i64..1000i64, // exclusiveMaximum value
    )
        .prop_map(
            |(
                type_name,
                include_property_names,
                include_exc_min,
                include_exc_max,
                use_array_type,
                use_array_items,
                exc_min_val,
                exc_max_val,
            )| {
                let mut schema = serde_json::Map::new();

                // Set type field — either as string or array
                if use_array_type {
                    schema.insert(
                        "type".to_string(),
                        json!([type_name.as_str(), "null"]),
                    );
                } else {
                    schema.insert("type".to_string(), json!(type_name));
                }

                // Optionally add propertyNames
                if include_property_names {
                    schema.insert(
                        "propertyNames".to_string(),
                        json!({"pattern": "^[a-z]+$"}),
                    );
                }

                // Optionally add exclusiveMinimum (only meaningful for integer type)
                if include_exc_min && type_name == "integer" {
                    schema.insert(
                        "exclusiveMinimum".to_string(),
                        json!(exc_min_val),
                    );
                }

                // Optionally add exclusiveMaximum (only meaningful for integer type)
                if include_exc_max && type_name == "integer" {
                    schema.insert(
                        "exclusiveMaximum".to_string(),
                        json!(exc_max_val),
                    );
                }

                // Optionally use array-typed items
                if use_array_items && type_name == "array" {
                    schema.insert(
                        "items".to_string(),
                        json!([{"type": "string"}, {"type": "integer"}]),
                    );
                }

                // Add a description to make it more realistic
                schema.insert(
                    "description".to_string(),
                    json!("A test schema field"),
                );

                Value::Object(schema)
            },
        )
}

/// Generate a nested JSON Schema with properties containing potentially forbidden fields.
fn nested_schema_strategy() -> impl Strategy<Value = Value> {
    (
        schema_with_forbidden_fields_strategy(),
        schema_with_forbidden_fields_strategy(),
    )
        .prop_map(|(inner1, inner2)| {
            json!({
                "type": "object",
                "properties": {
                    "field_a": inner1,
                    "field_b": inner2
                }
            })
        })
}

/// Generate an arbitrary valid JSON Schema (without forbidden fields) for identity testing.
fn clean_schema_strategy() -> impl Strategy<Value = Value> {
    (json_type_strategy(), 0..3u8).prop_map(|(type_name, variant)| match variant {
        0 => json!({
            "type": type_name,
            "description": "A simple field"
        }),
        1 => json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "count": { "type": "integer", "minimum": 0 }
            },
            "required": ["name"]
        }),
        _ => json!({
            "type": "array",
            "items": { "type": type_name }
        }),
    })
}

// ── Helper: recursively check for forbidden fields ─────────────────

/// Returns true if the JSON value contains any Gemini-forbidden fields.
fn contains_forbidden_fields(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            // Check for propertyNames key
            if map.contains_key("propertyNames") {
                return true;
            }
            // Check for exclusiveMinimum key
            if map.contains_key("exclusiveMinimum") {
                return true;
            }
            // Check for exclusiveMaximum key
            if map.contains_key("exclusiveMaximum") {
                return true;
            }
            // Check for array-typed `type`
            if let Some(type_val) = map.get("type") {
                if type_val.is_array() {
                    return true;
                }
            }
            // Check for array-typed `items`
            if let Some(items_val) = map.get("items") {
                if items_val.is_array() {
                    return true;
                }
            }
            // Recurse into all values
            map.values().any(|v| contains_forbidden_fields(v))
        }
        Value::Array(arr) => arr.iter().any(|v| contains_forbidden_fields(v)),
        _ => false,
    }
}

// ── Property 4: Schema Sanitization Provider Correctness ───────────

// Feature: phase-2-complete, Property 4: Schema Sanitization Provider Correctness
// **Validates: Requirements 5.1, 5.2, 5.3, 5.4**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Property 4a: For any valid JSON Schema, the Gemini sanitizer SHALL produce
    /// a schema that does not contain `exclusiveMinimum`, `exclusiveMaximum`,
    /// `propertyNames`, array-typed `items`, or array-typed `type` fields.
    #[test]
    fn gemini_sanitizer_removes_all_forbidden_fields(
        schema in schema_with_forbidden_fields_strategy()
    ) {
        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        prop_assert!(
            !contains_forbidden_fields(&result),
            "Gemini sanitizer output still contains forbidden fields.\nInput: {}\nOutput: {}",
            serde_json::to_string_pretty(&schema).unwrap(),
            serde_json::to_string_pretty(&result).unwrap()
        );
    }

    /// Property 4b: For any nested JSON Schema, the Gemini sanitizer SHALL
    /// recursively remove all forbidden fields at any depth.
    #[test]
    fn gemini_sanitizer_removes_forbidden_fields_nested(
        schema in nested_schema_strategy()
    ) {
        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        prop_assert!(
            !contains_forbidden_fields(&result),
            "Gemini sanitizer output still contains forbidden fields in nested schema.\nInput: {}\nOutput: {}",
            serde_json::to_string_pretty(&schema).unwrap(),
            serde_json::to_string_pretty(&result).unwrap()
        );
    }

    /// Property 4c: For non-Gemini providers (OpenAI, Anthropic), the identity
    /// sanitizer SHALL return a schema identical to the input.
    #[test]
    fn identity_sanitizer_returns_input_unchanged(
        schema in schema_with_forbidden_fields_strategy()
    ) {
        let sanitizer = IdentitySanitizer;
        let result = sanitizer.sanitize(&schema);

        prop_assert_eq!(
            &result, &schema,
            "Identity sanitizer should return input unchanged.\nInput: {}\nOutput: {}",
            serde_json::to_string_pretty(&schema).unwrap(),
            serde_json::to_string_pretty(&result).unwrap()
        );
    }

    /// Property 4d: Identity sanitizer preserves arbitrary clean schemas.
    #[test]
    fn identity_sanitizer_preserves_clean_schemas(
        schema in clean_schema_strategy()
    ) {
        let sanitizer = IdentitySanitizer;
        let result = sanitizer.sanitize(&schema);

        prop_assert_eq!(
            &result, &schema,
            "Identity sanitizer should preserve clean schemas exactly."
        );
    }
}

// ── Property 5: Gemini Exclusive Bound Conversion ──────────────────

// Feature: phase-2-complete, Property 5: Gemini Exclusive Bound Conversion
// **Validates: Requirements 5.7, 5.8**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Property 5a: For any integer N, the Gemini sanitizer SHALL convert
    /// `exclusiveMinimum: N` to `minimum: N+1`.
    #[test]
    fn gemini_converts_exclusive_minimum_correctly(n in -10000i64..10000i64) {
        let schema = json!({
            "type": "integer",
            "exclusiveMinimum": n
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // exclusiveMinimum must be removed
        prop_assert!(
            result.get("exclusiveMinimum").is_none(),
            "exclusiveMinimum should be removed. Output: {}",
            serde_json::to_string_pretty(&result).unwrap()
        );

        // minimum must equal N+1
        let minimum = result.get("minimum")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| panic!("minimum field missing in output: {}", result));

        prop_assert_eq!(
            minimum, n + 1,
            "exclusiveMinimum: {} should convert to minimum: {}, got minimum: {}",
            n, n + 1, minimum
        );
    }

    /// Property 5b: For any integer N, the Gemini sanitizer SHALL convert
    /// `exclusiveMaximum: N` to `maximum: N-1`.
    #[test]
    fn gemini_converts_exclusive_maximum_correctly(n in -10000i64..10000i64) {
        let schema = json!({
            "type": "integer",
            "exclusiveMaximum": n
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // exclusiveMaximum must be removed
        prop_assert!(
            result.get("exclusiveMaximum").is_none(),
            "exclusiveMaximum should be removed. Output: {}",
            serde_json::to_string_pretty(&result).unwrap()
        );

        // maximum must equal N-1
        let maximum = result.get("maximum")
            .and_then(|v| v.as_i64())
            .unwrap_or_else(|| panic!("maximum field missing in output: {}", result));

        prop_assert_eq!(
            maximum, n - 1,
            "exclusiveMaximum: {} should convert to maximum: {}, got maximum: {}",
            n, n - 1, maximum
        );
    }

    /// Property 5c: For any integer N, both exclusiveMinimum and exclusiveMaximum
    /// are converted correctly when present together.
    #[test]
    fn gemini_converts_both_exclusive_bounds_correctly(
        min_val in -5000i64..5000i64,
        max_val in -5000i64..5000i64,
    ) {
        let schema = json!({
            "type": "integer",
            "exclusiveMinimum": min_val,
            "exclusiveMaximum": max_val
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // Both exclusive fields removed
        prop_assert!(result.get("exclusiveMinimum").is_none());
        prop_assert!(result.get("exclusiveMaximum").is_none());

        // Converted values are correct
        let minimum = result.get("minimum").and_then(|v| v.as_i64()).unwrap();
        let maximum = result.get("maximum").and_then(|v| v.as_i64()).unwrap();

        prop_assert_eq!(minimum, min_val + 1);
        prop_assert_eq!(maximum, max_val - 1);
    }

    /// Property 5d: For array-typed `type` fields containing a type and "null",
    /// the sanitizer SHALL produce the non-null type with `nullable: true`.
    #[test]
    fn gemini_converts_nullable_array_type_correctly(
        type_name in prop_oneof![
            Just("string"),
            Just("integer"),
            Just("number"),
            Just("boolean"),
            Just("object"),
            Just("array"),
        ]
    ) {
        let schema = json!({
            "type": [type_name, "null"],
            "description": "nullable field"
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // type must be a string (not array)
        let result_type = result.get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("type should be a string in output: {}", result));

        prop_assert_eq!(
            result_type, type_name,
            "type should be '{}', got '{}'", type_name, result_type
        );

        // nullable must be true
        let nullable = result.get("nullable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        prop_assert!(
            nullable,
            "nullable should be true for type: ['{type_name}', 'null']. Output: {}",
            serde_json::to_string_pretty(&result).unwrap()
        );
    }

    /// Property 5e: For array-typed `type` with null first (["null", type]),
    /// the sanitizer still produces the non-null type with nullable: true.
    #[test]
    fn gemini_converts_null_first_array_type_correctly(
        type_name in prop_oneof![
            Just("string"),
            Just("integer"),
            Just("number"),
            Just("boolean"),
            Just("object"),
            Just("array"),
        ]
    ) {
        let schema = json!({
            "type": ["null", type_name]
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // type must be the non-null type
        let result_type = result.get("type")
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("type should be a string in output: {}", result));

        prop_assert_eq!(result_type, type_name);

        // nullable must be true
        let nullable = result.get("nullable").and_then(|v| v.as_bool()).unwrap_or(false);
        prop_assert!(nullable, "nullable should be true");
    }
}

// ── Property 6: Schema Storage Immutability ────────────────────────

// Feature: phase-2-complete, Property 6: Schema Storage Immutability
// **Validates: Requirements 5.6**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Property 6a: Sanitizing a schema does not mutate the original input.
    /// The stored schema SHALL be byte-for-byte identical to the original
    /// schema received from the MCP server.
    #[test]
    fn gemini_sanitize_does_not_mutate_original(
        schema in schema_with_forbidden_fields_strategy()
    ) {
        // Clone the schema to have a reference copy
        let original = schema.clone();
        let original_serialized = serde_json::to_string(&original).unwrap();

        let sanitizer = GeminiSanitizer;
        // Sanitize — this should NOT modify the input
        let _result = sanitizer.sanitize(&schema);

        // The original schema must be unchanged
        let after_serialized = serde_json::to_string(&schema).unwrap();
        prop_assert_eq!(
            &original_serialized, &after_serialized,
            "Original schema was mutated by sanitize().\nBefore: {}\nAfter: {}",
            original_serialized, after_serialized
        );
    }

    /// Property 6b: Sanitizing a nested schema does not mutate the original.
    #[test]
    fn gemini_sanitize_does_not_mutate_nested_original(
        schema in nested_schema_strategy()
    ) {
        let original_serialized = serde_json::to_string(&schema).unwrap();

        let sanitizer = GeminiSanitizer;
        let _result = sanitizer.sanitize(&schema);

        let after_serialized = serde_json::to_string(&schema).unwrap();
        prop_assert_eq!(
            &original_serialized, &after_serialized,
            "Nested schema was mutated by sanitize()."
        );
    }

    /// Property 6c: Identity sanitizer also does not mutate the original.
    #[test]
    fn identity_sanitize_does_not_mutate_original(
        schema in schema_with_forbidden_fields_strategy()
    ) {
        let original_serialized = serde_json::to_string(&schema).unwrap();

        let sanitizer = IdentitySanitizer;
        let _result = sanitizer.sanitize(&schema);

        let after_serialized = serde_json::to_string(&schema).unwrap();
        prop_assert_eq!(
            &original_serialized, &after_serialized,
            "Schema was mutated by identity sanitize()."
        );
    }

    /// Property 6d: Multiple sanitizations of the same schema always produce
    /// the same result (idempotency of the transformation on the original).
    #[test]
    fn gemini_sanitize_is_deterministic(
        schema in schema_with_forbidden_fields_strategy()
    ) {
        let sanitizer = GeminiSanitizer;

        let result1 = sanitizer.sanitize(&schema);
        let result2 = sanitizer.sanitize(&schema);

        prop_assert_eq!(
            &result1, &result2,
            "Sanitize should be deterministic — same input should produce same output."
        );
    }
}
