use axum::Json;
use serde_json::{Value, json};

/// Liveness probe; needs no token.
pub async fn healthz() -> Json<Value> {
    Json(json!({"status": "ok", "service": "claude-router", "version": env!("CARGO_PKG_VERSION")}))
}
