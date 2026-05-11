//! Configuration JSON API handler.

use super::ControlPanelState;
use std::sync::Arc;

pub(crate) async fn config_json(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(state.redacted_config())
}
