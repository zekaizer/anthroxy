//! `GET /api/env`: Claude Code's variables for the address the browser used
//! to reach the router, which is the address the Windows host can use too.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Extension;
use axum::extract::{ConnectInfo, State};
use axum::response::Response;
use http::HeaderMap;
use http::header::HOST;
use serde_json::json;

use super::live;
use super::probe::max_context_tokens;
use crate::config::client_env::{self, EnvFormat};
use crate::server::{AppState, Snapshot};

pub async fn env(
    State(app): State<AppState>,
    Extension(snapshot): Extension<Arc<Snapshot>>,
    ConnectInfo(viewer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let config = &snapshot.config;
    let host = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|host| plain_host(host))
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let host = client_env::host(config, None);
            format!("{host}:{}", config.server.listen.port())
        });
    let base_url = format!("http://{host}");
    let mut vars = client_env::vars(config, &base_url);
    let max_context = max_context_tokens(&app.probed(), &snapshot.registry);
    if let Some(tokens) = max_context {
        vars.push(("CLAUDE_CODE_MAX_CONTEXT_TOKENS", tokens.to_string()));
    }
    live(&json!({
        "base_url": base_url,
        "viewer": viewer.to_string(),
        "max_context_tokens": max_context,
        "sh": client_env::render(&vars, EnvFormat::Sh, false),
        "powershell": client_env::render(&vars, EnvFormat::Powershell, false),
        "json": client_env::render(&vars, EnvFormat::Json, false),
    }))
}

/// A host and optional port, nothing a shell snippet should carry.
fn plain_host(host: &str) -> bool {
    !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}
