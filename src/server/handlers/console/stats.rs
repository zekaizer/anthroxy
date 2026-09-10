//! `GET /api/stats?range=1d|7d|30d|all`.

use std::sync::Arc;

use axum::Extension;
use axum::extract::Query;
use axum::response::Response;
use http::StatusCode;
use serde::Deserialize;
use serde_json::json;

use super::{error, live};
use crate::anthropic::ErrorType;
use crate::server::{RequestId, Snapshot};
use crate::stats::{Range, aggregate};

#[derive(Deserialize)]
pub struct StatsQuery {
    range: Option<String>,
}

/// The last seven days unless `range` says otherwise.
pub async fn stats(
    Extension(snapshot): Extension<Arc<Snapshot>>,
    request_id: RequestId,
    Query(query): Query<StatsQuery>,
) -> Response {
    let Some(log) = snapshot.stats.clone() else {
        return live(&json!({"enabled": false}));
    };
    let range = match query.range.as_deref() {
        None => Range::Week,
        Some(text) => match Range::parse(text) {
            Some(range) => range,
            None => {
                return error(
                    StatusCode::BAD_REQUEST,
                    ErrorType::InvalidRequestError,
                    "range must be 1d, 7d, 30d or all",
                    &request_id,
                );
            }
        },
    };
    let since = range.since(jiff::Timestamp::now());
    let dir = log.dir().display().to_string();
    let report = tokio::task::spawn_blocking(move || aggregate(&log.read(since), range, since))
        .await
        .expect("aggregation does not panic");
    live(&json!({"enabled": true, "dir": dir, "report": report}))
}
