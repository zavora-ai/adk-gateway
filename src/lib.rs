//! adk-gateway library — exposes modules for integration testing.

pub mod access_control;
pub mod agent_config;
pub mod agent_registry;
pub mod audit;
pub mod browser_factory;
pub mod channel;
pub mod config;
pub mod context_coordinator;
pub mod cron;
pub mod delivery;
pub mod event_stream;
pub mod jwt;
pub mod knowledge_graph;
pub mod mcp;
pub mod plugin_manager;
pub mod reconnect;
pub mod router;
pub mod session_bridge;
pub mod shutdown;
pub mod skill_loader;
pub mod tool_registry;
pub mod webhook;

pub mod rbac_bridge;

pub mod process_manager;

pub mod agent_codegen;

pub mod proxy_pool;

pub mod action_executor;
pub mod config_watcher;
pub mod control_panel;
pub mod gateway_routes;
pub mod gateway_state;
pub mod graph_workflow;
pub mod metrics;
pub mod model_factory;
pub mod pairing;
pub mod rag;
pub mod sqlrite_store;
pub mod telemetry;

pub mod awp;
pub mod executable_tools;
pub mod fallback_chain;
