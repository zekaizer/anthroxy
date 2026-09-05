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
}

impl Server {
    pub fn new(config: &Config) -> Result<Self, ServerBuildError> {
        let body_log = match &config.logging.body_dir {
            Some(dir) => Some(Arc::new(BodyLog::open(dir).map_err(|source| {
                ServerBuildError::BodyLog {
                    dir: dir.clone(),
                    source,
                }
            })?)),
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
        Ok(Self {
            app: routes::build(state),
        })
    }

    pub async fn bind(self, listen: SocketAddr) -> std::io::Result<BoundServer> {
        let listener = TcpListener::bind(listen).await?;
        Ok(BoundServer {
            listener,
            app: self.app,
        })
    }
}

/// A router bound to a socket, ready to serve.
pub struct BoundServer {
    listener: TcpListener,
    app: Router,
}

impl BoundServer {
    pub fn local_addr(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("bound listener has an address")
    }

    /// Serves until `shutdown` resolves, then lets in-flight requests finish.
    pub async fn serve(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        axum::serve(
            self.listener,
            self.app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await
    }
}
