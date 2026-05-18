//! Property-based tests for Runner Max Iterations configuration.
//!
//! Feature: phase-2-complete, Property 7: Max Iterations Validation and Enforcement
//! Validates: Requirements 6.1, 6.2, 6.3, 6.5

use adk_gateway::config::{AgentEntry, ConfigError, GatewayRunnerConfig};
use proptest::prelude::*;

// ── Strategies ─────────────────────────────────────────────────────

/// Strategy for valid max_iterations values in [1, 1000].
fn arb_valid_max_iterations() -> impl Strategy<Value = u32> {
    1u32..=1000u32
}

/// Strategy for invalid max_iterations values outside [1, 1000].
fn arb_invalid_max_iterations() -> impl Strategy<Value = u32> {
    prop_oneof![
        Just(0u32),
        1001u32..=u32::MAX,
    ]
}

/// Strategy for an AgentEntry with an optional max_iterations override.
fn arb_agent_entry_with_override(max_iter: Option<u32>) -> impl Strategy<Value = AgentEntry> {
    "[a-zA-Z0-9_-]{1,20}".prop_map(move |id| AgentEntry {
        id,
        default: false,
        workspace: None,
        model: None,
        skills: vec![],
        browser: None,
        tools: vec![],
        max_iterations: max_iter,
        acp: None,
    })
}

// ── Property Tests ─────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    // Feature: phase-2-complete, Property 7: Max Iterations Validation and Enforcement
    // **Validates: Requirements 6.5**
    //
    // For any max_iterations value in [1, 1000], validate() SHALL accept it.
    #[test]
    fn valid_max_iterations_accepted(value in arb_valid_max_iterations()) {
        let config = GatewayRunnerConfig { max_iterations: value };
        let result = config.validate();
        prop_assert!(
            result.is_ok(),
            "Expected value {} in [1, 1000] to be accepted, but got error: {:?}",
            value, result
        );
    }

    // Feature: phase-2-complete, Property 7: Max Iterations Validation and Enforcement
    // **Validates: Requirements 6.5**
    //
    // For any max_iterations value outside [1, 1000], validate() SHALL reject it
    // with ConfigError::InvalidMaxIterations.
    #[test]
    fn invalid_max_iterations_rejected(value in arb_invalid_max_iterations()) {
        let config = GatewayRunnerConfig { max_iterations: value };
        let result = config.validate();
        prop_assert!(
            result.is_err(),
            "Expected value {} outside [1, 1000] to be rejected, but it was accepted",
            value
        );
        prop_assert_eq!(
            result.unwrap_err(),
            ConfigError::InvalidMaxIterations(value),
            "Expected InvalidMaxIterations error for value {}",
            value
        );
    }

    // Feature: phase-2-complete, Property 7: Max Iterations Validation and Enforcement
    // **Validates: Requirements 6.3**
    //
    // Per-request override (agent-level max_iterations) SHALL take precedence
    // over the gateway default. When an agent has a max_iterations override,
    // resolve_max_iterations returns the agent's value, not the gateway default.
    #[test]
    fn agent_override_takes_precedence(
        gateway_default in arb_valid_max_iterations(),
        agent_override in arb_valid_max_iterations(),
    ) {
        let config = GatewayRunnerConfig { max_iterations: gateway_default };
        let agent = AgentEntry {
            id: "test-agent".to_string(),
            default: false,
            workspace: None,
            model: None,
            skills: vec![],
            browser: None,
            tools: vec![],
            max_iterations: Some(agent_override),
            acp: None,
        };

        let resolved = config.resolve_max_iterations(Some(&agent));
        prop_assert_eq!(
            resolved, agent_override,
            "Expected agent override {} to take precedence over gateway default {}, got {}",
            agent_override, gateway_default, resolved
        );
    }

    // Feature: phase-2-complete, Property 7: Max Iterations Validation and Enforcement
    // **Validates: Requirements 6.3**
    //
    // When no agent override is set (max_iterations is None), resolve_max_iterations
    // SHALL return the gateway default.
    #[test]
    fn gateway_default_used_without_override(
        gateway_default in arb_valid_max_iterations(),
        agent_id in "[a-zA-Z0-9_-]{1,20}",
    ) {
        let config = GatewayRunnerConfig { max_iterations: gateway_default };
        let agent = AgentEntry {
            id: agent_id,
            default: false,
            workspace: None,
            model: None,
            skills: vec![],
            browser: None,
            tools: vec![],
            max_iterations: None,
            acp: None,
        };

        let resolved = config.resolve_max_iterations(Some(&agent));
        prop_assert_eq!(
            resolved, gateway_default,
            "Expected gateway default {} when agent has no override, got {}",
            gateway_default, resolved
        );
    }

    // Feature: phase-2-complete, Property 7: Max Iterations Validation and Enforcement
    // **Validates: Requirements 6.3**
    //
    // When no agent entry is provided (None), resolve_max_iterations SHALL return
    // the gateway default.
    #[test]
    fn gateway_default_used_without_agent(gateway_default in arb_valid_max_iterations()) {
        let config = GatewayRunnerConfig { max_iterations: gateway_default };

        let resolved = config.resolve_max_iterations(None);
        prop_assert_eq!(
            resolved, gateway_default,
            "Expected gateway default {} when no agent provided, got {}",
            gateway_default, resolved
        );
    }
}
