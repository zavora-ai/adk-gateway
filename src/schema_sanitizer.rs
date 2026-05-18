//! Provider-specific JSON Schema sanitization.
//!
//! Schemas from MCP tools are stored in their original form. At model invocation
//! time, the appropriate `SchemaSanitizer` transforms the schema for the target
//! provider (e.g., removing unsupported properties for Gemini).

/// Provider-specific schema transformation.
pub trait SchemaSanitizer: Send + Sync {
    /// Transform a JSON Schema for this provider. Returns the original if no changes needed.
    fn sanitize(&self, schema: &serde_json::Value) -> serde_json::Value;
}

/// Gemini-specific sanitizer that removes/transforms unsupported properties.
pub struct GeminiSanitizer;

impl GeminiSanitizer {
    /// Recursively walk a JSON value and apply Gemini-specific transformations:
    /// - Remove `propertyNames` keys from all objects
    /// - Convert `exclusiveMinimum: N` → `minimum: N+1` for integer types
    /// - Convert `exclusiveMaximum: N` → `maximum: N-1` for integer types
    /// - Convert array-typed `type` (e.g., `["string", "null"]`) → single type with `nullable: true`
    fn sanitize_recursive(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut new_map = serde_json::Map::new();

                // Handle array-typed `type` field: ["string", "null"] → type: "string", nullable: true
                let mut resolved_type: Option<String> = None;
                let mut add_nullable = false;

                if let Some(type_val) = map.get("type") {
                    if let Some(arr) = type_val.as_array() {
                        let has_null = arr.iter().any(|v| v.as_str() == Some("null"));
                        let non_null_types: Vec<&str> = arr
                            .iter()
                            .filter_map(|v| v.as_str())
                            .filter(|s| *s != "null")
                            .collect();

                        if has_null && !non_null_types.is_empty() {
                            resolved_type = Some(non_null_types[0].to_string());
                            add_nullable = true;
                        } else if !non_null_types.is_empty() {
                            // Array without null — use first type
                            resolved_type = Some(non_null_types[0].to_string());
                        }
                    }
                }

                // Check if this object represents an integer type schema
                let is_integer_type = if let Some(ref rt) = resolved_type {
                    rt == "integer"
                } else {
                    map.get("type")
                        .and_then(|v| v.as_str())
                        .map(|t| t == "integer")
                        .unwrap_or(false)
                };

                for (key, val) in map {
                    // Remove propertyNames
                    if key == "propertyNames" {
                        continue;
                    }

                    // Handle array-typed `type` — replace with resolved single type
                    if key == "type" && resolved_type.is_some() {
                        new_map.insert(
                            "type".to_string(),
                            serde_json::Value::String(resolved_type.as_ref().unwrap().clone()),
                        );
                        continue;
                    }

                    // Convert exclusiveMinimum: N → minimum: N+1 for integer types
                    if key == "exclusiveMinimum" && is_integer_type {
                        if let Some(n) = val.as_i64() {
                            new_map.insert(
                                "minimum".to_string(),
                                serde_json::Value::Number(serde_json::Number::from(n + 1)),
                            );
                        } else if let Some(n) = val.as_u64() {
                            new_map.insert(
                                "minimum".to_string(),
                                serde_json::Value::Number(serde_json::Number::from(n + 1)),
                            );
                        }
                        continue;
                    }

                    // Convert exclusiveMaximum: N → maximum: N-1 for integer types
                    if key == "exclusiveMaximum" && is_integer_type {
                        if let Some(n) = val.as_i64() {
                            new_map.insert(
                                "maximum".to_string(),
                                serde_json::Value::Number(serde_json::Number::from(n - 1)),
                            );
                        } else if let Some(n) = val.as_u64() {
                            new_map.insert(
                                "maximum".to_string(),
                                serde_json::Value::Number(serde_json::Number::from(
                                    n.saturating_sub(1),
                                )),
                            );
                        }
                        continue;
                    }

                    // Convert array-typed `items` to single schema (first element).
                    // Gemini doesn't support tuple validation (items as array of schemas).
                    if key == "items" {
                        if let Some(arr) = val.as_array() {
                            // items is an array — take the first element and recurse into it
                            if let Some(first) = arr.first() {
                                new_map
                                    .insert("items".to_string(), Self::sanitize_recursive(first));
                            }
                            // If the array is empty, omit items entirely
                        } else {
                            // items is already a single schema object — recurse into it
                            new_map.insert(key.clone(), Self::sanitize_recursive(val));
                        }
                        continue;
                    }

                    new_map.insert(key.clone(), Self::sanitize_recursive(val));
                }

                // Add nullable: true if the type array contained "null"
                if add_nullable {
                    new_map.insert("nullable".to_string(), serde_json::Value::Bool(true));
                }

                serde_json::Value::Object(new_map)
            }
            serde_json::Value::Array(arr) => {
                serde_json::Value::Array(arr.iter().map(Self::sanitize_recursive).collect())
            }
            other => other.clone(),
        }
    }
}

impl SchemaSanitizer for GeminiSanitizer {
    fn sanitize(&self, schema: &serde_json::Value) -> serde_json::Value {
        Self::sanitize_recursive(schema)
    }
}

/// Identity sanitizer for providers that accept standard JSON Schema (OpenAI, Anthropic).
pub struct IdentitySanitizer;

impl SchemaSanitizer for IdentitySanitizer {
    fn sanitize(&self, schema: &serde_json::Value) -> serde_json::Value {
        schema.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn removes_top_level_property_names() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            },
            "propertyNames": { "pattern": "^[a-z]+$" }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert!(result.get("propertyNames").is_none());
        assert_eq!(result.get("type").unwrap(), "object");
        assert!(result.get("properties").is_some());
    }

    #[test]
    fn removes_nested_property_names() {
        let schema = json!({
            "type": "object",
            "properties": {
                "metadata": {
                    "type": "object",
                    "properties": {
                        "tags": { "type": "string" }
                    },
                    "propertyNames": { "pattern": "^[a-z_]+$" }
                }
            },
            "propertyNames": { "pattern": "^[a-z]+$" }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // Top-level propertyNames removed
        assert!(result.get("propertyNames").is_none());
        // Nested propertyNames removed
        let metadata = &result["properties"]["metadata"];
        assert!(metadata.get("propertyNames").is_none());
        assert_eq!(metadata.get("type").unwrap(), "object");
    }

    #[test]
    fn removes_property_names_in_array_items() {
        let schema = json!({
            "type": "array",
            "items": {
                "type": "object",
                "propertyNames": { "pattern": "^[a-z]+$" },
                "properties": {
                    "id": { "type": "integer" }
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let items = result.get("items").unwrap();
        assert!(items.get("propertyNames").is_none());
        assert_eq!(items.get("type").unwrap(), "object");
    }

    #[test]
    fn preserves_schema_without_property_names() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer" }
            },
            "required": ["name"]
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert_eq!(result, schema);
    }

    #[test]
    fn converts_exclusive_minimum_to_minimum_for_integer() {
        let schema = json!({
            "type": "integer",
            "exclusiveMinimum": 5
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert!(result.get("exclusiveMinimum").is_none());
        assert_eq!(result.get("minimum").unwrap(), 6);
        assert_eq!(result.get("type").unwrap(), "integer");
    }

    #[test]
    fn converts_exclusive_minimum_zero_to_minimum_one() {
        let schema = json!({
            "type": "integer",
            "exclusiveMinimum": 0
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert!(result.get("exclusiveMinimum").is_none());
        assert_eq!(result.get("minimum").unwrap(), 1);
    }

    #[test]
    fn converts_negative_exclusive_minimum_for_integer() {
        let schema = json!({
            "type": "integer",
            "exclusiveMinimum": -1
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert!(result.get("exclusiveMinimum").is_none());
        assert_eq!(result.get("minimum").unwrap(), 0);
    }

    #[test]
    fn does_not_convert_exclusive_minimum_for_non_integer_types() {
        let schema = json!({
            "type": "number",
            "exclusiveMinimum": 5.0
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // For non-integer types, exclusiveMinimum is kept as-is (not converted)
        assert!(result.get("exclusiveMinimum").is_some());
        assert!(result.get("minimum").is_none());
    }

    #[test]
    fn converts_exclusive_minimum_in_nested_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "age": {
                    "type": "integer",
                    "exclusiveMinimum": 0
                },
                "name": {
                    "type": "string"
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let age = &result["properties"]["age"];
        assert!(age.get("exclusiveMinimum").is_none());
        assert_eq!(age.get("minimum").unwrap(), 1);
        assert_eq!(age.get("type").unwrap(), "integer");
    }

    #[test]
    fn converts_exclusive_minimum_in_array_items() {
        let schema = json!({
            "type": "array",
            "items": {
                "type": "integer",
                "exclusiveMinimum": 10
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let items = result.get("items").unwrap();
        assert!(items.get("exclusiveMinimum").is_none());
        assert_eq!(items.get("minimum").unwrap(), 11);
    }

    #[test]
    fn converts_exclusive_minimum_deeply_nested() {
        let schema = json!({
            "type": "object",
            "properties": {
                "level1": {
                    "type": "object",
                    "properties": {
                        "level2": {
                            "type": "object",
                            "properties": {
                                "count": {
                                    "type": "integer",
                                    "exclusiveMinimum": 99
                                }
                            }
                        }
                    }
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let count = &result["properties"]["level1"]["properties"]["level2"]["properties"]["count"];
        assert!(count.get("exclusiveMinimum").is_none());
        assert_eq!(count.get("minimum").unwrap(), 100);
    }

    #[test]
    fn preserves_existing_minimum_when_no_exclusive_minimum() {
        let schema = json!({
            "type": "integer",
            "minimum": 5
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert_eq!(result.get("minimum").unwrap(), 5);
        assert!(result.get("exclusiveMinimum").is_none());
    }

    #[test]
    fn handles_both_property_names_and_exclusive_minimum() {
        let schema = json!({
            "type": "object",
            "propertyNames": { "pattern": "^[a-z]+$" },
            "properties": {
                "count": {
                    "type": "integer",
                    "exclusiveMinimum": 0
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // propertyNames removed
        assert!(result.get("propertyNames").is_none());
        // exclusiveMinimum converted
        let count = &result["properties"]["count"];
        assert!(count.get("exclusiveMinimum").is_none());
        assert_eq!(count.get("minimum").unwrap(), 1);
    }

    #[test]
    fn handles_deeply_nested_property_names() {
        let schema = json!({
            "type": "object",
            "properties": {
                "level1": {
                    "type": "object",
                    "properties": {
                        "level2": {
                            "type": "object",
                            "properties": {
                                "level3": {
                                    "type": "object",
                                    "propertyNames": { "minLength": 1 }
                                }
                            },
                            "propertyNames": { "maxLength": 10 }
                        }
                    },
                    "propertyNames": { "pattern": ".*" }
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // All levels should have propertyNames removed
        assert!(result["properties"]["level1"]
            .get("propertyNames")
            .is_none());
        assert!(result["properties"]["level1"]["properties"]["level2"]
            .get("propertyNames")
            .is_none());
        assert!(
            result["properties"]["level1"]["properties"]["level2"]["properties"]["level3"]
                .get("propertyNames")
                .is_none()
        );
    }

    // --- exclusiveMaximum conversion tests ---

    #[test]
    fn converts_exclusive_maximum_to_maximum_for_integer() {
        let schema = json!({
            "type": "integer",
            "exclusiveMaximum": 10
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert!(result.get("exclusiveMaximum").is_none());
        assert_eq!(result.get("maximum").unwrap(), 9);
        assert_eq!(result.get("type").unwrap(), "integer");
    }

    #[test]
    fn converts_exclusive_maximum_one_to_maximum_zero() {
        let schema = json!({
            "type": "integer",
            "exclusiveMaximum": 1
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert!(result.get("exclusiveMaximum").is_none());
        assert_eq!(result.get("maximum").unwrap(), 0);
    }

    #[test]
    fn converts_negative_exclusive_maximum_for_integer() {
        let schema = json!({
            "type": "integer",
            "exclusiveMaximum": -1
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert!(result.get("exclusiveMaximum").is_none());
        assert_eq!(result.get("maximum").unwrap(), -2);
    }

    #[test]
    fn does_not_convert_exclusive_maximum_for_non_integer_types() {
        let schema = json!({
            "type": "number",
            "exclusiveMaximum": 10.0
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // For non-integer types, exclusiveMaximum is kept as-is (not converted)
        assert!(result.get("exclusiveMaximum").is_some());
        assert!(result.get("maximum").is_none());
    }

    #[test]
    fn converts_exclusive_maximum_in_nested_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "count": {
                    "type": "integer",
                    "exclusiveMaximum": 100
                },
                "name": {
                    "type": "string"
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let count = &result["properties"]["count"];
        assert!(count.get("exclusiveMaximum").is_none());
        assert_eq!(count.get("maximum").unwrap(), 99);
        assert_eq!(count.get("type").unwrap(), "integer");
    }

    #[test]
    fn converts_exclusive_maximum_in_array_items() {
        let schema = json!({
            "type": "array",
            "items": {
                "type": "integer",
                "exclusiveMaximum": 50
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let items = result.get("items").unwrap();
        assert!(items.get("exclusiveMaximum").is_none());
        assert_eq!(items.get("maximum").unwrap(), 49);
    }

    #[test]
    fn converts_exclusive_maximum_deeply_nested() {
        let schema = json!({
            "type": "object",
            "properties": {
                "level1": {
                    "type": "object",
                    "properties": {
                        "level2": {
                            "type": "object",
                            "properties": {
                                "limit": {
                                    "type": "integer",
                                    "exclusiveMaximum": 1000
                                }
                            }
                        }
                    }
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let limit = &result["properties"]["level1"]["properties"]["level2"]["properties"]["limit"];
        assert!(limit.get("exclusiveMaximum").is_none());
        assert_eq!(limit.get("maximum").unwrap(), 999);
    }

    #[test]
    fn preserves_existing_maximum_when_no_exclusive_maximum() {
        let schema = json!({
            "type": "integer",
            "maximum": 100
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert_eq!(result.get("maximum").unwrap(), 100);
        assert!(result.get("exclusiveMaximum").is_none());
    }

    #[test]
    fn handles_both_exclusive_minimum_and_exclusive_maximum() {
        let schema = json!({
            "type": "integer",
            "exclusiveMinimum": 0,
            "exclusiveMaximum": 100
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert!(result.get("exclusiveMinimum").is_none());
        assert!(result.get("exclusiveMaximum").is_none());
        assert_eq!(result.get("minimum").unwrap(), 1);
        assert_eq!(result.get("maximum").unwrap(), 99);
    }

    #[test]
    fn handles_property_names_and_exclusive_maximum() {
        let schema = json!({
            "type": "object",
            "propertyNames": { "pattern": "^[a-z]+$" },
            "properties": {
                "limit": {
                    "type": "integer",
                    "exclusiveMaximum": 50
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // propertyNames removed
        assert!(result.get("propertyNames").is_none());
        // exclusiveMaximum converted
        let limit = &result["properties"]["limit"];
        assert!(limit.get("exclusiveMaximum").is_none());
        assert_eq!(limit.get("maximum").unwrap(), 49);
    }

    // --- array-typed `type` conversion tests ---

    #[test]
    fn converts_string_null_array_type_to_nullable() {
        let schema = json!({
            "type": ["string", "null"],
            "description": "An optional name"
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert_eq!(result.get("type").unwrap(), "string");
        assert_eq!(result.get("nullable").unwrap(), true);
        assert_eq!(result.get("description").unwrap(), "An optional name");
    }

    #[test]
    fn converts_integer_null_array_type_to_nullable() {
        let schema = json!({
            "type": ["integer", "null"]
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert_eq!(result.get("type").unwrap(), "integer");
        assert_eq!(result.get("nullable").unwrap(), true);
    }

    #[test]
    fn converts_null_first_in_array_type_to_nullable() {
        let schema = json!({
            "type": ["null", "boolean"]
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert_eq!(result.get("type").unwrap(), "boolean");
        assert_eq!(result.get("nullable").unwrap(), true);
    }

    #[test]
    fn converts_multiple_non_null_types_uses_first() {
        let schema = json!({
            "type": ["string", "integer", "null"]
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // Uses first non-null type
        assert_eq!(result.get("type").unwrap(), "string");
        assert_eq!(result.get("nullable").unwrap(), true);
    }

    #[test]
    fn converts_array_type_without_null_uses_first_type() {
        let schema = json!({
            "type": ["string", "integer"]
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // No null in array — use first type, no nullable
        assert_eq!(result.get("type").unwrap(), "string");
        assert!(result.get("nullable").is_none());
    }

    #[test]
    fn converts_array_type_in_nested_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": ["string", "null"],
                    "description": "User name"
                },
                "age": {
                    "type": ["integer", "null"]
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let name = &result["properties"]["name"];
        assert_eq!(name.get("type").unwrap(), "string");
        assert_eq!(name.get("nullable").unwrap(), true);

        let age = &result["properties"]["age"];
        assert_eq!(age.get("type").unwrap(), "integer");
        assert_eq!(age.get("nullable").unwrap(), true);
    }

    #[test]
    fn converts_array_type_in_array_items() {
        let schema = json!({
            "type": "array",
            "items": {
                "type": ["number", "null"]
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let items = result.get("items").unwrap();
        assert_eq!(items.get("type").unwrap(), "number");
        assert_eq!(items.get("nullable").unwrap(), true);
    }

    #[test]
    fn converts_array_type_deeply_nested() {
        let schema = json!({
            "type": "object",
            "properties": {
                "level1": {
                    "type": "object",
                    "properties": {
                        "level2": {
                            "type": "object",
                            "properties": {
                                "value": {
                                    "type": ["string", "null"]
                                }
                            }
                        }
                    }
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let value = &result["properties"]["level1"]["properties"]["level2"]["properties"]["value"];
        assert_eq!(value.get("type").unwrap(), "string");
        assert_eq!(value.get("nullable").unwrap(), true);
    }

    #[test]
    fn preserves_string_type_unchanged() {
        let schema = json!({
            "type": "string",
            "description": "A regular string"
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert_eq!(result.get("type").unwrap(), "string");
        assert!(result.get("nullable").is_none());
    }

    #[test]
    fn handles_array_type_with_exclusive_bounds() {
        let schema = json!({
            "type": ["integer", "null"],
            "exclusiveMinimum": 0,
            "exclusiveMaximum": 100
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert_eq!(result.get("type").unwrap(), "integer");
        assert_eq!(result.get("nullable").unwrap(), true);
        assert_eq!(result.get("minimum").unwrap(), 1);
        assert_eq!(result.get("maximum").unwrap(), 99);
        assert!(result.get("exclusiveMinimum").is_none());
        assert!(result.get("exclusiveMaximum").is_none());
    }

    // --- array-typed `items` conversion tests ---

    #[test]
    fn converts_array_items_to_first_element() {
        let schema = json!({
            "type": "array",
            "items": [
                { "type": "string" },
                { "type": "integer" }
            ]
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let items = result.get("items").unwrap();
        assert!(items.is_object());
        assert_eq!(items.get("type").unwrap(), "string");
    }

    #[test]
    fn preserves_single_object_items_unchanged() {
        let schema = json!({
            "type": "array",
            "items": {
                "type": "string",
                "description": "A name"
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let items = result.get("items").unwrap();
        assert!(items.is_object());
        assert_eq!(items.get("type").unwrap(), "string");
        assert_eq!(items.get("description").unwrap(), "A name");
    }

    #[test]
    fn recurses_into_single_object_items() {
        let schema = json!({
            "type": "array",
            "items": {
                "type": "object",
                "propertyNames": { "pattern": "^[a-z]+$" },
                "properties": {
                    "id": { "type": "integer" }
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let items = result.get("items").unwrap();
        // propertyNames should be removed by recursion
        assert!(items.get("propertyNames").is_none());
        assert_eq!(items["properties"]["id"]["type"], "integer");
    }

    #[test]
    fn recurses_into_first_element_of_array_items() {
        let schema = json!({
            "type": "array",
            "items": [
                {
                    "type": "object",
                    "propertyNames": { "pattern": "^[a-z]+$" },
                    "properties": {
                        "name": { "type": "string" }
                    }
                },
                { "type": "integer" }
            ]
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let items = result.get("items").unwrap();
        assert!(items.is_object());
        // propertyNames should be removed from the first element
        assert!(items.get("propertyNames").is_none());
        assert_eq!(items["properties"]["name"]["type"], "string");
    }

    #[test]
    fn converts_array_items_in_nested_properties() {
        let schema = json!({
            "type": "object",
            "properties": {
                "tags": {
                    "type": "array",
                    "items": [
                        { "type": "string" },
                        { "type": "number" }
                    ]
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let items = &result["properties"]["tags"]["items"];
        assert!(items.is_object());
        assert_eq!(items.get("type").unwrap(), "string");
    }

    #[test]
    fn converts_array_items_deeply_nested() {
        let schema = json!({
            "type": "object",
            "properties": {
                "level1": {
                    "type": "object",
                    "properties": {
                        "data": {
                            "type": "array",
                            "items": [
                                { "type": "integer", "exclusiveMinimum": 0 },
                                { "type": "string" }
                            ]
                        }
                    }
                }
            }
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let items = &result["properties"]["level1"]["properties"]["data"]["items"];
        assert!(items.is_object());
        assert_eq!(items.get("type").unwrap(), "integer");
        // exclusiveMinimum should also be converted in the first element
        assert!(items.get("exclusiveMinimum").is_none());
        assert_eq!(items.get("minimum").unwrap(), 1);
    }

    #[test]
    fn handles_empty_array_items() {
        let schema = json!({
            "type": "array",
            "items": []
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        // Empty array items — items key should be omitted
        assert!(result.get("items").is_none());
    }

    #[test]
    fn handles_array_items_with_single_element() {
        let schema = json!({
            "type": "array",
            "items": [
                { "type": "boolean" }
            ]
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let items = result.get("items").unwrap();
        assert!(items.is_object());
        assert_eq!(items.get("type").unwrap(), "boolean");
    }

    #[test]
    fn handles_array_items_with_nullable_type_in_first_element() {
        let schema = json!({
            "type": "array",
            "items": [
                { "type": ["string", "null"] },
                { "type": "integer" }
            ]
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        let items = result.get("items").unwrap();
        assert!(items.is_object());
        // The first element's array type should also be converted
        assert_eq!(items.get("type").unwrap(), "string");
        assert_eq!(items.get("nullable").unwrap(), true);
    }

    #[test]
    fn preserves_other_properties_when_converting_array_items() {
        let schema = json!({
            "type": "array",
            "minItems": 1,
            "maxItems": 10,
            "items": [
                { "type": "string" },
                { "type": "integer" }
            ]
        });

        let sanitizer = GeminiSanitizer;
        let result = sanitizer.sanitize(&schema);

        assert_eq!(result.get("type").unwrap(), "array");
        assert_eq!(result.get("minItems").unwrap(), 1);
        assert_eq!(result.get("maxItems").unwrap(), 10);
        let items = result.get("items").unwrap();
        assert!(items.is_object());
        assert_eq!(items.get("type").unwrap(), "string");
    }

    // --- IdentitySanitizer tests ---

    #[test]
    fn identity_sanitizer_returns_schema_unchanged() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "age": { "type": "integer", "exclusiveMinimum": 0 },
                "tags": { "type": ["string", "null"] }
            },
            "propertyNames": { "pattern": "^[a-z]+$" },
            "required": ["name"]
        });

        let sanitizer = IdentitySanitizer;
        let result = sanitizer.sanitize(&schema);

        // IdentitySanitizer should return an exact clone — no transformations
        assert_eq!(result, schema);
    }
}
