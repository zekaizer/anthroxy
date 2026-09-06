use std::collections::BTreeMap;
use std::sync::Arc;

use http::{HeaderMap, HeaderName, HeaderValue};

use crate::config::Config;
use crate::credential::{self, CredentialError, CredentialSource};

/// A configured backend with its resolved credential source.
#[derive(Debug)]
pub struct Backend {
    pub name: String,
    /// Origin without trailing slash.
    pub url: String,
    pub credential: Arc<dyn CredentialSource>,
    /// Headers forced onto every upstream request.
    pub headers: HeaderMap,
    /// Beta flags merged into `anthropic-beta`.
    pub anthropic_beta: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("backend `{backend}`: {source}")]
pub struct BackendBuildError {
    pub backend: String,
    #[source]
    pub source: CredentialError,
}

/// All backends by name.
#[derive(Debug, Default)]
pub struct Backends {
    map: BTreeMap<String, Arc<Backend>>,
}

impl Backends {
    /// Assumes `config` passed validation (header names are valid).
    pub fn from_config(config: &Config) -> Result<Self, BackendBuildError> {
        let mut map = BTreeMap::new();
        for (name, cfg) in &config.backends {
            let credential =
                credential::build(&cfg.credential).map_err(|source| BackendBuildError {
                    backend: name.clone(),
                    source,
                })?;
            let mut headers = HeaderMap::new();
            for (key, value) in &cfg.headers {
                let key = HeaderName::from_bytes(key.as_bytes()).expect("validated header name");
                let value = HeaderValue::from_str(value).expect("validated header value");
                headers.insert(key, value);
            }
            map.insert(
                name.clone(),
                Arc::new(Backend {
                    name: name.clone(),
                    url: cfg.url.clone(),
                    credential,
                    headers,
                    anthropic_beta: cfg.anthropic_beta.clone(),
                }),
            );
        }
        Ok(Self { map })
    }

    pub fn get(&self, name: &str) -> Option<&Arc<Backend>> {
        self.map.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<Backend>> {
        self.map.values()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}
