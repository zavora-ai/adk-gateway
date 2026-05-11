//! WebSocket handler and event types for live updates.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};

use super::ControlPanelState;

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

    // Subscribe to the broadcast channel
    let mut rx = state.ws_broadcast.subscribe();

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
