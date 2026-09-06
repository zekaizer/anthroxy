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

use axum::Router;
use tokio::net::TcpListener;

use crate::config::Config;

pub use auth::ClientToken;
pub use error::RouterError;
pub use request_id::RequestId;
pub use state::{AppState, Snapshot};

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

/// Outcome of a successful [`ReloadHandle::apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadReport {
    pub backends: usize,
    pub models: usize,
    /// The new file names a different `server.listen`; that needs a restart.
    pub listen_changed: bool,
}

/// Swaps the router's configuration while it keeps serving.
#[derive(Clone)]
pub struct ReloadHandle {
    state: AppState,
}

impl ReloadHandle {
    /// Builds a new snapshot from `config` and makes it current. On error the
    /// previous snapshot stays in place.
    pub fn apply(&self, config: &Config) -> Result<ReloadReport, ServerBuildError> {
        let snapshot = Snapshot::from_config(config)?;
        let report = ReloadReport {
            backends: snapshot.backends.len(),
            models: snapshot.registry.len(),
            listen_changed: config.server.listen != self.state.listen(),
        };
        self.state.replace(snapshot);
        Ok(report)
    }
}

/// A configured but not yet listening router.
pub struct Server {
    app: Router,
    state: AppState,
}

impl Server {
    pub fn new(config: &Config) -> Result<Self, ServerBuildError> {
        let state = AppState::new(Snapshot::from_config(config)?, config.server.listen);
        Ok(Self {
            app: routes::build(state.clone()),
            state,
        })
    }

    pub fn reload_handle(&self) -> ReloadHandle {
        ReloadHandle {
            state: self.state.clone(),
        }
    }

    pub async fn bind(self, listen: SocketAddr) -> std::io::Result<BoundServer> {
        let listener = TcpListener::bind(listen).await?;
        Ok(BoundServer {
            listener,
            app: self.app,
            state: self.state,
        })
    }
}

/// A router bound to a socket, ready to serve.
pub struct BoundServer {
    listener: TcpListener,
    app: Router,
    state: AppState,
}

impl BoundServer {
    pub fn reload_handle(&self) -> ReloadHandle {
        ReloadHandle {
            state: self.state.clone(),
        }
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("bound listener has an address")
    }

    /// Serves until `shutdown` resolves, then lets in-flight requests finish.
    /// Body-log pruning runs alongside on whichever snapshot is current and
    /// stops with the server.
    pub async fn serve(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
    ) -> std::io::Result<()> {
        let pruner = tokio::spawn(prune_loop(self.state.clone()));
        let result = axum::serve(
            self.listener,
            self.app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(shutdown)
        .await;
        pruner.abort();
        result
    }
}

async fn prune_loop(state: AppState) {
    loop {
        if let Some(log) = &state.snapshot().body_log {
            let removed = log.prune(jiff::Timestamp::now());
            if removed > 0 {
                tracing::info!(removed, dir = %log.root().display(), "pruned body log entries");
            }
        }
        tokio::time::sleep(PRUNE_INTERVAL).await;
    }
}
