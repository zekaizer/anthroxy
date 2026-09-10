use std::net::SocketAddr;
use std::path::PathBuf;

use anthroxy::config::{Config, process_env};
use anthroxy::server::{AppState, Server};
use serde_json::{Value, json};

pub const TOKEN: &str = "router-test-token";

/// A router serving `config` on an ephemeral loopback port.
pub struct TestRouter {
    pub addr: SocketAddr,
    pub http: reqwest::Client,
    pub reload: AppState,
    /// Where statistics go: a temporary directory unless the configuration
    /// has a `[stats]` table of its own.
    pub stats_dir: PathBuf,
    _state: tempfile::TempDir,
    _task: tokio::task::JoinHandle<()>,
}

impl TestRouter {
    pub async fn start(config_toml: &str) -> Self {
        let mut config = Config::parse(config_toml, process_env).expect("valid test config");
        let state = tempfile::tempdir().unwrap();
        if !config_toml.contains("[stats]") {
            config.stats.dir = state.path().join("stats");
        }
        let stats_dir = config.stats.dir.clone();
        let server = Server::bind(&config).await.expect("server binds");
        let addr = server.local_addr();
        let reload = server.state();
        let task = tokio::spawn(async move {
            server.serve(std::future::pending::<()>()).await.unwrap();
        });
        Self {
            addr,
            http: reqwest::Client::new(),
            reload,
            stats_dir,
            _state: state,
            _task: task,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    /// Authenticated POST of a JSON body to `path`.
    pub fn post(&self, path: &str, body: &Value) -> reqwest::RequestBuilder {
        self.http
            .post(self.url(path))
            .header("x-api-key", TOKEN)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .body(body.to_string())
    }

    pub fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http.get(self.url(path)).header("x-api-key", TOKEN)
    }
}

/// A `/v1/messages` body naming `model`, with fields the router must relay
/// untouched.
pub fn messages_body(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 16,
        "messages": [{"role": "user", "content": "hi"}],
        "metadata": {"user_id": "u1"},
        "future_field": {"nested": [1, 2, 3]}
    })
}

/// `data[].id` of a model list.
pub fn model_ids(list: &Value) -> Vec<String> {
    list["data"]
        .as_array()
        .expect("a model list")
        .iter()
        .map(|m| m["id"].as_str().unwrap().to_owned())
        .collect()
}

/// Configuration with one static-credential backend and two models.
pub fn config_with_backend(backend_url: &str, extra: &str) -> String {
    format!(
        r#"
[server]
listen = "127.0.0.1:0"
token = "{TOKEN}"

[backends.mock]
url = "{backend_url}"
credential = {{ kind = "static", value = "backend-secret-key" }}
anthropic_beta = ["oauth-2025-04-20"]

[[models]]
id = "fast"
backend = "mock"
upstream_model = "mock-fast-v1"
display_name = "Fast Mock"
aliases = ["claude-haiku-4-5"]

[[models]]
id = "smart"
backend = "mock"
{extra}
"#
    )
}
