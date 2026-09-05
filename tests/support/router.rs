use std::net::SocketAddr;

use claude_router::config::Config;
use claude_router::server::Server;

pub const TOKEN: &str = "router-test-token";

/// A router serving `config` on an ephemeral loopback port.
pub struct TestRouter {
    pub addr: SocketAddr,
    pub http: reqwest::Client,
    _task: tokio::task::JoinHandle<()>,
}

impl TestRouter {
    pub async fn start(config_toml: &str) -> Self {
        let config =
            Config::parse(config_toml, |name| std::env::var(name).ok()).expect("valid test config");
        let server = Server::new(&config).expect("server builds");
        let bound = server
            .bind("127.0.0.1:0".parse().unwrap())
            .await
            .expect("bind");
        let addr = bound.local_addr();
        let task = tokio::spawn(async move {
            bound.serve(std::future::pending::<()>()).await.unwrap();
        });
        Self {
            addr,
            http: reqwest::Client::new(),
            _task: task,
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }

    /// Authenticated POST of a JSON body to `path`.
    pub fn post(&self, path: &str, body: &serde_json::Value) -> reqwest::RequestBuilder {
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
