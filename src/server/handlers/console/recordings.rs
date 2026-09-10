//! `/api/recordings`: the body log, listed, read and deleted.

use std::sync::Arc;

use axum::Extension;
use axum::extract::Path;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS};
use serde_json::json;

use super::{error, live, not_found};
use crate::anthropic::ErrorType;
use crate::server::{RequestId, Snapshot};

/// Entries listed at most.
const LISTED: usize = 500;

pub async fn list(Extension(snapshot): Extension<Arc<Snapshot>>) -> Response {
    let Some(log) = snapshot.body_log.clone() else {
        return live(&json!({"dir": null, "entries": []}));
    };
    let dir = log.root().display().to_string();
    let entries = tokio::task::spawn_blocking(move || log.list(LISTED))
        .await
        .expect("listing does not panic");
    live(&json!({
        "dir": dir,
        "retention": crate::config::view::duration_text(snapshot.config.logging.body_retention),
        "entries": entries,
    }))
}

/// A recorded file as plain text: the page shows it, nothing renders it.
pub async fn file(
    Extension(snapshot): Extension<Arc<Snapshot>>,
    request_id: RequestId,
    Path((name, file)): Path<(String, String)>,
) -> Response {
    let Some(path) = snapshot
        .body_log
        .as_ref()
        .and_then(|log| log.file(&name, &file))
    else {
        return not_found("no such recording", &request_id);
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (CONTENT_TYPE, "text/plain; charset=utf-8"),
                (X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (CONTENT_SECURITY_POLICY, "sandbox"),
                (CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => not_found("no such recording", &request_id),
    }
}

pub async fn remove(
    Extension(snapshot): Extension<Arc<Snapshot>>,
    request_id: RequestId,
    Path(name): Path<String>,
) -> Response {
    let Some(log) = snapshot.body_log.clone() else {
        return not_found("body recording is off", &request_id);
    };
    match tokio::task::spawn_blocking(move || log.remove(&name))
        .await
        .expect("removal does not panic")
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found("no such recording", &request_id),
        Err(problem) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorType::ApiError,
            &format!("cannot delete the recording: {problem}"),
            &request_id,
        ),
    }
}

pub async fn remove_all(Extension(snapshot): Extension<Arc<Snapshot>>) -> Response {
    let removed = match snapshot.body_log.clone() {
        Some(log) => tokio::task::spawn_blocking(move || log.remove_all())
            .await
            .expect("removal does not panic"),
        None => 0,
    };
    live(&json!({"removed": removed}))
}
