//! Serde types mirroring the configuration file. Field defaults live here;
//! cross-field rules live in `validate`.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use super::byte_size;
use super::credential_header::CredentialHeader;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub server: ServerConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub upstream: UpstreamConfig,
    #[serde(default)]
    pub backends: BTreeMap<String, BackendConfig>,
    #[serde(default)]
    pub models: Vec<ModelConfig>,
    #[serde(default)]
    pub routing: RoutingConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Socket the router listens on. `0.0.0.0` so the Windows host can reach a
    /// WSL2 instance under both NAT and mirrored networking.
    #[serde(default = "ServerConfig::default_listen")]
    pub listen: SocketAddr,
    /// Static token Claude Code must present (`x-api-key` or bearer).
    pub token: String,
    /// Largest accepted request body.
    #[serde(default = "ServerConfig::default_max_body", with = "byte_size")]
    pub max_body_bytes: usize,
}

impl ServerConfig {
    pub fn default_listen() -> SocketAddr {
        "0.0.0.0:8787".parse().expect("valid literal")
    }
    pub const fn default_max_body() -> usize {
        64 * 1024 * 1024
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LoggingConfig {
    /// `tracing` filter directive, e.g. `info` or `anthroxy=debug,info`.
    pub level: String,
    pub format: LogFormat,
    /// When set, every proxied request and response body is written under this
    /// directory.
    pub body_dir: Option<PathBuf>,
    /// Recorded exchanges older than this are deleted; `0` keeps them forever.
    #[serde(with = "humantime_serde")]
    pub body_retention: Duration,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_owned(),
            format: LogFormat::default(),
            body_dir: None,
            body_retention: Duration::from_secs(7 * 24 * 3600),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum LogFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpstreamConfig {
    #[serde(with = "humantime_serde")]
    pub connect_timeout: Duration,
    /// Maximum silence between two chunks of an upstream response.
    #[serde(with = "humantime_serde")]
    pub read_timeout: Duration,
    /// Additional attempts after a connection failure.
    pub retries: u32,
    /// Delay before the first retry; doubles on each further attempt.
    #[serde(with = "humantime_serde")]
    pub retry_backoff: Duration,
    /// Upstream status codes that are retried like a connection failure.
    pub retry_on_status: Vec<u16>,
    /// PEM file of CA certificates trusted for HTTPS backends, on top of the
    /// bundled Mozilla roots and the OS store.
    pub ca_certificate: Option<PathBuf>,
}

impl Default for UpstreamConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            read_timeout: Duration::from_secs(300),
            retries: 2,
            retry_backoff: Duration::from_millis(200),
            retry_on_status: Vec::new(),
            ca_certificate: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    /// Origin of the backend, e.g. `http://127.0.0.1:8000`. The request path is
    /// appended unchanged.
    pub url: String,
    #[serde(default)]
    pub credential: CredentialConfig,
    /// Headers set on every upstream request, overriding the client's value.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Beta flags merged into the client's `anthropic-beta` header.
    #[serde(default)]
    pub anthropic_beta: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CredentialConfig {
    #[default]
    None,
    Static {
        value: String,
        #[serde(default)]
        header: CredentialHeader,
    },
    Env {
        name: String,
        #[serde(default)]
        header: CredentialHeader,
    },
    /// `command` runs through `sh -c`; stdout is read as `output` says.
    Command {
        command: String,
        #[serde(default)]
        output: CommandOutput,
        #[serde(
            default = "CredentialConfig::default_refresh",
            with = "humantime_serde"
        )]
        refresh: Duration,
        #[serde(
            default = "CredentialConfig::default_timeout",
            with = "humantime_serde"
        )]
        timeout: Duration,
        #[serde(default)]
        header: CredentialHeader,
    },
}

impl CredentialConfig {
    pub const fn default_refresh() -> Duration {
        Duration::from_secs(300)
    }
    pub const fn default_timeout() -> Duration {
        Duration::from_secs(10)
    }
}

/// What a credential command prints on stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandOutput {
    /// The credential itself; surrounding whitespace is trimmed.
    #[default]
    Text,
    /// `{"token": "...", "expires_at": ...}`. `expires_at` is optional: an
    /// RFC 3339 timestamp or unix seconds (milliseconds when >= 10^11).
    Json,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelConfig {
    /// Identifier Claude Code sees and sends in `model`.
    pub id: String,
    pub backend: String,
    /// Model name sent upstream; defaults to `id`.
    #[serde(default)]
    pub upstream_model: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    /// Further identifiers routed to this model.
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingConfig {
    /// Model that receives requests naming an unknown model. Unset rejects them.
    #[serde(default)]
    pub default_model: Option<String>,
}
