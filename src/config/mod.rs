//! Router configuration: schema, loading and validation.
//!
//! A configuration is a TOML document (see ADR-0002). `${NAME}` inside any
//! string value is replaced with the environment variable `NAME`; `$${NAME}`
//! yields a literal `${NAME}`. Keys and comments are never expanded.

mod byte_size;
mod credential_header;
mod env;
mod error;
pub mod example;
mod schema;
mod validate;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

pub use credential_header::{CredentialHeader, X_API_KEY};
pub use error::ConfigError;
pub use schema::{
    BackendConfig, BackendKind, CommandOutput, Config, CredentialConfig, LogFormat, LoggingConfig,
    ModelConfig, RoutingConfig, ServerConfig, UpstreamConfig,
};

/// Environment variable naming the configuration file.
pub const CONFIG_ENV: &str = "ANTHROXY_CONFIG";

/// The `lookup` for [`Config::parse`] that reads the process environment.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Default configuration path: `$XDG_CONFIG_HOME/anthroxy/config.toml`.
pub fn default_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("anthroxy")
        .join("config.toml")
}

impl Config {
    /// Reads, expands and validates the file at `path`.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Config::parse(&text, process_env)
    }

    /// Parses `text`, resolving `${NAME}` in string values through `lookup`,
    /// then validates.
    pub fn parse(
        text: &str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Config, ConfigError> {
        let mut document: toml::Value =
            toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        env::expand_value(&mut document, &lookup)?;
        let mut config: Config = document
            .try_into()
            .map_err(|e: toml::de::Error| ConfigError::Parse(e.to_string()))?;
        normalize(&mut config);
        validate::validate(&config)?;
        Ok(config)
    }
}

/// Command-line values that replace what the file says.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Overrides {
    pub listen: Option<std::net::SocketAddr>,
    pub body_dir: Option<PathBuf>,
}

impl Config {
    /// Applies `overrides`, then normalizes and validates again so an
    /// override obeys the same rules as the file.
    pub fn with_overrides(mut self, overrides: &Overrides) -> Result<Config, ConfigError> {
        if let Some(listen) = overrides.listen {
            self.server.listen = listen;
        }
        if let Some(dir) = &overrides.body_dir {
            self.logging.body_dir = Some(dir.clone());
        }
        normalize(&mut self);
        validate::validate(&self)?;
        Ok(self)
    }
}

/// Canonical forms that later layers rely on: backend URLs carry no trailing
/// slash, so `url + path` never produces `//`; a leading `~/` in paths means
/// the home directory.
fn normalize(config: &mut Config) {
    for backend in config.backends.values_mut() {
        while backend.url.ends_with('/') {
            backend.url.pop();
        }
    }
    expand_home(&mut config.logging.body_dir);
    expand_home(&mut config.upstream.ca_certificate);
}

fn expand_home(path: &mut Option<PathBuf>) {
    if let Some(p) = path
        && let Ok(rest) = p.strip_prefix("~")
        && let Some(home) = dirs::home_dir()
    {
        *p = home.join(rest);
    }
}
