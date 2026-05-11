//! Property-based tests for ProcessManager.
//!
//! Feature: multi-agent-isolation
//! Property 7 (Process cleanup — stopped agent releases port)

use adk_gateway::process_manager::*;
use proptest::prelude::*;
use std::time::Duration;

// ── Helpers ────────────────────────────────────────────────────────

fn arb_agent_id() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9-]{0,9}"
}

/// Generate a small port range and a set of agent IDs to allocate/stop.
fn arb_port_scenario() -> impl Strategy<Value = (u16, u16, Vec<String>)> {
    // start_offset in 19001..19050, range_size 5..20, agent IDs 1..10
    (
        0u16..50,
        5u16..20,
        prop::collection::vec(arb_agent_id(), 1..10),
    )
        .prop_map(|(offset, size, ids)| {
            let start = 19001 + offset;
            let end = start + size;
            (start, end, ids)
        })
}

// ── Property 7: Process cleanup — stopped agent releases port ──────
// **Validates: Requirements 6.2**
//
// Allocate ports for agents, then "stop" them (remove from processes map),
// and verify the port is no longer in the active set. Since we can't spawn
// real processes in property tests, we test the port allocation/release logic
// directly.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn stopped_agent_releases_port(
        (start, end, agent_ids) in arb_port_scenario()
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let pm = ProcessManager::new(start..=end, Duration::from_secs(30));

            // Deduplicate agent IDs.
            let mut unique_ids: Vec<String> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for id in &agent_ids {
                if seen.insert(id.clone()) {
                    unique_ids.push(id.clone());
                }
            }

            // Limit to available ports.
            let max_agents = (end - start + 1) as usize;
            let ids_to_use: Vec<String> = unique_ids.into_iter().take(max_agents).collect();

            // Allocate ports and insert fake processes.
            let mut allocated: Vec<(String, u16)> = Vec::new();
            for id in &ids_to_use {
                let port = pm.allocate_port().unwrap();

                // Spawn a trivial child process (sleep) to satisfy the ManagedProcess struct.
                let child = tokio::process::Command::new("sleep")
                    .arg("300")
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .expect("failed to spawn sleep");

                let process = ManagedProcess {
                    agent_id: id.clone(),
                    port,
                    child,
                    started_at: chrono::Utc::now(),
                    health: HealthStatus::Unknown,
                };
                pm.insert_process(process);
                allocated.push((id.clone(), port));
            }

            // Verify all allocated ports are active.
            let active_before: std::collections::HashSet<u16> =
                pm.active_ports().into_iter().collect();
            for (_, port) in &allocated {
                prop_assert!(
                    active_before.contains(port),
                    "port {} should be active before stop",
                    port
                );
            }

            // Stop each agent (remove from map) and verify port is released.
            for (id, port) in &allocated {
                pm.remove_process(id).await;

                prop_assert!(
                    !pm.is_port_active(*port),
                    "port {} should be released after stopping agent '{}'",
                    port,
                    id
                );
                prop_assert!(
                    !pm.has_agent(id),
                    "agent '{}' should not be in process manager after stop",
                    id
                );
            }

            // After all agents stopped, no ports should be active.
            prop_assert_eq!(
                pm.active_ports().len(),
                0,
                "all ports should be released after stopping all agents"
            );
            prop_assert_eq!(
                pm.process_count(),
                0,
                "process count should be 0 after stopping all agents"
            );

            Ok(())
        })?;
    }
}
