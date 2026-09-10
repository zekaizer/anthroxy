//! `GET /api/status`: the running configuration and how it got there.

use std::sync::Arc;

use axum::Extension;
use axum::extract::State;
use axum::response::Response;
use serde_json::{Value, json};

use super::live;
use crate::config::view;
use crate::server::{AppState, Snapshot};

pub async fn status(
    State(app): State<AppState>,
    Extension(snapshot): Extension<Arc<Snapshot>>,
) -> Response {
    let now = jiff::Timestamp::now();
    let backends: Vec<Value> = snapshot
        .registry
        .backends()
        .map(|backend| {
            json!({
                "name": backend.name,
                "kind": backend.kind,
                "url": backend.url,
                "credential": backend.credential.status(),
                "drop_fields": backend.drop_fields,
                "anthropic_beta": backend.anthropic_beta,
            })
        })
        .collect();
    let models: Vec<Value> = snapshot
        .registry
        .routes()
        .iter()
        .map(|route| {
            json!({
                "id": route.id,
                "display_name": route.display_name,
                "backend": route.backend.name,
                "upstream_model": route.upstream_model,
                "aliases": route.aliases,
            })
        })
        .collect();
    let config = &snapshot.config;
    live(&json!({
        "version": crate::build_info::VERSION,
        "started_at": app.started_at,
        "now": now,
        "uptime_s": now.duration_since(app.started_at).as_secs().max(0),
        "listen": app.listen().to_string(),
        "config_path": app.config_path().map(|path| path.display().to_string()),
        "reloads": app.reloads(),
        "backends": backends,
        "models": models,
        "default_model": snapshot.registry.default_route().map(|route| route.id.clone()),
        "body_log": snapshot.body_log.as_ref().map(|log| json!({
            "dir": log.root().display().to_string(),
            "retention": view::duration_text(config.logging.body_retention),
        })),
        "stats": snapshot.stats.as_ref().map(|stats| json!({
            "dir": stats.dir().display().to_string(),
            "retention": view::duration_text(config.stats.retention),
        })),
        "names": app.activity.names(),
        "config": view::masked(config),
    }))
}
