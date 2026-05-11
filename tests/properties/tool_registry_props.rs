//! Property-based tests for tool registry and built-in tool resolution.
//!
//! Feature: gateway-production-maturity, Property 6: Tool name resolution correctness
//! **Validates: Requirements R3.1, R3.5, R3.6, R3.7, R17.1, R17.3, R17.4**

use adk_gateway::tool_registry::{BuiltInToolFactory, ToolRegistry};
use proptest::prelude::*;
use std::collections::HashSet;

/// Strategy that generates an arbitrary unknown tool name that is guaranteed
/// not to collide with any known built-in tool name.
fn arb_unknown_tool_name() -> impl Strategy<Value = String> {
    "[a-z_]{1,12}_unknown_[0-9]{1,4}"
}

/// Strategy that picks a random subset of known built-in tool names.
fn arb_known_names_subset() -> impl Strategy<Value = Vec<String>> {
    let known: Vec<&'static str> = BuiltInToolFactory::new().known_names();
    let len = known.len();
    prop::collection::vec(0..len, 0..=len).prop_map(move |indices| {
        let mut seen = HashSet::new();
        indices
            .into_iter()
            .filter(|i| seen.insert(*i))
            .map(|i| known[i].to_string())
            .collect()
    })
}

/// Strategy that generates a mixed list of known and unknown tool names.
fn arb_mixed_tool_names() -> impl Strategy<Value = (Vec<String>, Vec<String>, Vec<String>)> {
    (
        arb_known_names_subset(),
        prop::collection::vec(arb_unknown_tool_name(), 0..5),
    )
        .prop_map(|(known, unknown)| {
            let mut mixed = Vec::new();
            // Interleave known and unknown names
            let mut ki = known.iter();
            let mut ui = unknown.iter();
            loop {
                match (ki.next(), ui.next()) {
                    (Some(k), Some(u)) => {
                        mixed.push(k.clone());
                        mixed.push(u.clone());
                    }
                    (Some(k), None) => mixed.push(k.clone()),
                    (None, Some(u)) => mixed.push(u.clone()),
                    (None, None) => break,
                }
            }
            (mixed, known, unknown)
        })
}

// Feature: gateway-production-maturity, Property 6: Tool name resolution correctness
// **Validates: Requirements R3.1, R3.5, R3.6, R3.7, R17.1, R17.3, R17.4**
proptest! {
    #[test]
    fn tool_name_resolution_correctness(
        (mixed, known, unknown) in arb_mixed_tool_names()
    ) {
        let registry = ToolRegistry::new();
        let resolved = registry.resolve_tools(&mixed, None);

        // All known names should be resolved (R3.1, R17.1)
        prop_assert_eq!(
            resolved.len(),
            known.len(),
            "resolved count ({}) should match known count ({})",
            resolved.len(),
            known.len()
        );

        let resolved_names: Vec<&str> = resolved.iter().map(|t| t.name.as_str()).collect();
        let resolved_set: HashSet<&str> = resolved_names.iter().copied().collect();

        // Each known name appears in the resolved set (R3.5, R3.6)
        for name in &known {
            prop_assert!(
                resolved_set.contains(name.as_str()),
                "known tool '{}' should be resolved",
                name
            );
        }

        // No unknown name appears in the resolved set (R17.4 — skip unknown)
        for name in &unknown {
            prop_assert!(
                !resolved_set.contains(name.as_str()),
                "unknown tool '{}' should NOT be in resolved results",
                name
            );
        }

        // Resolved tools preserve order relative to the mixed input (R17.3)
        let mut expected_order: Vec<&str> = Vec::new();
        let known_set: HashSet<&str> = known.iter().map(|s| s.as_str()).collect();
        for name in &mixed {
            if known_set.contains(name.as_str()) {
                expected_order.push(name.as_str());
            }
        }
        prop_assert_eq!(
            resolved_names, expected_order,
            "resolved tools should preserve input order"
        );

        // Each resolved tool has a non-empty description
        for tool in &resolved {
            prop_assert!(
                !tool.description.is_empty(),
                "tool '{}' should have a non-empty description",
                tool.name
            );
        }
    }
}

// Feature: gateway-production-maturity, Property 7: Tool execution failure produces tool error result
// **Validates: Requirement R3.4**
proptest! {
    #[test]
    fn tool_execution_failure_produces_tool_error_result(
        tool_name in "[a-z_]{1,30}",
        error_msg in ".*"
    ) {
        let result = ToolRegistry::build_tool_error_result(&tool_name, &error_msg);

        // Result must be valid JSON (it's a serde_json::Value, so always valid,
        // but verify it round-trips through serialization)
        let serialized = serde_json::to_string(&result).expect("result should serialize to JSON");
        let parsed: serde_json::Value = serde_json::from_str(&serialized)
            .expect("serialized result should parse back as valid JSON");

        // The result must contain the tool name (R3.4)
        prop_assert_eq!(
            parsed["name"].as_str().unwrap(),
            tool_name.as_str(),
            "result 'name' field should match the tool name"
        );

        // The result must have the error flag set to true (R3.4)
        prop_assert_eq!(
            parsed["error"].as_bool().unwrap(),
            true,
            "result 'error' field should be true"
        );

        // The result must contain the error message content (R3.4)
        prop_assert_eq!(
            parsed["content"].as_str().unwrap(),
            error_msg.as_str(),
            "result 'content' field should match the error message"
        );
    }
}

// ── Property 9: Tool access check enforces role and scope requirements ──
// Feature: gateway-full-wiring, Property 9: Tool access check enforces role and scope requirements
// **Validates: Requirements 4.3, 4.4**

use adk_gateway::access_control::{ToolAccessCheck, ToolAccessDecision};

/// Strategy for generating a set of role names.
fn arb_role_set() -> impl Strategy<Value = HashSet<String>> {
    prop::collection::hash_set("[a-z_]{1,12}", 0..5)
}

/// Strategy for generating a set of scope strings.
fn arb_scope_set() -> impl Strategy<Value = HashSet<String>> {
    prop::collection::hash_set("[a-z]{1,8}:[a-z]{1,8}", 0..5)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 9: For any user with a set of roles and scopes, and a tool
    /// with required role and required scopes, ToolAccessCheck::check_tool_access
    /// should return Allowed if and only if the user holds the required role
    /// (when specified) AND holds all required scopes.
    #[test]
    fn tool_access_check_enforces_role_and_scope(
        user_roles in arb_role_set(),
        user_scopes in arb_scope_set(),
        tool_name in "[a-z_]{1,20}",
        required_role in prop::option::of("[a-z_]{1,12}"),
        required_scopes in prop::collection::vec("[a-z]{1,8}:[a-z]{1,8}", 0..4),
    ) {
        let decision = ToolAccessCheck::check_tool_access(
            &user_roles,
            &user_scopes,
            &tool_name,
            required_role.as_deref(),
            &required_scopes,
        );

        // Compute expected result
        let has_required_role = match &required_role {
            Some(role) => user_roles.contains(role.as_str()),
            None => true, // no role requirement means role check passes
        };

        let has_all_scopes = required_scopes.iter().all(|s| user_scopes.contains(s));

        let should_be_allowed = has_required_role && has_all_scopes;

        if should_be_allowed {
            prop_assert_eq!(decision, ToolAccessDecision::Allowed,
                "should allow when user has required role ({:?}) and all scopes ({:?}). \
                 user_roles={:?}, user_scopes={:?}",
                required_role, required_scopes, user_roles, user_scopes);
        } else {
            prop_assert!(matches!(decision, ToolAccessDecision::Denied { .. }),
                "should deny when user lacks required role or scopes. \
                 has_role={}, has_scopes={}, required_role={:?}, required_scopes={:?}, \
                 user_roles={:?}, user_scopes={:?}",
                has_required_role, has_all_scopes,
                required_role, required_scopes, user_roles, user_scopes);
        }
    }
}
