use axum::Json;
use serde_json::{Value, json};

/// Claude Code's connection-warming probe, sent at startup and not
/// necessarily with a token; Claude Code does not read the answer.
pub async fn hello() {}

/// Liveness probe; needs no token.
pub async fn healthz() -> Json<Value> {
    Json(json!({"status": "ok", "service": "anthroxy", "version": crate::build_info::VERSION}))
}
