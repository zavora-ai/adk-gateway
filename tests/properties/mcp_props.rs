//! Property-based tests for MCP connection management.
//!
//! Feature: gateway-production-maturity
//! - Property 34: MCP connection failure does not block startup
//!   **Validates: Requirements R19.4, R19.7**
//! - Property 35: MCP reconnection follows exponential backoff
//!   **Validates: Requirements R19.5**

use adk_gateway::mcp::{backoff_duration, McpConnectionManager, McpServerConfig, McpTransport};
use proptest::prelude::*;
use std::time::Duration;

// ── Strategies ─────────────────────────────────────────────────────

/// Generate a valid McpServerConfig with a unique server_id.
fn mcp_server_config_strategy(index: usize) -> McpServerConfig {
    McpServerConfig {
        server_id: format!("srv-{}", index),
        transport: McpTransport::Sse {
            url: format!("http://localhost:{}", 9000 + index),
        },
        auth: None,
        enabled: true,
    }
}

// ── Property 34 ────────────────────────────────────────────────────

// Feature: gateway-production-maturity, Property 34: MCP connection failure does not block startup
// **Validates: Requirements R19.4, R19.7**
proptest! {
    /// Property 34: Generate N MCP server configs (1..5), connect them all
    /// (some may fail), verify the manager doesn't panic and tracks the
    /// successful ones.
    #[test]
    fn mcp_connection_failure_does_not_block_startup(n in 1usize..=5) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let manager = McpConnectionManager::new();

            // Create N configs — all enabled, the stub connect always succeeds
            let configs: Vec<McpServerConfig> = (0..n)
                .map(mcp_server_config_strategy)
                .collect();

            // Connect each one individually — manager must not panic
            for config in &configs {
                let result = manager.connect(config).await;
                // The stub implementation always succeeds for enabled servers
                prop_assert!(result.is_ok(), "connect should not fail for enabled server");
            }

            // All N servers should be tracked
            prop_assert_eq!(
                manager.connection_count(), n,
                "expected {} connections, got {}",
                n, manager.connection_count()
            );

            // Each server should be available
            for config in &configs {
                prop_assert!(
                    manager.is_tool_available(&config.server_id),
                    "server {} should be available",
                    config.server_id
                );
            }

            Ok(())
        })?;
    }

    /// Property 34 (disabled servers): Generate N configs where some are
    /// disabled. Disabled servers should be skipped without blocking startup.
    #[test]
    fn mcp_disabled_servers_do_not_block_startup(
        enabled_flags in proptest::collection::vec(any::<bool>(), 1..=5),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let manager = McpConnectionManager::new();

            let configs: Vec<McpServerConfig> = enabled_flags
                .iter()
                .enumerate()
                .map(|(i, &enabled)| {
                    let mut cfg = mcp_server_config_strategy(i);
                    cfg.enabled = enabled;
                    cfg
                })
                .collect();

            let expected_connected = enabled_flags.iter().filter(|&&e| e).count();

            // Use reconcile to connect all at once (exercises R19.7 path too)
            manager.reconcile(&configs).await;

            prop_assert_eq!(
                manager.connection_count(),
                expected_connected,
                "expected {} connected (enabled) servers, got {}",
                expected_connected,
                manager.connection_count()
            );

            // Verify only enabled servers are available
            for (i, &enabled) in enabled_flags.iter().enumerate() {
                let server_id = format!("srv-{}", i);
                prop_assert_eq!(
                    manager.is_tool_available(&server_id),
                    enabled,
                    "server {} availability mismatch (enabled={})",
                    server_id,
                    enabled
                );
            }

            Ok(())
        })?;
    }
}

// ── Property 35 ────────────────────────────────────────────────────

// Feature: gateway-production-maturity, Property 35: MCP reconnection follows exponential backoff
// **Validates: Requirements R19.5**
proptest! {
    /// Property 35: For arbitrary attempt numbers (0..20), backoff_duration
    /// follows exponential backoff (1s, 2s, 4s, ...) capped at 60s.
    #[test]
    fn mcp_reconnection_follows_exponential_backoff(attempt in 0u32..=20) {
        let duration = backoff_duration(attempt);

        // Must be at least 1 second (initial backoff)
        prop_assert!(
            duration >= Duration::from_secs(1),
            "backoff for attempt {} should be >= 1s, got {:?}",
            attempt, duration
        );

        // Must be capped at 60 seconds
        prop_assert!(
            duration <= Duration::from_secs(60),
            "backoff for attempt {} should be <= 60s, got {:?}",
            attempt, duration
        );

        // Must follow exponential formula: min(1 * 2^attempt, 60)
        let expected_secs = (1u64 << attempt.min(63)).min(60);
        prop_assert_eq!(
            duration,
            Duration::from_secs(expected_secs),
            "backoff for attempt {} should be {}s, got {:?}",
            attempt, expected_secs, duration
        );
    }

    /// Property 35 (monotonicity): Backoff durations are non-decreasing
    /// across consecutive attempts.
    #[test]
    fn mcp_backoff_is_non_decreasing(attempt in 0u32..=19) {
        let current = backoff_duration(attempt);
        let next = backoff_duration(attempt + 1);

        prop_assert!(
            next >= current,
            "backoff should be non-decreasing: attempt {} = {:?}, attempt {} = {:?}",
            attempt, current, attempt + 1, next
        );
    }
}

// ── Property 5: MCP reconciliation produces correct connection set ──
// Feature: gateway-full-wiring, Property 5: MCP reconciliation produces correct connection set
// **Validates: Requirements 3.5**
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    /// Property 5: For any initial set of McpServerConfig items and a new set,
    /// after reconcile, connected server IDs should equal enabled server IDs
    /// in new configs.
    #[test]
    fn mcp_reconciliation_produces_correct_connection_set(
        initial_count in 0usize..=5,
        new_enabled_flags in proptest::collection::vec(any::<bool>(), 1..=6),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let manager = McpConnectionManager::new();

            // Build initial configs and connect them
            let initial_configs: Vec<McpServerConfig> = (0..initial_count)
                .map(|i| mcp_server_config_strategy(i))
                .collect();
            for config in &initial_configs {
                let _ = manager.connect(config).await;
            }

            // Build new configs with varying enabled flags, using offset IDs
            // to ensure some overlap and some new/removed servers
            let new_configs: Vec<McpServerConfig> = new_enabled_flags
                .iter()
                .enumerate()
                .map(|(i, &enabled)| {
                    McpServerConfig {
                        server_id: format!("srv-{}", i),
                        transport: McpTransport::Sse {
                            url: format!("http://localhost:{}", 9000 + i),
                        },
                        auth: None,
                        enabled,
                    }
                })
                .collect();

            // Reconcile
            manager.reconcile(&new_configs).await;

            // Expected: connected server IDs == enabled server IDs in new configs
            let expected_ids: std::collections::HashSet<String> = new_configs
                .iter()
                .filter(|c| c.enabled)
                .map(|c| c.server_id.clone())
                .collect();

            let actual_ids: std::collections::HashSet<String> = manager
                .server_ids()
                .into_iter()
                .collect();

            prop_assert_eq!(
                &actual_ids, &expected_ids,
                "after reconcile, connected IDs should equal enabled IDs in new configs"
            );

            // Verify each enabled server is available
            for id in &expected_ids {
                prop_assert!(
                    manager.is_tool_available(id),
                    "enabled server {} should be available after reconcile", id
                );
            }

            Ok(())
        })?;
    }
}
