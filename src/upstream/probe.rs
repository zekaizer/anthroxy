//! Reachability check used by `anthroxy check`: acquire the credential
//! and call `GET /v1/models` on the backend.

use std::time::{Duration, Instant};

use super::{Backend, upstream_headers};
use crate::credential::CredentialError;

#[derive(Debug)]
pub struct Probe {
    /// Credential source, plus the masked value when there is one.
    pub credential: Result<String, CredentialError>,
    pub models: Option<ModelsProbe>,
}

#[derive(Debug)]
pub enum ModelsProbe {
    /// The backend answered; `ids` is filled when the body listed models,
    /// `detail` when it carried an error message.
    Answered {
        status: u16,
        latency: Duration,
        ids: Vec<String>,
        detail: Option<String>,
    },
    Unreachable(String),
}

/// Never fails: every outcome is data for the report.
pub async fn probe(http: &reqwest::Client, backend: &Backend) -> Probe {
    let credential = match backend.credential.credential().await {
        Ok(credential) => credential,
        Err(error) => {
            return Probe {
                credential: Err(error),
                models: None,
            };
        }
    };
    let description = match &credential {
        Some(c) => format!("{} ({})", backend.credential.describe(), c.masked()),
        None => backend.credential.describe(),
    };
    // What Claude Code always sends; backend `headers` still override.
    let mut base = http::HeaderMap::new();
    base.insert(
        "anthropic-version",
        http::HeaderValue::from_static("2023-06-01"),
    );
    base.insert(
        http::header::ACCEPT,
        http::HeaderValue::from_static("application/json"),
    );
    let mut headers = upstream_headers(&base, backend);
    if let Some(credential) = &credential {
        let (name, value) = credential.header_pair();
        headers.insert(name, value);
    }
    let started = Instant::now();
    let models = match http
        .get(format!("{}/v1/models", backend.url))
        .headers(headers)
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status().as_u16();
            let latency = started.elapsed();
            let body = response.bytes().await.unwrap_or_default();
            let json = serde_json::from_slice::<serde_json::Value>(&body).ok();
            let ids = json.as_ref().map(model_ids).unwrap_or_default();
            let detail = if (200..300).contains(&status) {
                None
            } else {
                Some(error_detail(json.as_ref(), &body))
            };
            ModelsProbe::Answered {
                status,
                latency,
                ids,
                detail,
            }
        }
        Err(error) => ModelsProbe::Unreachable(super::client::describe(&error)),
    };
    Probe {
        credential: Ok(description),
        models: Some(models),
    }
}

/// Probes every backend concurrently; results are in the same order as
/// `backends`.
pub async fn probe_all<'a>(
    http: &reqwest::Client,
    backends: impl IntoIterator<Item = &'a Backend>,
) -> Vec<Probe> {
    futures_util::future::join_all(backends.into_iter().map(|b| probe(http, b))).await
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

/// `data[].id` of either an Anthropic or an OpenAI model list.
fn model_ids(body: &serde_json::Value) -> Vec<String> {
    body.get("data")
        .and_then(|d| d.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use crate::config::{BackendConfig, CredentialConfig, CredentialHeader};

    async fn slow_backend(name: &str, delay: Duration) -> Backend {
        let app = axum::Router::new().route(
            "/v1/models",
            axum::routing::get(move || async move {
                tokio::time::sleep(delay).await;
                axum::Json(serde_json::json!({"data": [{"id": "m"}]}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Backend::from_config(
            name,
            &BackendConfig {
                url,
                credential: CredentialConfig::None,
                headers: Default::default(),
                anthropic_beta: Vec::new(),
            },
        )
        .unwrap()
    }

    #[tokio::test]
    async fn probe_all_runs_backends_concurrently_in_order() {
        let a = slow_backend("a", Duration::from_millis(300)).await;
        let b = slow_backend("b", Duration::from_millis(300)).await;
        let http = reqwest::Client::new();
        let started = Instant::now();
        let probes = probe_all(&http, [&a, &b]).await;
        let elapsed = started.elapsed();
        assert_eq!(probes.len(), 2);
        assert!(
            elapsed < Duration::from_millis(550),
            "probes ran sequentially: {elapsed:?}"
        );
        for p in &probes {
            match &p.models {
                Some(ModelsProbe::Answered { status, ids, .. }) => {
                    assert_eq!(*status, 200);
                    assert_eq!(ids, &["m"]);
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn credential_line_is_the_source_plus_the_masked_value() {
        let mut backend = slow_backend("a", Duration::ZERO).await;
        backend.credential = crate::credential::build(&CredentialConfig::Static {
            value: "key-1234567890".into(),
            header: CredentialHeader::XApiKey,
        })
        .unwrap();
        let probe = probe(&reqwest::Client::new(), &backend).await;
        assert_eq!(probe.credential.unwrap(), "static (key-…7890)");
        assert!(matches!(
            probe.models,
            Some(ModelsProbe::Answered { status: 200, .. })
        ));

        let none = slow_backend("b", Duration::ZERO).await;
        assert_eq!(
            super::probe(&reqwest::Client::new(), &none)
                .await
                .credential
                .unwrap(),
            "none"
        );
    }

    #[test]
    fn extracts_ids_from_both_list_shapes() {
        let anthropic =
            serde_json::json!({"data": [{"id": "a", "type": "model"}], "has_more": false});
        let openai = serde_json::json!({"object": "list", "data": [{"id": "x", "object": "model"}, {"id": "y"}]});
        assert_eq!(model_ids(&anthropic), vec!["a"]);
        assert_eq!(model_ids(&openai), vec!["x", "y"]);
        assert!(model_ids(&serde_json::json!({"error": "nope"})).is_empty());
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
