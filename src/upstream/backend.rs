use http::{HeaderMap, HeaderName, HeaderValue};

use crate::config::{BackendConfig, BackendKind};
use crate::credential::{self, CredentialError, CredentialSource};

/// A configured backend with its resolved credential source.
#[derive(Debug)]
pub struct Backend {
    pub name: String,
    pub kind: BackendKind,
    /// Origin without trailing slash.
    pub url: String,
    /// Path the probe fetches the model list from.
    pub models_path: String,
    pub credential: Box<dyn CredentialSource>,
    /// Headers forced onto every upstream request, values marked sensitive.
    pub headers: HeaderMap,
    /// Beta flags merged into `anthropic-beta`.
    pub anthropic_beta: Vec<String>,
    /// Body paths removed before forwarding.
    pub drop_fields: Vec<String>,
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
            let mut value = HeaderValue::from_str(value).expect("validated header value");
            value.set_sensitive(true);
            headers.insert(key, value);
        }
        Ok(Self {
            name: name.to_owned(),
            kind: config.kind,
            url: config.url.clone(),
            models_path: config.models_path.clone(),
            credential,
            headers,
            anthropic_beta: config.anthropic_beta.clone(),
            drop_fields: config.drop_fields.clone(),
        })
    }
}
