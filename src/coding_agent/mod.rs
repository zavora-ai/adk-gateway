//! Coding Agent Integration subsystem.
//!
//! Provides lifecycle management for external coding agents (Kiro CLI, Claude Code,
//! OpenCode, Pi Agent, OpenClaw, Hermes, GitHub Copilot CLI). Extends the existing
//! ACP scaffolding with registration, monitoring, task orchestration, cost tracking,
//! and multi-channel result delivery.

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
pub mod history;
