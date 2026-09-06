//! `GET /v1/models` and `GET /v1/models/{id}`: the model table Claude Code
//! discovers (ADR-0003).

use std::sync::Arc;

use axum::extract::{Path, State};
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
    Path(id): Path<String>,
) -> Response {
    match snapshot.registry.lookup(&id) {
        Some(resolution) => Json(object(&state, resolution.route)).into_response(),
        None => RouterError::unknown_model(id, &snapshot.registry).into_response(&request_id),
    }
}
