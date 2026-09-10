//! `POST /api/reload`: the same reload `SIGHUP` runs.

use axum::extract::State;
use axum::response::Response;

use super::live;
use crate::server::{AppState, ReloadTrigger};

pub async fn reload(State(app): State<AppState>) -> Response {
    let event = tokio::task::spawn_blocking(move || app.reload(ReloadTrigger::Console))
        .await
        .expect("reload does not panic");
    live(&event)
}
