//! Router configuration: schema, loading and validation.
//!
//! A configuration is a TOML document (see ADR-0002). `${NAME}` references are
//! replaced with environment variables before parsing; `$${NAME}` yields a
//! literal `${NAME}`.

mod byte_size;
mod env;
mod error;
mod schema;
mod validate;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

pub use error::ConfigError;
pub use schema::{
    BackendConfig, Config, CredentialConfig, CredentialHeader, LogFormat, LoggingConfig,
    ModelConfig, RoutingConfig, ServerConfig, UpstreamConfig,
};

/// Environment variable naming the configuration file.
pub const CONFIG_ENV: &str = "CLAUDE_ROUTER_CONFIG";

/// Default configuration path: `$XDG_CONFIG_HOME/claude-router/config.toml`.
pub fn default_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claude-router")
        .join("config.toml")
}

impl Config {
    /// Reads, expands and validates the file at `path`.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Config::parse(&text, |name| std::env::var(name).ok())
    }

    /// Parses `text`, resolving `${NAME}` through `lookup`, then validates.
    pub fn parse(
        text: &str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Config, ConfigError> {
        let expanded = env::expand(text, lookup)?;
        let mut config: Config =
            toml::from_str(&expanded).map_err(|e| ConfigError::Parse(e.to_string()))?;
        normalize(&mut config);
        validate::validate(&config)?;
        Ok(config)
    }
}

/// Canonical forms that later layers rely on: backend URLs carry no trailing
/// slash, so `url + path` never produces `//`.
fn normalize(config: &mut Config) {
    for backend in config.backends.values_mut() {
        while backend.url.ends_with('/') {
            backend.url.pop();
        }
    }
}
