use axum::Json;
use serde_json::{Value, json};

/// Liveness probe; needs no token.
pub async fn healthz() -> Json<Value> {
    Json(json!({"status": "ok", "service": "claude-router", "version": crate::build_info::VERSION}))
}
