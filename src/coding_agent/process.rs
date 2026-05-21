//! Child process management for stdio-based coding agents.
//!
//! Spawns coding agent CLIs as child processes and communicates via
//! stdin/stdout using JSON-RPC (ACP over stdio). Handles process lifecycle,
//! restart on crash, and graceful shutdown.

use std::process::Stdio;
use std::sync::Arc;

use chrono::Utc;
use dashmap::DashMap;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use super::config::AgentTransport;
use super::registry::CodingAgentRegistry;
use super::status::AgentConnectionStatus;

/// A managed coding agent child process.
struct ManagedAgent {
    /// The child process handle.
    child: Child,
    /// Stdin writer for sending requests.
    stdin: tokio::process::ChildStdin,
}

/// Manages coding agent child processes (stdio transport).
pub struct AgentProcessManager {
    /// Running agent processes keyed by agent ID.
    processes: DashMap<String, Arc<Mutex<ManagedAgent>>>,
    /// Reference to the registry for status updates.
    registry: Arc<CodingAgentRegistry>,
}

impl AgentProcessManager {
    /// Create a new process manager.
    pub fn new(registry: Arc<CodingAgentRegistry>) -> Self {
        Self {
            processes: DashMap::new(),
            registry,
        }
    }

    /// Spawn a coding agent process for the given agent ID and transport config.
    pub async fn spawn_agent(
        &self,
        agent_id: &str,
        transport: &AgentTransport,
    ) -> anyhow::Result<()> {
        let AgentTransport::Stdio { command, args, env } = transport else {
            anyhow::bail!("spawn_agent only supports Stdio transport");
        };

        info!(
            agent_id = %agent_id,
            command = %command,
            args = ?args,
            "spawning coding agent process"
        );

        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, val) in env {
            cmd.env(key, val);
        }

        let mut child = cmd.spawn().map_err(|e| {
            anyhow::anyhow!("failed to spawn agent '{}': {} (command: {})", agent_id, e, command)
        })?;

        let stdin = child.stdin.take()
            .ok_or_else(|| anyhow::anyhow!("failed to capture stdin for agent '{}'", agent_id))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Spawn stdout reader for logging/monitoring
        let agent_id_clone = agent_id.to_string();
        if let Some(stdout) = stdout {
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    debug!(agent_id = %agent_id_clone, stdout = %line, "agent stdout");
                }
            });
        }

        // Spawn stderr reader for error logging
        let agent_id_clone2 = agent_id.to_string();
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    warn!(agent_id = %agent_id_clone2, stderr = %line, "agent stderr");
                }
            });
        }

        let managed = ManagedAgent { child, stdin };
        self.processes.insert(agent_id.to_string(), Arc::new(Mutex::new(managed)));

        // Update status to connected
        let _ = self.registry.update_status(agent_id, AgentConnectionStatus::Connected);

        info!(agent_id = %agent_id, "coding agent process spawned successfully");
        Ok(())
    }

    /// Stop a running agent process.
    pub async fn stop_agent(&self, agent_id: &str) -> anyhow::Result<()> {
        let Some((_, process)) = self.processes.remove(agent_id) else {
            anyhow::bail!("agent '{}' is not running", agent_id);
        };

        let mut managed = process.lock().await;
        // Try graceful shutdown first
        let _ = managed.stdin.shutdown().await;

        // Give it a moment to exit
        tokio::select! {
            result = managed.child.wait() => {
                match result {
                    Ok(status) => info!(agent_id = %agent_id, exit_code = ?status.code(), "agent process exited"),
                    Err(e) => warn!(agent_id = %agent_id, error = %e, "error waiting for agent exit"),
                }
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                warn!(agent_id = %agent_id, "agent did not exit gracefully, killing");
                let _ = managed.child.kill().await;
            }
        }

        let _ = self.registry.update_status(
            agent_id,
            AgentConnectionStatus::Disconnected { since: Utc::now() },
        );

        Ok(())
    }

    /// Check if an agent process is running.
    pub fn is_running(&self, agent_id: &str) -> bool {
        self.processes.contains_key(agent_id)
    }

    /// Get the list of running agent IDs.
    pub fn running_agents(&self) -> Vec<String> {
        self.processes.iter().map(|e| e.key().clone()).collect()
    }

    /// Send a JSON-RPC message to an agent's stdin.
    pub async fn send_message(&self, agent_id: &str, message: &str) -> anyhow::Result<()> {
        let process = self.processes.get(agent_id)
            .ok_or_else(|| anyhow::anyhow!("agent '{}' is not running", agent_id))?;

        let mut managed = process.lock().await;
        managed.stdin.write_all(message.as_bytes()).await?;
        managed.stdin.write_all(b"\n").await?;
        managed.stdin.flush().await?;

        Ok(())
    }

    /// Spawn all agents that have stdio transport configured.
    pub async fn spawn_all(&self, agents: &[(String, AgentTransport)]) {
        for (agent_id, transport) in agents {
            if let Err(e) = self.spawn_agent(agent_id, transport).await {
                error!(agent_id = %agent_id, error = %e, "failed to spawn agent on startup");
                let _ = self.registry.update_status(
                    agent_id,
                    AgentConnectionStatus::Error {
                        message: format!("Failed to spawn: {}", e),
                        since: Utc::now(),
                    },
                );
            }
        }
    }
}

impl Drop for AgentProcessManager {
    fn drop(&mut self) {
        // Best-effort cleanup — kill any remaining processes
        for entry in self.processes.iter() {
            let agent_id = entry.key().clone();
            warn!(agent_id = %agent_id, "dropping agent process manager, process may be orphaned");
        }
    }
}
