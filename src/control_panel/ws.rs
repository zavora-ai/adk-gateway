//! WebSocket handler and event types for live updates.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};

use super::ControlPanelState;
use crate::coding_agent::status::AgentConnectionStatus;

/// Events broadcast to all connected WebSocket clients.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type")]
pub enum WsEvent {
    #[serde(rename = "connected")]
    Connected { message: String },
    #[serde(rename = "log")]
    Log {
        timestamp: String,
        level: String,
        message: String,
        target: Option<String>,
    },
    #[serde(rename = "agent_state")]
    AgentState { agent_id: String, state: String },
    #[serde(rename = "dashboard")]
    Dashboard {
        uptime_secs: u64,
        session_count: u64,
        channel_count: usize,
    },
    /// Coding agent connection status change event.
    #[serde(rename = "coding_agent_status")]
    CodingAgentStatus {
        agent_id: String,
        previous_status: AgentConnectionStatus,
        new_status: AgentConnectionStatus,
        timestamp: String,
    },
    /// Coding agent task state transition event.
    #[serde(rename = "coding_agent_task")]
    CodingAgentTask {
        agent_id: String,
        task_id: String,
        state: String,
        timestamp: String,
    },
    /// Coding agent cost cap warning event.
    #[serde(rename = "coding_agent_cost_warning")]
    CodingAgentCostWarning {
        agent_id: String,
        current_cost_usd: f64,
        cap_usd: f64,
        timestamp: String,
    },
}

pub(crate) async fn ws_events_handler(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
    ws: axum::extract::WebSocketUpgrade,
) -> impl axum::response::IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_connection(socket, state))
}

async fn handle_ws_connection(mut socket: WebSocket, state: Arc<ControlPanelState>) {
    // Send initial connected message
    let connected_msg = serde_json::to_string(&WsEvent::Connected {
        message: "event stream active".to_string(),
    })
    .unwrap_or_default();

    if socket
        .send(Message::Text(connected_msg.into()))
        .await
        .is_err()
    {
        return;
    }

    // Subscribe to the main broadcast channel
    let mut rx = state.ws_broadcast.subscribe();

    // Subscribe to coding agent status events if registry is available
    let mut coding_agent_rx = state
        .coding_agent_registry
        .as_ref()
        .map(|registry| registry.subscribe_status());

    loop {
        tokio::select! {
            // Forward broadcast events to the WebSocket client
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        let json = match serde_json::to_string(&event) {
                            Ok(j) => j,
                            Err(_) => continue,
                        };
                        if socket.send(Message::Text(json.into())).await.is_err() {
                            // Client disconnected
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!(skipped = n, "WebSocket client lagged, skipping messages");
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
            // Forward coding agent status events to the WebSocket client
            result = async {
                match coding_agent_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    // If no registry, pend forever so this branch never fires
                    None => std::future::pending().await,
                }
            } => {
                match result {
                    Ok(status_event) => {
                        let ws_event = WsEvent::CodingAgentStatus {
                            agent_id: status_event.agent_id,
                            previous_status: status_event.previous_status,
                            new_status: status_event.new_status,
                            timestamp: status_event.timestamp.to_rfc3339(),
                        };
                        let json = match serde_json::to_string(&ws_event) {
                            Ok(j) => j,
                            Err(_) => continue,
                        };
                        if socket.send(Message::Text(json.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!(
                            skipped = n,
                            "Coding agent status receiver lagged, skipping events"
                        );
                        continue;
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Coding agent subsystem shut down; disable this branch
                        coding_agent_rx = None;
                        continue;
                    }
                }
            }
            // Handle incoming messages from the client (mainly for detecting disconnect)
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                    _ => {} // Ignore other messages
                }
            }
        }
    }
}
