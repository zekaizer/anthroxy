use http::{HeaderMap, HeaderName, HeaderValue};

use crate::config::BackendConfig;
use crate::credential::{self, CredentialError, CredentialSource};

/// A configured backend with its resolved credential source.
#[derive(Debug)]
pub struct Backend {
    pub name: String,
    /// Origin without trailing slash.
    pub url: String,
    pub credential: Box<dyn CredentialSource>,
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

impl Backend {
    /// Assumes `config` passed validation (header names and values are valid).
    pub fn from_config(name: &str, config: &BackendConfig) -> Result<Self, BackendBuildError> {
        let credential =
            credential::build(&config.credential).map_err(|source| BackendBuildError {
                backend: name.to_owned(),
                source,
            })?;
        let mut headers = HeaderMap::new();
        for (key, value) in &config.headers {
            let key = HeaderName::from_bytes(key.as_bytes()).expect("validated header name");
            let value = HeaderValue::from_str(value).expect("validated header value");
            headers.insert(key, value);
        }
        Ok(Self {
            name: name.to_owned(),
            url: config.url.clone(),
            credential,
            headers,
            anthropic_beta: config.anthropic_beta.clone(),
        })
    }
}
