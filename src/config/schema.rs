//! Serde types mirroring the configuration file. Field defaults live here;
//! cross-field rules live in `validate`.

use crate::openai::SystemPlacement;
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
    #[serde(default)]
    pub stats: StatsConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    /// Socket the router listens on. `0.0.0.0` so the Windows host can reach a
    /// WSL2 instance under both NAT and mirrored networking.
    #[serde(default = "ServerConfig::default_listen")]
    pub listen: SocketAddr,
    /// Static token the console presents, and `/v1` when [`V1Auth::Token`].
    pub token: String,
    /// How `/v1/*` authenticates. The console always uses `token`.
    #[serde(default)]
    pub v1_auth: V1Auth,
    /// Largest accepted request body.
    #[serde(default = "ServerConfig::default_max_body", with = "byte_size")]
    pub max_body_bytes: usize,
}

/// `/v1` client authentication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum V1Auth {
    /// `x-api-key` or `Authorization: Bearer` must equal `server.token`.
    #[default]
    Token,
    /// No `/v1` authentication. `listen` must be loopback.
    None,
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

/// Persistent request statistics (ADR-0011).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct StatsConfig {
    /// Append one line per `/v1/messages` exchange.
    pub enabled: bool,
    pub dir: PathBuf,
    /// Files whose day ended longer ago than this are deleted; `0` keeps them.
    #[serde(with = "humantime_serde")]
    pub retention: Duration,
}

impl StatsConfig {
    /// `$XDG_STATE_HOME/anthroxy/stats`, or the platform's local data
    /// directory where there is no state directory.
    pub fn default_dir() -> PathBuf {
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("anthroxy")
            .join("stats")
    }
}

impl Default for StatsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            dir: Self::default_dir(),
            retention: Duration::from_secs(90 * 24 * 3600),
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
    /// Non-stream: after connect, until the whole response body is finished.
    #[serde(with = "humantime_serde")]
    pub non_stream_timeout: Duration,
    /// Stream: until the first body byte.
    #[serde(with = "humantime_serde")]
    pub stream_first_byte_timeout: Duration,
    /// Stream: silence between subsequent body chunks; resets on each chunk.
    #[serde(with = "humantime_serde")]
    pub stream_idle_timeout: Duration,
    /// Additional attempts after a connection failure.
    pub retries: u32,
    /// Delay before the first retry; doubles on each further attempt, up to
    /// [`crate::upstream::MAX_BACKOFF`].
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
            non_stream_timeout: Duration::from_secs(900),
            stream_first_byte_timeout: Duration::from_secs(300),
            stream_idle_timeout: Duration::from_secs(60),
            retries: 2,
            retry_backoff: Duration::from_millis(200),
            retry_on_status: Vec::new(),
            ca_certificate: None,
        }
    }
}

/// Which API a backend speaks. `Anthropic` bodies are relayed as bytes
/// (ADR-0003); `OpenAi` bodies are translated (ADR-0010).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    #[default]
    Anthropic,
    OpenAi,
    /// Origin whose model list is fetched live and whose requests are relayed
    /// byte-for-byte, including the client's `Authorization`.
    Passthrough,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackendConfig {
    #[serde(default)]
    pub kind: BackendKind,
    /// Origin of the backend, e.g. `http://127.0.0.1:8000`. For `anthropic`
    /// the request path is appended unchanged; for `openai` it is
    /// `/v1/chat/completions`.
    pub url: String,
    /// Where the probe asks this backend for its model list, appended to
    /// `url`; for a gateway that serves the list off `/v1/models`.
    #[serde(default = "BackendConfig::default_models_path")]
    pub models_path: String,
    /// Fetch `GET {url}{models_path}` and expose those ids as Anthropic model
    /// identity. Anthropic and OpenAI list shapes both work. Implied for
    /// `kind = "passthrough"`. Configured `[[models]]` still win on the same id.
    #[serde(default)]
    pub live_models: bool,
    #[serde(default)]
    pub credential: CredentialConfig,
    /// Headers set on every upstream request, overriding the client's value.
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// Beta flags merged into the client's `anthropic-beta` header.
    #[serde(default)]
    pub anthropic_beta: Vec<String>,
    /// Client headers this backend never sees: a header name, a `*` pattern
    /// (`x-stainless-*`), or `@claude-code` for what Claude Code adds to name
    /// itself. For a gateway that refuses headers it does not know.
    #[serde(default)]
    pub drop_headers: Vec<String>,
    /// Request body fields removed before forwarding, as dot-separated paths
    /// (`context_management`, `metadata.user_id`), for a backend that rejects
    /// parameters it does not know.
    #[serde(default)]
    pub drop_fields: Vec<String>,
    /// `kind = "openai"` only: where a `system` message that is not the
    /// first message goes, for a chat template that refuses one anywhere
    /// else (Qwen 3.5+). `keep` (default) sends it where it is, `merge`
    /// appends its text to the leading system message, `user` sends it as a
    /// user message.
    #[serde(default)]
    pub mid_conversation_system: SystemPlacement,
    /// Proxy this backend is reached through (ADR-0012); unset connects
    /// directly.
    #[serde(default)]
    pub proxy: Option<String>,
}

impl BackendConfig {
    pub fn default_models_path() -> String {
        "/v1/models".to_owned()
    }
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
    /// Overrides the backend's `mid_conversation_system` for this model.
    #[serde(default)]
    pub mid_conversation_system: Option<SystemPlacement>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingConfig {
    /// Model that receives requests naming an unknown model. Unset rejects them.
    #[serde(default)]
    pub default_model: Option<String>,
}
