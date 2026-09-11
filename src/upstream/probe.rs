//! Reachability check used by `anthroxy check`: acquire the credential
//! and call `GET /v1/models` on the backend.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method};

use super::headers::ANTHROPIC_BETA;
use super::{Backend, UpstreamClient, UpstreamError, UpstreamRequest, upstream_headers};
use crate::credential::{Credential, CredentialError, mask};

#[derive(Debug)]
pub struct Probe {
    /// Credential source, plus the masked value when there is one.
    pub credential: Result<String, CredentialError>,
    /// What the router set on `GET /v1/models`, by name; empty when no
    /// credential could be had and nothing was sent.
    pub request_headers: Vec<SentHeader>,
    pub models: Option<ModelsProbe>,
}

/// One request header as the report shows it: backend-forced values and the
/// credential masked, the router's own defaults as sent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SentHeader {
    pub name: String,
    pub value: String,
    pub source: HeaderSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeaderSource {
    /// What Claude Code would send.
    Default,
    /// The backend's `headers` or `anthropic_beta`.
    Backend,
    Credential,
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
        models: Vec<ListedModel>,
        detail: Option<String>,
    },
    Failed(UpstreamError),
}

/// One entry of a backend's model list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedModel {
    pub id: String,
    /// Tokens the server accepts for this model, when its list says.
    pub context_length: Option<u64>,
}

/// Never fails: every outcome is data for the report. `client` decides the
/// timeouts and retries; `check` uses none.
pub async fn probe(client: &UpstreamClient, backend: &Backend) -> Probe {
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
    let request = list_request(backend, "/v1/models");
    let request_headers = sent_headers(backend, &request.headers, credential.as_ref());
    let models = match client.send(request).await {
        Ok(upstream) => {
            let status = upstream.response.status().as_u16();
            let headers = upstream
                .response
                .headers()
                .iter()
                .map(|(name, value)| (name.to_string(), header_text(value).to_owned()))
                .collect();
            let body = upstream.response.bytes().await.unwrap_or_default();
            let json = serde_json::from_slice::<serde_json::Value>(&body).ok();
            let mut models = json.as_ref().map(listed_models).unwrap_or_default();
            let detail = if (200..300).contains(&status) {
                None
            } else {
                Some(error_detail(json.as_ref(), &body))
            };
            if detail.is_none()
                && !models.is_empty()
                && models.iter().all(|m| m.context_length.is_none())
            {
                fill_native_context(client, backend, &mut models).await;
            }
            ModelsProbe::Answered {
                status,
                latency: upstream.latency,
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
async fn fill_native_context(
    client: &UpstreamClient,
    backend: &Backend,
    models: &mut [ListedModel],
) {
    let Ok(upstream) = client.send(list_request(backend, "/api/v0/models")).await else {
        return;
    };
    if !upstream.response.status().is_success() {
        return;
    }
    let Ok(body) = upstream.response.bytes().await else {
        return;
    };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return;
    };
    let native: HashMap<String, u64> = listed_models(&json)
        .into_iter()
        .filter_map(|m| Some((m.id, m.context_length?)))
        .collect();
    for model in models {
        model.context_length = native.get(&model.id).copied();
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
    }
}

/// `headers` plus the credential the send adds on top, as the report shows
/// them: a value the backend forces may be a secret and is masked like the
/// credential.
fn sent_headers(
    backend: &Backend,
    headers: &HeaderMap,
    credential: Option<&Credential>,
) -> Vec<SentHeader> {
    let mut sent = BTreeMap::new();
    for name in headers.keys() {
        let value = headers
            .get_all(name)
            .iter()
            .map(header_text)
            .collect::<Vec<_>>()
            .join(", ");
        let (value, source) = if backend.headers.contains_key(name) {
            (mask(&value), HeaderSource::Backend)
        } else if *name == ANTHROPIC_BETA && !backend.anthropic_beta.is_empty() {
            (value, HeaderSource::Backend)
        } else {
            (value, HeaderSource::Default)
        };
        sent.insert(name.to_string(), (value, source));
    }
    if let Some(credential) = credential {
        let (name, _) = credential.header_pair();
        sent.insert(
            name.to_string(),
            (credential.masked_value(), HeaderSource::Credential),
        );
    }
    sent.into_iter()
        .map(|(name, (value, source))| SentHeader {
            name,
            value,
            source,
        })
        .collect()
}

fn header_text(value: &HeaderValue) -> &str {
    value.to_str().unwrap_or("<binary>")
}

/// Probes every backend concurrently; results are in the same order as
/// `backends`.
pub async fn probe_all<'a>(
    client: &UpstreamClient,
    backends: impl IntoIterator<Item = &'a Backend>,
) -> Vec<Probe> {
    futures_util::future::join_all(backends.into_iter().map(|b| probe(client, b))).await
}

/// `error.message` of an Anthropic error, else the first line of the body.
fn error_detail(json: Option<&serde_json::Value>, body: &[u8]) -> String {
    json.and_then(|j| j.pointer("/error/message"))
        .and_then(|m| m.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let text = String::from_utf8_lossy(body);
            text.lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(160)
                .collect()
        })
}

/// Fields model lists use for the context window, most specific first: LM
/// Studio's loaded context before its maximum, vLLM's `max_model_len`.
const CONTEXT_FIELDS: [&str; 6] = [
    "loaded_context_length",
    "max_model_len",
    "context_length",
    "max_context_length",
    "context_window",
    "max_input_tokens",
];

/// `data[]` of either an Anthropic or an OpenAI model list.
fn listed_models(body: &serde_json::Value) -> Vec<ListedModel> {
    body.get("data")
        .and_then(|d| d.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|m| {
                    Some(ListedModel {
                        id: m.get("id")?.as_str()?.to_owned(),
                        context_length: CONTEXT_FIELDS
                            .iter()
                            .find_map(|field| m.get(*field)?.as_u64()),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::Barrier;

    use crate::config::{BackendConfig, BackendKind, CredentialConfig, CredentialHeader};
    use crate::upstream::RetryPolicy;

    fn client() -> UpstreamClient {
        UpstreamClient::new(reqwest::Client::new(), RetryPolicy::never())
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
                credential: CredentialConfig::None,
                headers: Default::default(),
                anthropic_beta: Vec::new(),
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
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let backend = Backend::from_config(
            "gw",
            &BackendConfig {
                kind: BackendKind::Anthropic,
                url,
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

    fn ids(models: Vec<ListedModel>) -> Vec<String> {
        models.into_iter().map(|m| m.id).collect()
    }

    #[test]
    fn extracts_ids_from_both_list_shapes() {
        let anthropic =
            serde_json::json!({"data": [{"id": "a", "type": "model"}], "has_more": false});
        let openai = serde_json::json!({"object": "list", "data": [{"id": "x", "object": "model"}, {"id": "y"}]});
        assert_eq!(ids(listed_models(&anthropic)), vec!["a"]);
        assert_eq!(ids(listed_models(&openai)), vec!["x", "y"]);
        assert!(listed_models(&serde_json::json!({"error": "nope"})).is_empty());
    }

    #[test]
    fn context_lengths_come_from_the_fields_servers_use() {
        let list = serde_json::json!({"data": [
            {"id": "vllm", "max_model_len": 32768},
            {"id": "lmstudio", "max_context_length": 131072, "loaded_context_length": 8192},
            {"id": "openrouter", "context_length": 200000},
            {"id": "plain"},
            {"id": "odd", "max_model_len": "big"}
        ]});
        let context: Vec<(String, Option<u64>)> = listed_models(&list)
            .into_iter()
            .map(|m| (m.id, m.context_length))
            .collect();
        assert_eq!(
            context,
            [
                ("vllm".to_owned(), Some(32768)),
                ("lmstudio".to_owned(), Some(8192)),
                ("openrouter".to_owned(), Some(200000)),
                ("plain".to_owned(), None),
                ("odd".to_owned(), None)
            ]
        );
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
                credential: CredentialConfig::None,
                headers: Default::default(),
                anthropic_beta: Vec::new(),
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
                models,
                [
                    ListedModel {
                        id: "gemma".into(),
                        context_length: Some(32768)
                    },
                    ListedModel {
                        id: "other".into(),
                        context_length: None
                    }
                ]
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
                models.iter().all(|m| m.context_length.is_none()),
                "{models:?}"
            ),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn error_detail_prefers_anthropic_message() {
        let json = serde_json::json!({"type":"error","error":{"type":"invalid_request_error","message":"anthropic-version header is required"}});
        assert_eq!(
            error_detail(Some(&json), b"{}"),
            "anthropic-version header is required"
        );
        assert_eq!(error_detail(None, b"<html>\nnope"), "<html>");
    }
}
