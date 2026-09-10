//! `GET /v1/models` and `GET /v1/models/{id}`: the model table Claude Code
//! discovers (ADR-0003).

use std::sync::Arc;

use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::Uri;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

use crate::anthropic::{ModelList, ModelObject};
use crate::routing::Route;
use crate::server::{AppState, RequestId, RouterError, Snapshot};

fn object(state: &AppState, route: &Route) -> ModelObject {
    ModelObject::new(&route.id, &route.display_name, state.started_at.to_string())
}

pub async fn list(
    State(state): State<AppState>,
    Extension(snapshot): Extension<Arc<Snapshot>>,
) -> Json<ModelList> {
    let data = snapshot
        .registry
        .routes()
        .iter()
        .map(|r| object(&state, r))
        .collect();
    Json(ModelList::all(data))
}

pub async fn get_one(
    State(state): State<AppState>,
    Extension(snapshot): Extension<Arc<Snapshot>>,
    request_id: RequestId,
    uri: Uri,
    id: Result<Path<String>, PathRejection>,
) -> Response {
    let id = match id {
        Ok(Path(id)) => id,
        // A segment that does not percent-decode to UTF-8 names no model, and
        // axum's own rejection is the one reply that would not be an Anthropic
        // error document; the raw segment goes into the router's 404 instead.
        Err(_) => uri.path().rsplit('/').next().unwrap_or_default().to_owned(),
    };
    match snapshot.registry.lookup(&id) {
        Some(resolution) => Json(object(&state, resolution.route)).into_response(),
        None => RouterError::unknown_model(id, &snapshot.registry).into_response(&request_id),
    }
}
