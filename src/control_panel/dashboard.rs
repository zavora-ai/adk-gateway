//! Dashboard JSON API handler.

use super::{ControlPanelState, DashboardData};
use std::sync::Arc;

pub(crate) async fn dashboard_json(
    axum::extract::State(state): axum::extract::State<Arc<ControlPanelState>>,
) -> axum::Json<DashboardData> {
    axum::Json(state.dashboard())
}
