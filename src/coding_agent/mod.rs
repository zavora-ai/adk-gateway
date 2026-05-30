//! Coding Agent Integration subsystem.
//!
//! Provides lifecycle management for external coding agents (Kiro CLI, Claude Code,
//! OpenCode, Pi Agent, Hermes, GitHub Copilot CLI). Extends the existing
//! ACP scaffolding with registration, monitoring, task orchestration, cost tracking,
//! and multi-channel result delivery.
//!
//! ## ACP Integration
//!
//! This module provides two ACP client implementations:
//!
//! - **`acp_client`**: Basic session-based client that blocks until completion.
//!   Good for simple fire-and-forget tasks.
//!
//! - **`acp_streaming`**: Streaming client that yields real-time updates as the
//!   agent works. Provides visibility into tool calls, thoughts, permissions, and
//!   status changes. Use this for transparent agent interactions.
//!
//! ## Example: Streaming Updates
//!
//! ```rust,ignore
//! use adk_gateway::coding_agent::acp_streaming::{StreamingAcpClient, CodingAgentUpdate};
//!
//! let client = StreamingAcpClient::new();
//! let mut stream = client.execute_streaming("kiro", &config, &request).await?;
//!
//! while let Some(update) = stream.recv().await {
//!     match update {
//!         CodingAgentUpdate::Text(text) => print!("{}", text),
//!         CodingAgentUpdate::ToolCallStarted { title } => println!("🔧 {}", title),
//!         CodingAgentUpdate::Done { success, .. } => break,
//!         _ => {}
//!     }
//! }
//! ```

pub mod config;
pub mod models;
pub mod error;
pub mod status;
pub mod registry;
pub mod delegator;
pub mod queue;
pub mod cost;
pub mod progress;
pub mod workspace;
pub mod formatting;
pub mod backend;
pub mod executor;
pub mod health_monitor;
pub mod history_db;
pub mod process;
pub mod acp_client;
pub mod acp_streaming;
pub mod streaming_executor;
pub mod hitl_permissions;
pub mod history;
