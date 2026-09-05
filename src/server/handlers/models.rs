//! `GET /v1/models` and `GET /v1/models/{id}`: the model table Claude Code
//! discovers (ADR-0003).

use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};

use crate::anthropic::{ModelList, ModelObject};
use crate::routing::Route;
use crate::server::{AppState, RequestId, RouterError};

fn object(state: &AppState, route: &Route) -> ModelObject {
    ModelObject::new(&route.id, &route.display_name, state.started_at.to_string())
}

pub async fn list(State(state): State<AppState>) -> Json<ModelList> {
    let data = state
        .registry
        .routes()
        .iter()
        .map(|r| object(&state, r))
        .collect();
    Json(ModelList::all(data))
}

pub async fn get_one(
    State(state): State<AppState>,
    request_id: RequestId,
    Path(id): Path<String>,
) -> Response {
    match state.registry.resolve(&id) {
        Some(resolution) if resolution.matched != crate::routing::Match::Default => {
            Json(object(&state, resolution.route)).into_response()
        }
        _ => RouterError::UnknownModel {
            model: id,
            known: state
                .registry
                .known_names()
                .into_iter()
                .map(str::to_owned)
                .collect(),
        }
        .into_response(&request_id),
    }
}
