//! `GET /api/requests` and `GET /api/requests/{id}`.

use axum::extract::{Path, State};
use axum::response::Response;
use serde_json::{Value, json};

use super::{live, not_found};
use crate::activity::ExchangeView;
use crate::server::{AppState, RequestId};

/// In flight oldest first, with how long each has run; finished newest
/// first. Error bodies are left out of the list; the detail has them.
pub async fn list(State(app): State<AppState>) -> Response {
    let now = jiff::Timestamp::now();
    let in_flight: Vec<Value> = app
        .activity
        .in_flight()
        .iter()
        .map(|view| {
            let mut row = listed(view);
            row["elapsed_ms"] =
                json!(now.duration_since(view.received_at).as_millis().max(0) as u64);
            row
        })
        .collect();
    let recent: Vec<Value> = app
        .activity
        .recent()
        .iter()
        .map(|view| listed(view))
        .collect();
    live(&json!({"now": now, "in_flight": in_flight, "recent": recent}))
}

pub async fn detail(
    State(app): State<AppState>,
    request_id: RequestId,
    Path(id): Path<String>,
) -> Response {
    match app.activity.find(&id) {
        Some(view) => live(&view),
        None => not_found("no such request in memory", &request_id),
    }
}

fn listed(view: &ExchangeView) -> Value {
    let mut row = serde_json::to_value(view).expect("views serialize");
    row["error_body"] = Value::Null;
    row["hint_count"] = json!(view.hints.len());
    row
}
