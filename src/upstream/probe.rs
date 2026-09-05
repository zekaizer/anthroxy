//! Reachability check used by `claude-router check`: acquire the credential
//! and call `GET /v1/models` on the backend.

use std::time::{Duration, Instant};

use super::{Backend, upstream_headers};
use crate::credential::CredentialError;

#[derive(Debug)]
pub struct Probe {
    /// Credential source description with the value masked.
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
        Ok(Some(c)) => Ok(format!(
            "{} ({})",
            backend.credential.describe(),
            c.masked()
        )),
        Ok(None) => Ok(backend.credential.describe()),
        Err(e) => Err(e),
    };
    let Ok(credential_value) = backend.credential.credential().await else {
        return Probe {
            credential,
            models: None,
        };
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
    let headers = upstream_headers(&base, backend, credential_value.as_ref());
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
        credential,
        models: Some(models),
    }
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
