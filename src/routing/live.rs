//! Live model list of a `kind = "passthrough"` backend.

use std::sync::Arc;
use std::time::Duration;

use http::{HeaderMap, Method};
use tokio::sync::Mutex;
use tokio::time::Instant;

use super::Route;
use crate::anthropic::ModelObject;
use crate::upstream::{Backend, UpstreamClient, UpstreamRequest, upstream_headers};

/// How long a fetched list is reused for the same client `Authorization`.
pub const TTL: Duration = Duration::from_secs(30);

/// A list pull has its own deadline, apart from `upstream.non_stream_timeout`:
/// every request naming an unknown model waits on it.
const PULL_TIMEOUT: Duration = Duration::from_secs(10);

struct Entry {
    fetched_at: Instant,
    auth: String,
    models: Vec<ModelObject>,
}

/// Cached `GET {url}/v1/models` for one passthrough backend.
pub struct LiveCatalog {
    backend: Arc<Backend>,
    cache: Mutex<Option<Entry>>,
}

impl LiveCatalog {
    pub fn new(backend: Arc<Backend>) -> Self {
        Self {
            backend,
            cache: Mutex::new(None),
        }
    }

    pub fn backend(&self) -> &Arc<Backend> {
        &self.backend
    }

    /// Live models not shadowed by a configured id or alias.
    pub async fn models(
        &self,
        upstream: &UpstreamClient,
        client_headers: &HeaderMap,
        occupied: impl Fn(&str) -> bool,
    ) -> Vec<ModelObject> {
        self.fetch(upstream, client_headers)
            .await
            .into_iter()
            .filter(|m| !occupied(&m.id))
            .collect()
    }

    pub async fn identity(
        &self,
        name: &str,
        upstream: &UpstreamClient,
        client_headers: &HeaderMap,
        occupied: impl Fn(&str) -> bool,
    ) -> Option<ModelObject> {
        if occupied(name) {
            return None;
        }
        self.fetch(upstream, client_headers)
            .await
            .into_iter()
            .find(|m| m.id == name)
    }

    pub async fn lookup(
        &self,
        name: &str,
        upstream: &UpstreamClient,
        client_headers: &HeaderMap,
        occupied: impl Fn(&str) -> bool,
    ) -> Option<Route> {
        if occupied(name) {
            return None;
        }
        let models = self.fetch(upstream, client_headers).await;
        models.iter().find(|m| m.id == name).map(|m| Route {
            id: m.id.clone(),
            backend: Arc::clone(&self.backend),
            upstream_model: m.id.clone(),
            display_name: m.display_name.clone(),
            aliases: Vec::new(),
            mid_conversation_system: self.backend.mid_conversation_system,
        })
    }

    async fn fetch(
        &self,
        upstream: &UpstreamClient,
        client_headers: &HeaderMap,
    ) -> Vec<ModelObject> {
        let auth = if self.backend.forwards_client_auth {
            client_headers
                .get(http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_owned()
        } else {
            self.backend.name.clone()
        };
        // The lock is held across the pull, so concurrent misses share one
        // fetch instead of each asking the backend.
        let mut cache = self.cache.lock().await;
        if let Some(entry) = cache.as_ref()
            && entry.auth == auth
            && Instant::now().saturating_duration_since(entry.fetched_at) < TTL
        {
            return entry.models.clone();
        }
        let models =
            match tokio::time::timeout(PULL_TIMEOUT, self.pull(upstream, client_headers)).await {
                Ok(Ok(models)) => models,
                Ok(Err(error)) => {
                    tracing::warn!(
                        backend = %self.backend.name,
                        error = %error,
                        "live model list failed; serving configured models only"
                    );
                    Vec::new()
                }
                Err(_) => {
                    tracing::warn!(
                        backend = %self.backend.name,
                        "live model list took longer than {}; serving configured models only",
                        humantime::format_duration(PULL_TIMEOUT)
                    );
                    Vec::new()
                }
            };
        *cache = Some(Entry {
            fetched_at: Instant::now(),
            auth,
            models: models.clone(),
        });
        models
    }

    async fn pull(
        &self,
        upstream: &UpstreamClient,
        client_headers: &HeaderMap,
    ) -> Result<Vec<ModelObject>, crate::upstream::UpstreamError> {
        let headers = upstream_headers(client_headers, &self.backend);
        let upstream = upstream
            .send(UpstreamRequest {
                backend: &self.backend,
                method: Method::GET,
                path_and_query: &self.backend.models_path,
                headers,
                body: bytes::Bytes::new(),
                stream: false,
            })
            .await?;
        if !upstream.response.status().is_success() {
            tracing::warn!(
                backend = %self.backend.name,
                status = upstream.response.status().as_u16(),
                "live model list was not 2xx"
            );
            return Ok(Vec::new());
        }
        let raw = match upstream.body_bytes().await {
            Ok(raw) => raw,
            Err(crate::upstream::BodyError::TimedOut(clock)) => {
                return Err(crate::upstream::UpstreamError::TimedOut {
                    backend: self.backend.name.clone(),
                    clock,
                });
            }
            Err(crate::upstream::BodyError::Upstream(source)) => {
                return Err(crate::upstream::UpstreamError::Body {
                    backend: self.backend.name.clone(),
                    source,
                });
            }
            Err(crate::upstream::BodyError::TooLarge { limit }) => {
                return Err(crate::upstream::UpstreamError::BodyTooLarge {
                    backend: self.backend.name.clone(),
                    limit,
                });
            }
        };
        Ok(crate::translate::catalog(self.backend.kind, &raw))
    }
}
