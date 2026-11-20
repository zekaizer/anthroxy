use std::sync::Arc;

use super::dialect::{Dialect, Target};
use super::header_command::HeaderCommand;
use crate::config::{BackendConfig, BackendKind};
use crate::credential::{self, CredentialError, CredentialSource};

/// A configured backend with its resolved credential source.
#[derive(Debug)]
pub struct Backend {
    pub name: String,
    pub kind: BackendKind,
    /// Origin without trailing slash. Connections are opened to it, so it
    /// carries whatever the file said, userinfo included; anything shown to
    /// someone reads [`Self::shown_url`] instead.
    pub url: String,
    /// Path the probe fetches the model list from.
    pub models_path: String,
    pub credential: Box<dyn CredentialSource>,
    /// Headers computed for every attempt (ADR-0021).
    pub header_command: Option<HeaderCommand>,
    /// Beta flags merged into `anthropic-beta`.
    pub anthropic_beta: Vec<String>,
    /// Body paths removed before forwarding.
    pub drop_fields: Vec<String>,
    /// `kind = "passthrough"`: send the client's `Authorization` / `x-api-key`.
    pub forwards_client_auth: bool,
    /// Serve `GET {url}{models_path}` as Anthropic model identity.
    pub live_models: bool,
    /// How a request no route makes is written: the model list, a probe.
    pub dialect: Arc<Dialect>,
}

#[derive(Debug, thiserror::Error)]
#[error("backend `{backend}`: {source}")]
pub struct BackendBuildError {
    pub backend: String,
    #[source]
    pub source: CredentialError,
}

impl Backend {
    /// The URL with its userinfo, which may be a password, redacted.
    pub fn shown_url(&self) -> String {
        crate::config::view::redacted_url(&self.url)
    }

    /// `dialect` is the one built for `config` without a model. Assumes
    /// `config` passed validation.
    pub fn from_config(
        name: &str,
        config: &BackendConfig,
        dialect: Arc<Dialect>,
    ) -> Result<Self, BackendBuildError> {
        let credential =
            credential::build(&config.credential).map_err(|source| BackendBuildError {
                backend: name.to_owned(),
                source,
            })?;
        Ok(Self {
            name: name.to_owned(),
            kind: config.kind,
            url: config.url.clone(),
            models_path: config.models_path.clone(),
            header_command: config.header_command.as_ref().map(|command| {
                HeaderCommand::new(command, credential.header().map(|header| header.name))
            }),
            credential,
            anthropic_beta: config.anthropic_beta.clone(),
            drop_fields: config.drop_fields.clone(),
            forwards_client_auth: config.kind == BackendKind::Passthrough,
            live_models: config.kind == BackendKind::Passthrough || config.live_models,
            dialect,
        })
    }

    /// For a request no route makes: the model list, a probe.
    pub fn target(&self) -> Target<'_> {
        Target::new(self, &self.dialect.headers)
    }
}
