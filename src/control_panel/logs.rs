//! Logs JSON API handler.

use super::{ControlPanelState, LogEntry};
use std::sync::Arc;

pub(crate) async fn logs_json(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<Vec<LogEntry>> {
    axum::Json(state.recent_logs(200))
}
