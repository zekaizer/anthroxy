//! Router configuration: schema, loading and validation.
//!
//! A configuration is a TOML document (see ADR-0002). `${NAME}` inside any
//! string value is replaced with the environment variable `NAME`; `$${NAME}`
//! yields a literal `${NAME}`. Keys and comments are never expanded.

mod byte_size;
pub mod client_env;
mod credential_header;
mod drop_headers;
mod env;
mod error;
pub mod example;
mod origin;
mod schema;
pub mod snippet;
mod validate;
pub mod view;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

pub use credential_header::{CredentialHeader, X_API_KEY};
pub use drop_headers::{CLAUDE_CODE_PRESET, DropHeaders, HeaderPattern, PatternError};
pub use error::ConfigError;
pub use origin::origin;
pub use schema::{
    BackendConfig, BackendKind, CommandOutput, Config, CredentialConfig, LogFormat, LoggingConfig,
    ModelConfig, RoutingConfig, ServerConfig, StatsConfig, UpstreamConfig, V1Auth,
};

/// Environment variable naming the configuration file.
pub const CONFIG_ENV: &str = "ANTHROXY_CONFIG";

/// The `lookup` for [`Config::parse`] that reads the process environment.
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// The mode of `path` when it lets anyone but its owner near it. The file
/// carries `server.token` and any `static` backend credential, so the mode a
/// stock umask leaves on a hand-written file hands both to every local
/// account. `None` when the file is the owner's alone, cannot be read, or the
/// platform has no such modes.
#[cfg(unix)]
pub fn open_to_other_accounts(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;

    let mode = std::fs::metadata(path).ok()?.permissions().mode() & 0o777;
    (mode & 0o077 != 0).then_some(mode)
}

#[cfg(not(unix))]
pub fn open_to_other_accounts(_: &Path) -> Option<u32> {
    None
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
    if let Some(dir) = &mut config.logging.body_dir {
        expand_home(dir);
    }
    if let Some(file) = &mut config.upstream.ca_certificate {
        expand_home(file);
    }
    expand_home(&mut config.stats.dir);
}

fn expand_home(path: &mut PathBuf) {
    if let Ok(rest) = path.strip_prefix("~")
        && let Some(home) = dirs::home_dir()
    {
        *path = home.join(rest);
    }
}
