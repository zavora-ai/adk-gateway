//! Property-based tests for Tool Approval Decision Correctness.
//!
//! Feature: phase-2-complete, Property 1: Tool Approval Decision Correctness
//! Validates: Requirements 1.5, 1.6

use adk_gateway::config::ApprovalConfig;
use adk_gateway::tool_approval::{
    check_requires_approval, matches_pattern, ApprovalDecision, DEFAULT_DANGEROUS_TOOLS,
};
use proptest::prelude::*;

// ── Strategies ─────────────────────────────────────────────────────

/// Strategy for generating valid tool names (alphanumeric + underscores).
fn arb_tool_name() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,20}".prop_map(|s| s)
}

/// Strategy for generating a tool name that is one of the default dangerous tools.
fn arb_default_dangerous_tool() -> impl Strategy<Value = String> {
    prop::sample::select(
        DEFAULT_DANGEROUS_TOOLS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
    )
}

/// Strategy for generating a tool name that is NOT one of the default dangerous tools.
fn arb_safe_tool_name() -> impl Strategy<Value = String> {
    arb_tool_name().prop_filter("must not be a default dangerous tool", |name| {
        !DEFAULT_DANGEROUS_TOOLS.contains(&name.as_str())
    })
}

/// Strategy for generating custom approval rules (non-empty list of patterns).
fn arb_custom_rules() -> impl Strategy<Value = Vec<String>> {
    proptest::collection::vec(arb_tool_name(), 1..=5)
}

/// Strategy for generating an ApprovalConfig with custom rules.
fn arb_custom_config() -> impl Strategy<Value = ApprovalConfig> {
    (arb_custom_rules(), 30u64..=300u64).prop_map(|(rules, timeout)| ApprovalConfig {
        require_approval: rules,
        timeout_secs: timeout,
    })
}

/// Strategy for generating an ApprovalConfig with empty rules (uses defaults).
fn arb_default_config() -> impl Strategy<Value = ApprovalConfig> {
    (30u64..=300u64).prop_map(|timeout| ApprovalConfig {
        require_approval: vec![],
        timeout_secs: timeout,
    })
}

/// Strategy for generating a pattern with optional wildcards.
fn arb_pattern() -> impl Strategy<Value = String> {
    prop_oneof![
        // Exact match pattern
        arb_tool_name(),
        // Prefix wildcard: "prefix*"
        "[a-z]{1,5}".prop_map(|s| format!("{}*", s)),
        // Suffix wildcard: "*suffix"
        "[a-z]{1,5}".prop_map(|s| format!("*{}", s)),
        // Contains wildcard: "*middle*"
        "[a-z]{1,5}".prop_map(|s| format!("*{}*", s)),
    ]
}

// ── Property Tests ─────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    // Feature: phase-2-complete, Property 1: Tool Approval Decision Correctness
    // **Validates: Requirements 1.5**
    //
    // For any tool name that is in the default dangerous categories,
    // `requires_approval` SHALL return `Required` when no custom rules are configured.
    #[test]
    fn default_dangerous_tools_require_approval(
        tool_name in arb_default_dangerous_tool(),
        config in arb_default_config(),
    ) {
        let decision = check_requires_approval(&tool_name, &config);
        prop_assert_eq!(
            decision,
            ApprovalDecision::Required,
            "Default dangerous tool '{}' should require approval with default config",
            tool_name
        );
    }

    // Feature: phase-2-complete, Property 1: Tool Approval Decision Correctness
    // **Validates: Requirements 1.5**
    //
    // For any tool name that is NOT in the default dangerous categories,
    // `requires_approval` SHALL return `NotRequired` when no custom rules are configured.
    #[test]
    fn safe_tools_do_not_require_approval_with_defaults(
        tool_name in arb_safe_tool_name(),
        config in arb_default_config(),
    ) {
        let decision = check_requires_approval(&tool_name, &config);
        prop_assert_eq!(
            decision,
            ApprovalDecision::NotRequired,
            "Safe tool '{}' should not require approval with default config",
            tool_name
        );
    }

    // Feature: phase-2-complete, Property 1: Tool Approval Decision Correctness
    // **Validates: Requirements 1.6**
    //
    // When custom rules are configured, they SHALL take complete precedence over defaults.
    // A tool that matches a custom rule SHALL require approval.
    #[test]
    fn custom_rules_match_requires_approval(
        tool_name in arb_tool_name(),
        timeout in 30u64..=300u64,
    ) {
        // Create a config where the tool_name is explicitly in the rules
        let config = ApprovalConfig {
            require_approval: vec![tool_name.clone()],
            timeout_secs: timeout,
        };

        let decision = check_requires_approval(&tool_name, &config);
        prop_assert_eq!(
            decision,
            ApprovalDecision::Required,
            "Tool '{}' should require approval when it's in custom rules",
            tool_name
        );
    }

    // Feature: phase-2-complete, Property 1: Tool Approval Decision Correctness
    // **Validates: Requirements 1.6**
    //
    // When custom rules are configured, default dangerous tools that are NOT in
    // the custom rules SHALL NOT require approval (custom rules override defaults).
    #[test]
    fn custom_rules_override_defaults(
        dangerous_tool in arb_default_dangerous_tool(),
        custom_rule in arb_safe_tool_name(),
        timeout in 30u64..=300u64,
    ) {
        // Custom rules contain only a safe tool name, not the dangerous tool
        let config = ApprovalConfig {
            require_approval: vec![custom_rule.clone()],
            timeout_secs: timeout,
        };

        // The dangerous tool should NOT require approval because custom rules override
        let decision = check_requires_approval(&dangerous_tool, &config);
        prop_assert_eq!(
            decision,
            ApprovalDecision::NotRequired,
            "Default dangerous tool '{}' should NOT require approval when custom rules '{}' override defaults",
            dangerous_tool, custom_rule
        );
    }

    // Feature: phase-2-complete, Property 1: Tool Approval Decision Correctness
    // **Validates: Requirements 1.5, 1.6**
    //
    // For any tool name and any config, the decision is Required if and only if
    // the tool matches a pattern in the active rule set.
    #[test]
    fn decision_iff_matches_active_rules(
        tool_name in arb_tool_name(),
        config in prop_oneof![arb_default_config(), arb_custom_config()],
    ) {
        let decision = check_requires_approval(&tool_name, &config);

        // Determine the active rule set
        let matches_active = if !config.require_approval.is_empty() {
            // Custom rules active
            config.require_approval.iter().any(|pattern| matches_pattern(&tool_name, pattern))
        } else {
            // Default rules active
            DEFAULT_DANGEROUS_TOOLS.iter().any(|&default| matches_pattern(&tool_name, default))
        };

        if matches_active {
            prop_assert_eq!(
                decision,
                ApprovalDecision::Required,
                "Tool '{}' matches active rules but got NotRequired (config: {:?})",
                tool_name, config.require_approval
            );
        } else {
            prop_assert_eq!(
                decision,
                ApprovalDecision::NotRequired,
                "Tool '{}' does NOT match active rules but got Required (config: {:?})",
                tool_name, config.require_approval
            );
        }
    }

    // Feature: phase-2-complete, Property 1: Tool Approval Decision Correctness
    // **Validates: Requirements 1.6**
    //
    // Pattern matching correctness: prefix wildcard patterns match tool names
    // that start with the prefix.
    #[test]
    fn prefix_wildcard_matches_correctly(
        prefix in "[a-z]{1,4}",
        suffix in "[a-z_]{1,8}",
    ) {
        let tool_name = format!("{}{}", prefix, suffix);
        let pattern = format!("{}*", prefix);

        prop_assert!(
            matches_pattern(&tool_name, &pattern),
            "Tool '{}' should match prefix pattern '{}'",
            tool_name, pattern
        );
    }

    // Feature: phase-2-complete, Property 1: Tool Approval Decision Correctness
    // **Validates: Requirements 1.6**
    //
    // Pattern matching correctness: suffix wildcard patterns match tool names
    // that end with the suffix.
    #[test]
    fn suffix_wildcard_matches_correctly(
        prefix in "[a-z]{1,4}",
        suffix in "[a-z]{1,4}",
    ) {
        let tool_name = format!("{}{}", prefix, suffix);
        let pattern = format!("*{}", suffix);

        prop_assert!(
            matches_pattern(&tool_name, &pattern),
            "Tool '{}' should match suffix pattern '{}'",
            tool_name, pattern
        );
    }

    // Feature: phase-2-complete, Property 1: Tool Approval Decision Correctness
    // **Validates: Requirements 1.6**
    //
    // Pattern matching correctness: exact patterns only match the exact tool name.
    #[test]
    fn exact_pattern_only_matches_exact_name(
        tool_name in arb_tool_name(),
        other_name in arb_tool_name(),
    ) {
        // Exact match: tool matches itself
        prop_assert!(
            matches_pattern(&tool_name, &tool_name),
            "Tool '{}' should match its own exact pattern",
            tool_name
        );

        // If names differ, they should not match
        if tool_name != other_name {
            prop_assert!(
                !matches_pattern(&tool_name, &other_name),
                "Tool '{}' should NOT match different exact pattern '{}'",
                tool_name, other_name
            );
        }
    }
}
