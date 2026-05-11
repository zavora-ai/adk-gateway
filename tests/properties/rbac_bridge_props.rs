//! Property-based tests for RbacBridge.
//!
//! Feature: multi-agent-isolation
//! - Property 4: Permission isolation — user agent never passes system tool check
//!   **Validates: Requirements 5, 10**

use adk_gateway::agent_config::AgentRoleConfig;
use adk_gateway::rbac_bridge::{RbacBridge, SYSTEM_TOOLS};
use proptest::prelude::*;

// ── Strategies ─────────────────────────────────────────────────────

/// Generate an arbitrary tool name (may or may not overlap with system tools).
fn arb_tool_name() -> impl Strategy<Value = String> {
    prop_oneof![
        // Random user tool names
        "[a-z_]{1,15}".prop_map(|s| s.to_string()),
        // Occasionally include system tool names to test stripping
        prop::sample::select(
            SYSTEM_TOOLS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        ),
    ]
}

/// Generate an arbitrary AgentRoleConfig with random allow/deny lists.
fn arb_role_config() -> impl Strategy<Value = AgentRoleConfig> {
    (
        prop::collection::vec(arb_tool_name(), 0..10),
        prop::collection::vec(arb_tool_name(), 0..5),
    )
        .prop_map(|(allow, deny)| AgentRoleConfig { allow, deny })
}

fn arb_agent_id() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{0,9}"
}

// ── Property 4: Permission isolation ───────────────────────────────

proptest! {
    /// **Validates: Requirements 5, 10**
    ///
    /// For any user agent with any role configuration, the agent must
    /// NEVER pass a check_tool call for any of the 6 system tools.
    /// System tools are always stripped during registration.
    #[test]
    fn user_agent_never_passes_system_tool_check(
        agent_id in arb_agent_id(),
        role_config in arb_role_config(),
    ) {
        let bridge = RbacBridge::new();
        let _stripped = bridge.register_agent(&agent_id, &role_config);

        for system_tool in SYSTEM_TOOLS {
            let result = bridge.check_tool(&agent_id, system_tool);
            prop_assert!(
                result.is_err(),
                "user agent '{}' should NEVER have access to system tool '{}', \
                 but check_tool returned Ok. Role config allow: {:?}",
                agent_id,
                system_tool,
                role_config.allow,
            );
        }
    }
}
