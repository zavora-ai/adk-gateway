//! Process manager for child agent process lifecycle.
//!
//! Manages spawning, stopping, health-checking, and port allocation for
//! agent binaries running as child processes.

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::path::Path;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use tokio::process::{Child, Command};
use tracing::{debug, error, info, warn};

/// Default port range for agent processes.
pub const DEFAULT_PORT_RANGE: RangeInclusive<u16> = 19001..=19100;

/// Health status of a managed agent process.
#[derive(Debug, Clone, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Unhealthy { consecutive_failures: u32 },
    Unknown,
}

/// A managed child process for an agent binary.
pub struct ManagedProcess {
    /// Agent identifier, used as the DashMap key during insertion.
    #[allow(dead_code)]
    pub agent_id: String,
    pub port: u16,
    pub child: Child,
    /// Timestamp when the process was started. Used by monitoring and diagnostics.
    #[allow(dead_code)]
    pub started_at: DateTime<Utc>,
    pub health: HealthStatus,
}

/// Manages child process lifecycle: spawn, stop, health-check, port allocation.
pub struct ProcessManager {
    processes: DashMap<String, ManagedProcess>,
    port_range: RangeInclusive<u16>,
    next_port: AtomicU16,
    health_interval: Duration,
}

impl ProcessManager {
    /// Create a new ProcessManager with the given port range and health check interval.
    pub fn new(port_range: RangeInclusive<u16>, health_interval: Duration) -> Self {
        let start = *port_range.start();
        Self {
            processes: DashMap::new(),
            port_range,
            next_port: AtomicU16::new(start),
            health_interval,
        }
    }

    /// Create a ProcessManager with default settings (ports 19001..=19100, 30s health interval).
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_PORT_RANGE, Duration::from_secs(30))
    }

    /// Allocate the next available port. Skips ports that are currently in use
    /// by active processes.
    pub fn allocate_port(&self) -> Result<u16> {
        let range_len = (*self.port_range.end() - *self.port_range.start() + 1) as usize;

        for _ in 0..range_len {
            let port = self.next_port.fetch_add(1, Ordering::SeqCst);

            // Wrap around if we exceed the range.
            if port > *self.port_range.end() {
                self.next_port
                    .store(*self.port_range.start(), Ordering::SeqCst);
                continue;
            }

            // Check if port is already in use by an active process.
            if !self.is_port_active(port) {
                // Verify the port is actually available on the system
                match std::net::TcpListener::bind(("127.0.0.1", port)) {
                    Ok(_listener) => {
                        // Port is free — listener is dropped, releasing the bind
                        return Ok(port);
                    }
                    Err(_) => {
                        tracing::debug!(port = port, "port in use by another process, skipping");
                        continue;
                    }
                }
            }
        }

        bail!(
            "no available ports in range {}..={}",
            self.port_range.start(),
            self.port_range.end()
        )
    }

    /// Check if a port is currently used by an active managed process.
    pub fn is_port_active(&self, port: u16) -> bool {
        self.processes
            .iter()
            .any(|entry| entry.value().port == port)
    }

    /// Return the set of all ports currently in use by active processes.
    /// Used by integration tests and monitoring.
    #[allow(dead_code)]
    pub fn active_ports(&self) -> Vec<u16> {
        self.processes
            .iter()
            .map(|entry| entry.value().port)
            .collect()
    }

    /// Return the number of currently managed processes.
    /// Used by integration tests and monitoring.
    #[allow(dead_code)]
    pub fn process_count(&self) -> usize {
        self.processes.len()
    }

    /// Check if an agent is currently managed.
    /// Used by integration tests and monitoring.
    #[allow(dead_code)]
    pub fn has_agent(&self, agent_id: &str) -> bool {
        self.processes.contains_key(agent_id)
    }

    /// Insert a managed process directly. Used for testing and internal bookkeeping.
    #[allow(dead_code)]
    pub fn insert_process(&self, process: ManagedProcess) {
        self.processes.insert(process.agent_id.clone(), process);
    }

    /// Remove a managed process by agent ID and kill its child process.
    /// Returns true if the process was found and removed.
    /// Used by integration tests and health monitor cleanup.
    #[allow(dead_code)]
    pub async fn remove_process(&self, agent_id: &str) -> bool {
        if let Some((_, mut process)) = self.processes.remove(agent_id) {
            let _ = process.child.kill().await;
            true
        } else {
            false
        }
    }

    /// Build a Command with the correct environment variables for spawning an agent.
    /// This is separated from `spawn()` to allow testing env var injection.
    pub fn build_command(
        binary_path: &Path,
        agent_id: &str,
        port: u16,
        env: &HashMap<String, String>,
    ) -> Command {
        let mut cmd = Command::new(binary_path);
        cmd.env("AGENT_PORT", port.to_string());
        cmd.env("AGENT_ID", agent_id);
        for (key, value) in env {
            cmd.env(key, value);
        }
        // Don't inherit stdin; capture stdout/stderr.
        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd
    }

    /// Spawn an agent binary as a child process.
    /// Allocates a port, injects env vars (AGENT_PORT, AGENT_ID, + custom env),
    /// and spawns the child process.
    pub async fn spawn(
        &self,
        agent_id: &str,
        binary_path: &Path,
        env: HashMap<String, String>,
    ) -> Result<u16> {
        if self.processes.contains_key(agent_id) {
            bail!("agent '{}' is already running", agent_id);
        }

        let port = self.allocate_port()?;

        let mut cmd = Self::build_command(binary_path, agent_id, port, &env);

        info!(
            agent_id = agent_id,
            port = port,
            binary = %binary_path.display(),
            "spawning agent process"
        );

        let child = cmd.spawn().with_context(|| {
            format!(
                "failed to spawn agent '{}' from {:?}",
                agent_id, binary_path
            )
        })?;

        let process = ManagedProcess {
            agent_id: agent_id.to_string(),
            port,
            child,
            started_at: Utc::now(),
            health: HealthStatus::Unknown,
        };

        self.processes.insert(agent_id.to_string(), process);
        Ok(port)
    }

    /// Wait for the agent's AgentCard to become available (readiness probe).
    /// Polls GET http://127.0.0.1:{port}/.well-known/agent.json with exponential
    /// backoff starting at 100ms, doubling each time up to 1600ms, total timeout 30s.
    pub async fn wait_ready(&self, agent_id: &str, timeout: Duration) -> Result<()> {
        let entry = self
            .processes
            .get(agent_id)
            .ok_or_else(|| anyhow::anyhow!("agent '{}' not found in process manager", agent_id))?;
        let port = entry.port;
        drop(entry);

        let url = format!("http://127.0.0.1:{}/.well-known/agent.json", port);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()?;

        let start = tokio::time::Instant::now();
        let mut delay = Duration::from_millis(100);
        let max_delay = Duration::from_millis(1600);

        loop {
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    info!(agent_id = agent_id, port = port, "agent is ready");
                    // Mark healthy.
                    if let Some(mut entry) = self.processes.get_mut(agent_id) {
                        entry.health = HealthStatus::Healthy;
                    }
                    return Ok(());
                }
                Ok(resp) => {
                    debug!(
                        agent_id = agent_id,
                        status = %resp.status(),
                        "agent not ready yet"
                    );
                }
                Err(e) => {
                    debug!(
                        agent_id = agent_id,
                        error = %e,
                        "agent not ready yet"
                    );
                }
            }

            if start.elapsed() >= timeout {
                bail!(
                    "agent '{}' did not become ready within {:?}",
                    agent_id,
                    timeout
                );
            }

            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(max_delay);
        }
    }

    /// Stop an agent process. Sends SIGTERM, waits for graceful exit,
    /// then SIGKILL if the process doesn't exit within drain_timeout.
    pub async fn stop(&self, agent_id: &str, drain_timeout: Duration) -> Result<()> {
        let mut entry = self
            .processes
            .get_mut(agent_id)
            .ok_or_else(|| anyhow::anyhow!("agent '{}' not found in process manager", agent_id))?;

        let child = &mut entry.child;

        // Try to get the PID for SIGTERM.
        if let Some(pid) = child.id() {
            info!(agent_id = agent_id, pid = pid, "sending SIGTERM to agent");
            // Send SIGTERM via libc.
            #[cfg(unix)]
            {
                let ret = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                if ret != 0 {
                    let err = std::io::Error::last_os_error();
                    warn!(agent_id = agent_id, pid = pid, error = %err, "SIGTERM failed");
                }
            }
            #[cfg(not(unix))]
            {
                // On non-unix, just kill directly.
                let _ = child.kill().await;
            }
        } else {
            // Process may have already exited.
            warn!(
                agent_id = agent_id,
                "no PID available, process may have already exited"
            );
        }

        // Wait for graceful exit with timeout.
        let wait_result = tokio::time::timeout(drain_timeout, child.wait()).await;

        match wait_result {
            Ok(Ok(status)) => {
                info!(
                    agent_id = agent_id,
                    status = %status,
                    "agent process exited gracefully"
                );
            }
            Ok(Err(e)) => {
                warn!(
                    agent_id = agent_id,
                    error = %e,
                    "error waiting for agent process"
                );
            }
            Err(_) => {
                // Timeout — force kill.
                warn!(
                    agent_id = agent_id,
                    "agent did not exit within {:?}, sending SIGKILL", drain_timeout
                );
                if let Some(pid) = entry.child.id() {
                    #[cfg(unix)]
                    {
                        let ret = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
                        if ret != 0 {
                            let err = std::io::Error::last_os_error();
                            warn!(agent_id = agent_id, pid = pid, error = %err, "SIGKILL failed");
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = entry.child.kill().await;
                    }
                }
                // Wait briefly for the kill to take effect.
                let _ = tokio::time::timeout(Duration::from_secs(2), entry.child.wait()).await;
            }
        }

        // Drop the mutable reference before removing.
        drop(entry);

        // Remove from managed processes — this releases the port.
        self.processes.remove(agent_id);
        info!(agent_id = agent_id, "agent process stopped and removed");

        Ok(())
    }

    /// Check agent health via GET on the agent card endpoint.
    /// Tracks consecutive failures.
    pub async fn health_check(&self, agent_id: &str) -> HealthStatus {
        let port = match self.processes.get(agent_id) {
            Some(entry) => entry.port,
            None => return HealthStatus::Unknown,
        };

        let url = format!("http://127.0.0.1:{}/.well-known/agent.json", port);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default();

        let status = match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => HealthStatus::Healthy,
            _ => {
                // Increment consecutive failures.
                let prev_failures = match self.processes.get(agent_id) {
                    Some(entry) => match &entry.health {
                        HealthStatus::Unhealthy {
                            consecutive_failures,
                        } => *consecutive_failures,
                        _ => 0,
                    },
                    None => 0,
                };
                HealthStatus::Unhealthy {
                    consecutive_failures: prev_failures + 1,
                }
            }
        };

        // Update health status in the process entry.
        if let Some(mut entry) = self.processes.get_mut(agent_id) {
            entry.health = status.clone();
        }

        status
    }

    /// Start a background health monitor that periodically checks all running agents.
    /// Calls `on_failure` when an agent has 3 or more consecutive health check failures.
    pub fn start_health_monitor(
        self: Arc<Self>,
        on_failure: impl Fn(String, String) + Send + Sync + 'static,
    ) -> tokio::task::JoinHandle<()> {
        let interval = self.health_interval;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;

                // Collect agent IDs to check.
                let agent_ids: Vec<String> = self
                    .processes
                    .iter()
                    .map(|entry| entry.key().clone())
                    .collect();

                for agent_id in agent_ids {
                    let status = self.health_check(&agent_id).await;
                    if let HealthStatus::Unhealthy {
                        consecutive_failures,
                    } = &status
                    {
                        if *consecutive_failures >= 3 {
                            error!(
                                agent_id = %agent_id,
                                failures = consecutive_failures,
                                "agent health check failed 3+ times"
                            );
                            on_failure(
                                agent_id.clone(),
                                format!(
                                    "health check failed {} consecutive times",
                                    consecutive_failures
                                ),
                            );
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 5.9: Port allocation doesn't reuse active ports ────────────

    #[test]
    fn port_allocation_no_reuse() {
        let pm = ProcessManager::new(19001..=19005, Duration::from_secs(30));

        // Allocate all 5 ports.
        let mut ports = Vec::new();
        for _ in 0..5 {
            let port = pm.allocate_port().unwrap();
            assert!(
                !ports.contains(&port),
                "port {} was already allocated",
                port
            );
            // Simulate an active process on this port by inserting a dummy entry.
            // We use a fake child process — for unit testing port allocation logic only.
            ports.push(port);
        }

        // All 5 ports should be unique.
        let unique: std::collections::HashSet<u16> = ports.iter().copied().collect();
        assert_eq!(unique.len(), 5);
    }

    #[test]
    fn port_allocation_skips_active_ports() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        rt.block_on(async {
            let pm = ProcessManager::new(19001..=19010, Duration::from_secs(30));

            // Manually insert a fake process on port 19001 to simulate it being active.
            // We spawn a simple `sleep` command as a real child process.
            let child = Command::new("sleep")
                .arg("60")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("failed to spawn sleep");

            pm.processes.insert(
                "fake-agent".to_string(),
                ManagedProcess {
                    agent_id: "fake-agent".to_string(),
                    port: 19001,
                    child,
                    started_at: Utc::now(),
                    health: HealthStatus::Unknown,
                },
            );

            // Next allocation should skip 19001.
            let port = pm.allocate_port().unwrap();
            assert_ne!(port, 19001, "should skip the active port 19001");
            assert!(
                (19002..=19010).contains(&port),
                "allocated port {} should be in range 19002..=19010",
                port
            );

            // Clean up the fake process.
            if let Some(mut entry) = pm.processes.get_mut("fake-agent") {
                let _ = entry.child.kill().await;
            }
            pm.processes.remove("fake-agent");
        });
    }

    // ── 5.10: Spawn injects correct env vars ──────────────────────

    #[test]
    fn spawn_injects_correct_env_vars() {
        let binary_path = Path::new("/usr/bin/echo");
        let agent_id = "test-agent";
        let port = 19042;
        let mut env = HashMap::new();
        env.insert("ANTHROPIC_API_KEY".to_string(), "sk-test-123".to_string());
        env.insert("AGENT_DATA_DIR".to_string(), "/data/test-agent".to_string());

        let cmd = ProcessManager::build_command(binary_path, agent_id, port, &env);

        // Verify the command is constructed correctly.
        // We can inspect the Command's program.
        let as_std = cmd.as_std();
        assert_eq!(as_std.get_program(), "/usr/bin/echo");

        // Check environment variables are set.
        let envs: HashMap<String, String> = as_std
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect();

        assert_eq!(envs.get("AGENT_PORT").map(|s| s.as_str()), Some("19042"));
        assert_eq!(envs.get("AGENT_ID").map(|s| s.as_str()), Some("test-agent"));
        assert_eq!(
            envs.get("ANTHROPIC_API_KEY").map(|s| s.as_str()),
            Some("sk-test-123")
        );
        assert_eq!(
            envs.get("AGENT_DATA_DIR").map(|s| s.as_str()),
            Some("/data/test-agent")
        );
    }

    #[test]
    fn spawn_injects_multiple_custom_env_vars() {
        let binary_path = Path::new("/bin/true");
        let agent_id = "multi-env-agent";
        let port = 19050;
        let mut env = HashMap::new();
        env.insert("KEY_A".to_string(), "val_a".to_string());
        env.insert("KEY_B".to_string(), "val_b".to_string());
        env.insert("KEY_C".to_string(), "val_c".to_string());

        let cmd = ProcessManager::build_command(binary_path, agent_id, port, &env);
        let as_std = cmd.as_std();

        let envs: HashMap<String, String> = as_std
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string())))
            .collect();

        // Should have AGENT_PORT + AGENT_ID + 3 custom = 5 total.
        assert_eq!(envs.len(), 5);
        assert_eq!(envs.get("AGENT_PORT").map(|s| s.as_str()), Some("19050"));
        assert_eq!(
            envs.get("AGENT_ID").map(|s| s.as_str()),
            Some("multi-env-agent")
        );
        assert_eq!(envs.get("KEY_A").map(|s| s.as_str()), Some("val_a"));
        assert_eq!(envs.get("KEY_B").map(|s| s.as_str()), Some("val_b"));
        assert_eq!(envs.get("KEY_C").map(|s| s.as_str()), Some("val_c"));
    }

    // ── 12.4: Shutdown stops all managed processes ─────────────────

    #[tokio::test]
    async fn shutdown_stops_all_managed_processes() {
        let pm = ProcessManager::new(19001..=19010, Duration::from_secs(30));

        // Spawn several sleep processes to simulate running agents.
        let agent_ids = ["agent-a", "agent-b", "agent-c"];
        for agent_id in &agent_ids {
            let child = Command::new("sleep")
                .arg("300")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("failed to spawn sleep");

            let port = pm.allocate_port().unwrap();
            pm.insert_process(ManagedProcess {
                agent_id: agent_id.to_string(),
                port,
                child,
                started_at: Utc::now(),
                health: HealthStatus::Healthy,
            });
        }

        // All 3 agents should be managed.
        assert_eq!(pm.process_count(), 3);
        for agent_id in &agent_ids {
            assert!(pm.has_agent(agent_id));
        }

        // Simulate shutdown: stop each agent process.
        let running_ids: Vec<String> = pm
            .processes
            .iter()
            .map(|entry| entry.key().clone())
            .collect();

        for agent_id in &running_ids {
            pm.stop(agent_id, Duration::from_secs(5))
                .await
                .expect("stop should succeed");
        }

        // After shutdown, no processes should remain.
        assert_eq!(pm.process_count(), 0);
        for agent_id in &agent_ids {
            assert!(!pm.has_agent(agent_id));
        }

        // Ports should be released (no active ports).
        assert!(pm.active_ports().is_empty());
    }
}
