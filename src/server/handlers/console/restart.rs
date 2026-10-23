//! `POST /api/restart`: a restart of the process, behind the same reload
//! `SIGHUP` runs as its gate.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use serde_json::json;

use super::{error, live};
use crate::anthropic::ErrorType;
use crate::server::{AppState, RequestId};

/// 202 with the gate's reload and the address the restarted router listens
/// on, once the restart is under way; 409 with the reason when the file did
/// not pass the gate, and the router carries on as it was.
pub async fn restart(State(app): State<AppState>, request_id: RequestId) -> Response {
    let state = app.clone();
    let gate = tokio::task::spawn_blocking(move || state.restart())
        .await
        .expect("restart does not panic");
    match gate {
        Ok(event) => {
            let mut body = serde_json::to_value(&event).expect("events serialize");
            body["listen"] = json!(app.snapshot().config.server.listen);
            (StatusCode::ACCEPTED, live(&body)).into_response()
        }
        Err(reason) => error(
            StatusCode::CONFLICT,
            ErrorType::InvalidRequestError,
            &reason,
            &request_id,
        ),
    }
}
