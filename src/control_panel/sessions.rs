//! Sessions JSON API handler.

use super::{ControlPanelState, SessionInfo};
use std::sync::Arc;

pub(crate) async fn sessions_json(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<Vec<SessionInfo>> {
    axum::Json(state.sessions_list())
}
