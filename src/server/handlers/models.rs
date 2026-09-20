//! `GET /v1/models` and `GET /v1/models/{id}`: configured table plus a live
//! passthrough list (ADR-0003).

use std::sync::Arc;

use axum::extract::rejection::PathRejection;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
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
    headers: HeaderMap,
) -> Json<ModelList> {
    let mut data = Vec::new();
    let occupied = |id: &str| snapshot.registry.lookup(id).is_some();
    for live in &snapshot.live {
        data.extend(live.models(&snapshot.upstream, &headers, occupied).await);
    }
    data.extend(snapshot.registry.routes().iter().map(|r| object(&state, r)));
    Json(ModelList::all(data))
}

pub async fn get_one(
    State(state): State<AppState>,
    Extension(snapshot): Extension<Arc<Snapshot>>,
    request_id: RequestId,
    headers: HeaderMap,
    uri: Uri,
    id: Result<Path<String>, PathRejection>,
) -> Response {
    let id = match id {
        Ok(Path(id)) => id,
        Err(_) => uri.path().rsplit('/').next().unwrap_or_default().to_owned(),
    };
    if let Some(resolution) = snapshot.registry.lookup(&id) {
        return Json(object(&state, resolution.route)).into_response();
    }
    let occupied = |name: &str| snapshot.registry.lookup(name).is_some();
    for live in &snapshot.live {
        if let Some(model) = live
            .identity(&id, &snapshot.upstream, &headers, occupied)
            .await
        {
            return Json(model).into_response();
        }
    }
    RouterError::unknown_model(id, &snapshot.registry).into_response(&request_id)
}
