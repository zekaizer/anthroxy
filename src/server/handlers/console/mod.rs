//! The web console (ADR-0011): its page, and the `/api/` routes the page
//! reads and acts through.

mod assets;
mod env;
mod health;
mod probe;
mod recordings;
mod reload;
mod requests;
mod smoke;
mod stats;
mod status;

use axum::Json;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use http::header::{CACHE_CONTROL, X_CONTENT_TYPE_OPTIONS};
use serde::Serialize;

use crate::anthropic::{ErrorResponse, ErrorType};
use crate::server::RequestId;

pub use assets::{index, recordings_script, root, script, style};
pub use env::env;
pub use probe::probe;
pub use recordings::{
    file as recording_file, list as recordings, remove as remove_recording,
    remove_all as remove_recordings,
};
pub use reload::reload;
pub use requests::{detail as request, list as requests};
pub use smoke::smoke;
pub use stats::stats;
pub use status::status;

/// Live state as JSON; never cached.
fn live<T: Serialize>(value: &T) -> Response {
    (
        [
            (CACHE_CONTROL, "no-store"),
            (X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Json(value),
    )
        .into_response()
}

/// An error in the router's usual shape (ADR-0005).
fn error(status: StatusCode, kind: ErrorType, message: &str, request_id: &RequestId) -> Response {
    let body = ErrorResponse::new(kind, message.to_owned()).with_request_id(request_id.as_str());
    (
        status,
        [
            (CACHE_CONTROL, "no-store"),
            (X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        Json(body),
    )
        .into_response()
}

fn not_found(what: &str, request_id: &RequestId) -> Response {
    error(
        StatusCode::NOT_FOUND,
        ErrorType::NotFoundError,
        what,
        request_id,
    )
}
