//! HTTP ingress: authentication, model discovery and the proxy handler.

mod annotate;
mod auth;
mod error;
mod handlers;
pub mod relay;
mod request_id;
mod routes;
mod state;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tokio::net::TcpListener;

use crate::config::Config;
use crate::observability::BodyLog;
use crate::routing::Registry;
use crate::upstream::{Backends, UpstreamClient};

pub use auth::ClientToken;
pub use error::RouterError;
pub use request_id::RequestId;
pub use state::AppState;

/// How often expired body-log entries are swept.
const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(600);

#[derive(Debug, thiserror::Error)]
pub enum ServerBuildError {
    #[error(transparent)]
    Backend(#[from] crate::upstream::BackendBuildError),
    #[error("cannot build HTTP client: {0}")]
    Client(#[from] reqwest::Error),
    #[error("cannot open body log directory {dir}: {source}")]
    BodyLog {
        dir: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// A configured but not yet listening router.
pub struct Server {
    app: Router,
    body_log: Option<Arc<BodyLog>>,
}

impl Server {
    pub fn new(config: &Config) -> Result<Self, ServerBuildError> {
        let body_log = match &config.logging.body_dir {
            Some(dir) => Some(Arc::new(
                BodyLog::open(dir, config.logging.body_retention).map_err(|source| {
                    ServerBuildError::BodyLog {
                        dir: dir.clone(),
                        source,
                    }
                })?,
            )),
            None => None,
        };
        let state = AppState {
            registry: Arc::new(Registry::from_config(config)),
            backends: Arc::new(Backends::from_config(config)?),
            upstream: Arc::new(UpstreamClient::from_config(&config.upstream)?),
            client_token: Arc::new(ClientToken::new(&config.server.token)),
            max_body_bytes: config.server.max_body_bytes,
            started_at: jiff::Timestamp::now(),
            body_log,
        };
        let body_log = state.body_log.clone();
        Ok(Self {
            app: routes::build(state),
            body_log,
        })
    }

    pub async fn bind(self, listen: SocketAddr) -> std::io::Result<BoundServer> {
        let listener = TcpListener::bind(listen).await?;
        Ok(BoundServer {
            listener,
            app: self.app,
            body_log: self.body_log,
        })
    }
}

/// A router bound to a socket, ready to serve.
pub struct BoundServer {
    listener: TcpListener,
    app: Router,
    body_log: Option<Arc<BodyLog>>,
}

impl BoundServer {
    pub fn local_addr(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("bound listener has an address")
    }

    /// Serves until `shutdown` resolves, then lets in-flight requests finish.
    /// Body-log pruning runs alongside and stops with the server.
    pub async fn serve(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        let pruner = self
            .body_log
            .filter(|log| log.retention().is_some())
            .map(|log| log.spawn_pruner(PRUNE_INTERVAL));
        let result = axum::serve(
            self.listener,
            self.app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await;
        if let Some(task) = pruner {
            task.abort();
        }
        result
    }
}
