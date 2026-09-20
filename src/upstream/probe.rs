//! Reachability check used by `anthroxy check`: acquire the credential
//! and call the backend's `models_path`.

use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method};

use super::headers::{SecretView, SentHeader, header_text, sent_headers};
use super::{Backend, UpstreamClient, UpstreamError, UpstreamRequest, upstream_headers};
use crate::credential::CredentialError;
use crate::ir::Model;
use crate::translate;

#[derive(Debug)]
pub struct Probe {
    /// Credential source, plus the masked value when there is one.
    pub credential: Result<String, CredentialError>,
    /// What the router set on the model list request, by name; empty when no
    /// credential could be had and nothing was sent.
    pub request_headers: Vec<SentHeader>,
    pub models: Option<ModelsProbe>,
}

#[derive(Debug)]
pub enum ModelsProbe {
    /// The backend answered; `models` is filled when the body listed models,
    /// `detail` when it carried an error message.
    Answered {
        status: u16,
        latency: Duration,
        /// Response headers as received, in order.
        headers: Vec<(String, String)>,
        models: Vec<Model>,
        detail: Option<String>,
    },
    Failed(UpstreamError),
    /// Not sent: a passthrough backend has no stored credential and the
    /// catalog is fetched with the client's Authorization at request time.
    Skipped {
        reason: String,
    },
}

/// Never fails: every outcome is data for the report. `client` decides the
/// timeouts and retries; `check` uses none.
pub async fn probe(client: &UpstreamClient, backend: &Backend) -> Probe {
    if backend.forwards_client_auth {
        return Probe {
            credential: Ok(backend.credential.describe()),
            request_headers: Vec::new(),
            models: Some(ModelsProbe::Skipped {
                reason: "forwards client Authorization; catalog is live per request".into(),
            }),
        };
    }
    let credential = match backend.credential.credential().await {
        Ok(credential) => credential,
        Err(error) => {
            return Probe {
                credential: Err(error),
                request_headers: Vec::new(),
                models: None,
            };
        }
    };
    let description = match &credential {
        Some(c) => format!("{} ({})", backend.credential.describe(), c.masked()),
        None => backend.credential.describe(),
    };
    let request = list_request(backend, &backend.models_path);
    let shown = credential.as_ref().map(|c| (c.header(), c.masked_value()));
    let request_headers = sent_headers(
        backend,
        &request.headers,
        shown,
        request.body.len(),
        SecretView::Masked,
    );
    let models = match client.send(request).await {
        Ok(upstream) => {
            let status = upstream.response.status().as_u16();
            let latency = upstream.latency;
            let headers = upstream
                .response
                .headers()
                .iter()
                .map(|(name, value)| (name.to_string(), header_text(value).to_owned()))
                .collect();
            let body = upstream.body_bytes().await.unwrap_or_default();
            let mut models = translate::models(backend.kind, &body);
            let detail = if (200..300).contains(&status) {
                None
            } else {
                Some(translate::failure(backend.kind, Some(status), &body).message)
            };
            if detail.is_none()
                && !models.is_empty()
                && models.iter().all(|m| m.context_window.is_none())
            {
                fill_native_context(client, backend, &mut models).await;
            }
            ModelsProbe::Answered {
                status,
                latency,
                headers,
                models,
                detail,
            }
        }
        Err(error) => ModelsProbe::Failed(error),
    };
    Probe {
        credential: Ok(description),
        request_headers,
        models: Some(models),
    }
}

/// LM Studio gives context lengths only in its native `/api/v0/models`.
/// Any failure there leaves them unknown.
async fn fill_native_context(client: &UpstreamClient, backend: &Backend, models: &mut [Model]) {
    let Ok(upstream) = client.send(list_request(backend, "/api/v0/models")).await else {
        return;
    };
    if !upstream.response.status().is_success() {
        return;
    }
    let Ok(body) = upstream.body_bytes().await else {
        return;
    };
    let native: HashMap<String, u64> = translate::models(backend.kind, &body)
        .into_iter()
        .filter_map(|m| Some((m.id, m.context_window?)))
        .collect();
    for model in models {
        model.context_window = native.get(&model.id).copied();
    }
}

fn list_request<'a>(backend: &'a Backend, path: &'a str) -> UpstreamRequest<'a> {
    // What Claude Code always sends; backend `headers` still override.
    let mut base = HeaderMap::new();
    base.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
    base.insert(
        http::header::ACCEPT,
        HeaderValue::from_static("application/json"),
    );
    UpstreamRequest {
        backend,
        method: Method::GET,
        path_and_query: path,
        headers: upstream_headers(&base, backend),
        body: Bytes::new(),
        stream: false,
    }
}

/// Probes every backend concurrently; results are in the same order as
/// `backends`.
pub async fn probe_all<'a>(
    client: &UpstreamClient,
    backends: impl IntoIterator<Item = &'a Backend>,
) -> Vec<Probe> {
    futures_util::future::join_all(backends.into_iter().map(|b| probe(client, b))).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::Barrier;

    use crate::config::{BackendConfig, BackendKind, CredentialConfig, CredentialHeader};
    use crate::upstream::{HeaderSource, RetryPolicy};

    fn client() -> UpstreamClient {
        UpstreamClient::new(reqwest::Client::new(), RetryPolicy::never())
    }

    #[tokio::test]
    async fn passthrough_is_not_probed() {
        let backend = Backend::from_config(
            "account",
            &BackendConfig {
                kind: BackendKind::Passthrough,
                url: "http://127.0.0.1:1".into(),
                models_path: BackendConfig::default_models_path(),
                live_models: false,
                credential: CredentialConfig::None,
                headers: Default::default(),
                anthropic_beta: Vec::new(),
                drop_headers: Vec::new(),
                drop_fields: Vec::new(),
                proxy: None,
            },
        )
        .unwrap();
        let probe = super::probe(&client(), &backend).await;
        match probe.models {
            Some(ModelsProbe::Skipped { reason }) => {
                assert!(reason.contains("Authorization"), "{reason}");
            }
            other => panic!("expected skip, got {other:?}"),
        }
    }

    /// A backend listing one model. With a `gate`, it answers only once every
    /// party has reached it, which no sequence of requests can satisfy.
    async fn backend(name: &str, gate: Option<Arc<Barrier>>) -> Backend {
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(move || {
                let gate = gate.clone();
                async move {
                    if let Some(gate) = gate {
                        gate.wait().await;
                    }
                    axum::Json(serde_json::json!({"data": [{"id": "m"}]}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Backend::from_config(
            name,
            &BackendConfig {
                kind: BackendKind::Anthropic,
                url,
                models_path: BackendConfig::default_models_path(),
                live_models: false,
                credential: CredentialConfig::None,
                headers: Default::default(),
                anthropic_beta: Vec::new(),
                drop_headers: Vec::new(),
                drop_fields: Vec::new(),
                proxy: None,
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn probe_all_runs_backends_concurrently_in_order() {
        let gate = Arc::new(Barrier::new(2));
        let a = backend("a", Some(gate.clone())).await;
        let b = backend("b", Some(gate)).await;
        let probes = tokio::time::timeout(Duration::from_secs(10), probe_all(&client(), [&a, &b]))
            .await
            .expect("the two probes never overlapped");
        assert_eq!(probes.len(), 2);
        for p in &probes {
            match &p.models {
                Some(ModelsProbe::Answered { status, models, .. }) => {
                    assert_eq!(*status, 200);
                    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
                    assert_eq!(ids, ["m"]);
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn request_headers_say_where_each_came_from_with_secrets_masked() {
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(|| async {
                (
                    [("x-served-by", "gw-1")],
                    axum::Json(serde_json::json!({"data": []})),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let authority = listener.local_addr().unwrap().to_string();
        let url = format!("http://{authority}");
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let backend = Backend::from_config(
            "gw",
            &BackendConfig {
                kind: BackendKind::Anthropic,
                url,
                models_path: BackendConfig::default_models_path(),
                live_models: false,
                credential: CredentialConfig::Static {
                    value: "key-1234567890".into(),
                    header: CredentialHeader::bearer(),
                },
                headers: [
                    (
                        "user-agent".to_owned(),
                        "claude-cli/2.0.0 (external, cli)".to_owned(),
                    ),
                    ("anthropic-version".to_owned(), "2023-01-01".to_owned()),
                ]
                .into(),
                anthropic_beta: vec!["oauth-2025-04-20".into()],
                drop_headers: Vec::new(),
                drop_fields: Vec::new(),
                proxy: None,
            },
        )
        .unwrap();

        let probe = probe(&client(), &backend).await;
        let sent: Vec<(&str, &str, HeaderSource)> = probe
            .request_headers
            .iter()
            .map(|h| (h.name.as_str(), h.value.as_str(), h.source))
            .collect();
        assert_eq!(
            sent,
            [
                ("accept", "application/json", HeaderSource::Default),
                ("anthropic-beta", "oauth-2025-04-20", HeaderSource::Backend),
                ("anthropic-version", "2023…1-01", HeaderSource::Backend),
                (
                    "authorization",
                    "Bearer key-…7890",
                    HeaderSource::Credential
                ),
                ("host", authority.as_str(), HeaderSource::Transport),
                ("user-agent", "clau…cli)", HeaderSource::Backend),
            ]
        );
        match probe.models {
            Some(ModelsProbe::Answered { headers, .. }) => assert!(
                headers.contains(&("x-served-by".to_owned(), "gw-1".to_owned())),
                "{headers:?}"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn credential_line_is_the_source_plus_the_masked_value() {
        let mut with_key = backend("a", None).await;
        with_key.credential = crate::credential::build(&CredentialConfig::Static {
            value: "key-1234567890".into(),
            header: CredentialHeader::x_api_key(),
        })
        .unwrap();
        let probe = probe(&client(), &with_key).await;
        assert_eq!(probe.credential.unwrap(), "static (key-…7890)");
        assert!(matches!(
            probe.models,
            Some(ModelsProbe::Answered { status: 200, .. })
        ));

        let none = backend("b", None).await;
        assert_eq!(
            super::probe(&client(), &none).await.credential.unwrap(),
            "none"
        );
    }

    #[tokio::test]
    async fn the_model_list_is_fetched_from_the_path_the_backend_names() {
        let app = axum::Router::new().route(
            "/llm/api/models",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({"data": [{"id": "in-house"}]}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let backend = Backend::from_config(
            "gw",
            &BackendConfig {
                kind: BackendKind::OpenAi,
                url,
                models_path: "/llm/api/models".into(),
                live_models: false,
                credential: CredentialConfig::None,
                headers: Default::default(),
                anthropic_beta: Vec::new(),
                drop_headers: Vec::new(),
                drop_fields: Vec::new(),
                proxy: None,
            },
        )
        .unwrap();

        match probe(&client(), &backend).await.models {
            Some(ModelsProbe::Answered {
                status: 200,
                models,
                ..
            }) => assert_eq!(ids(models), ["in-house"]),
            other => panic!("{other:?}"),
        }
    }

    fn ids(models: Vec<Model>) -> Vec<String> {
        models.into_iter().map(|m| m.id).collect()
    }

    /// A backend listing `gemma` without a context length, with LM Studio's
    /// native list at `/api/v0/models` when `native` is set.
    async fn lm_studio(native: bool) -> Backend {
        let mut app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({"data": [{"id": "gemma", "object": "model"}, {"id": "other"}]}))
            }),
        );
        if native {
            app = app.route(
                "/api/v0/models",
                axum::routing::get(|| async {
                    axum::Json(serde_json::json!({"data": [
                        {"id": "gemma", "max_context_length": 131072, "loaded_context_length": 32768},
                        {"id": "unlisted", "max_context_length": 4096}
                    ]}))
                }),
            );
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Backend::from_config(
            "lmstudio",
            &BackendConfig {
                kind: BackendKind::OpenAi,
                url,
                models_path: BackendConfig::default_models_path(),
                live_models: false,
                credential: CredentialConfig::None,
                headers: Default::default(),
                anthropic_beta: Vec::new(),
                drop_headers: Vec::new(),
                drop_fields: Vec::new(),
                proxy: None,
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn lm_studio_context_comes_from_its_native_list() {
        let probe = probe(&client(), &lm_studio(true).await).await;
        match probe.models {
            Some(ModelsProbe::Answered {
                status: 200,
                models,
                detail: None,
                ..
            }) => assert_eq!(
                models
                    .iter()
                    .map(|m| (m.id.as_str(), m.context_window))
                    .collect::<Vec<_>>(),
                [("gemma", Some(32768)), ("other", None)]
            ),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn without_a_native_list_context_stays_unknown() {
        let probe = probe(&client(), &lm_studio(false).await).await;
        match probe.models {
            Some(ModelsProbe::Answered {
                status: 200,
                models,
                detail: None,
                ..
            }) => assert!(
                models.iter().all(|m| m.context_window.is_none()),
                "{models:?}"
            ),
            other => panic!("{other:?}"),
        }
    }
}
