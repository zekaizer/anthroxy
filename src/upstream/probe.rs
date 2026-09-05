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
    /// The backend answered; `ids` is filled when the body listed models.
    Answered {
        status: u16,
        latency: Duration,
        ids: Vec<String>,
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
    let headers = upstream_headers(&http::HeaderMap::new(), backend, credential_value.as_ref());
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
            let ids = response
                .bytes()
                .await
                .ok()
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                .map(model_ids)
                .unwrap_or_default();
            ModelsProbe::Answered {
                status,
                latency,
                ids,
            }
        }
        Err(error) => ModelsProbe::Unreachable(super::client::describe(&error)),
    };
    Probe {
        credential,
        models: Some(models),
    }
}

/// `data[].id` of either an Anthropic or an OpenAI model list.
fn model_ids(body: serde_json::Value) -> Vec<String> {
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
        assert_eq!(model_ids(anthropic), vec!["a"]);
        assert_eq!(model_ids(openai), vec!["x", "y"]);
        assert!(model_ids(serde_json::json!({"error": "nope"})).is_empty());
    }
}
