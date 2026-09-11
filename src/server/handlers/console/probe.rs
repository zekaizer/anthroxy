//! `POST /api/probe`: each backend's credential and model list, from inside
//! the running router.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Extension;
use axum::extract::State;
use axum::response::Response;
use http::StatusCode;
use serde_json::{Value, json};

use super::{error, live};
use crate::anthropic::ErrorType;
use crate::config::snippet;
use crate::routing::Registry;
use crate::server::{AppState, RequestId, Snapshot};
use crate::upstream::probe::{ListedModel, ModelsProbe, probe_all};
use crate::upstream::{RetryPolicy, UpstreamClient, http_client};

/// Per backend, like `anthroxy check --timeout`.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn probe(
    State(app): State<AppState>,
    Extension(snapshot): Extension<Arc<Snapshot>>,
    request_id: RequestId,
) -> Response {
    let client = match http_client(&snapshot.config.upstream, &snapshot.config.backends)
        .and_then(|builder| Ok(builder.timeout(PROBE_TIMEOUT).build()?))
    {
        Ok(http) => UpstreamClient::new(http, RetryPolicy::never()),
        Err(problem) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorType::ApiError,
                &problem.to_string(),
                &request_id,
            );
        }
    };
    let backends: Vec<_> = snapshot.registry.backends().collect();
    let probes = probe_all(&client, backends.iter().map(|b| b.as_ref())).await;
    let mut listed = HashMap::new();
    let mut reports = Vec::new();
    for (backend, probe) in backends.iter().zip(probes) {
        let credential = match &probe.credential {
            Ok(text) => json!({"ok": true, "text": text}),
            Err(problem) => json!({"ok": false, "text": problem.to_string()}),
        };
        let models = match probe.models {
            None => Value::Null,
            Some(ModelsProbe::Failed(problem)) => json!({"error": problem.to_string()}),
            Some(ModelsProbe::Answered {
                status,
                latency,
                models,
                detail,
            }) => {
                let rows: Vec<Value> = models
                    .iter()
                    .map(|model| {
                        let configured: Vec<&str> = snapshot
                            .registry
                            .routes()
                            .iter()
                            .filter(|r| {
                                r.backend.name == backend.name && r.upstream_model == model.id
                            })
                            .map(|r| r.id.as_str())
                            .collect();
                        json!({
                            "id": model.id,
                            "context_length": model.context_length,
                            "configured_as": configured,
                            "snippet": snippet::model_block(&backend.name, &model.id),
                        })
                    })
                    .collect();
                listed.insert(backend.name.clone(), models);
                json!({
                    "status": status,
                    "latency_ms": latency.as_millis() as u64,
                    "detail": detail,
                    "listed": rows,
                })
            }
        };
        reports.push(json!({
            "name": backend.name,
            "kind": backend.kind,
            "url": backend.url,
            "credential": credential,
            "models": models,
        }));
    }
    let max_context = max_context_tokens(&listed, &snapshot.registry);
    app.set_probed(listed);
    live(&json!({
        "at": jiff::Timestamp::now(),
        "backends": reports,
        "max_context_tokens": max_context,
    }))
}

/// The smallest context window among configured models whose backend listed
/// one: the value Claude Code needs so it compacts in time for every model a
/// session may switch to.
pub fn max_context_tokens(
    listed: &HashMap<String, Vec<ListedModel>>,
    registry: &Registry,
) -> Option<u64> {
    registry
        .routes()
        .iter()
        .filter_map(|route| {
            listed
                .get(&route.backend.name)?
                .iter()
                .find(|model| model.id == route.upstream_model)?
                .context_length
        })
        .min()
}
