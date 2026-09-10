//! HTTP ingress: authentication, model discovery and the proxy handler.

mod annotate;
mod auth;
mod buffered;
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

pub use annotate::backend_prefix;
pub use auth::ClientToken;
pub use error::RouterError;
pub use request_id::RequestId;
pub use state::{AppState, ReloadReport, Snapshot};

/// How often expired body-log entries are swept.
const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(600);

#[derive(Debug, thiserror::Error)]
pub enum ServerBuildError {
    #[error(transparent)]
    Backend(#[from] crate::upstream::BackendBuildError),
    #[error(transparent)]
    Client(#[from] crate::upstream::ClientBuildError),
    #[error("cannot open body log directory {dir}: {source}")]
    BodyLog {
        dir: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot listen on {addr}: {source}")]
    Listen {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
}

/// A router bound to `server.listen`, ready to serve.
pub struct Server {
    listener: TcpListener,
    app: Router,
    state: AppState,
}

impl Server {
    /// Builds the first snapshot from `config` and binds `server.listen`.
    pub async fn bind(config: &Config) -> Result<Self, ServerBuildError> {
        let listen = config.server.listen;
        let state = AppState::new(Snapshot::from_config(config)?, listen);
        let listener =
            TcpListener::bind(listen)
                .await
                .map_err(|source| ServerBuildError::Listen {
                    addr: listen,
                    source,
                })?;
        Ok(Self {
            listener,
            app: routes::build(state.clone()),
            state,
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("bound listener has an address")
    }

    /// Handle for reloading and for reading the current snapshot.
    pub fn state(&self) -> AppState {
        self.state.clone()
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

/// The sweep walks the directory synchronously, so it runs on the blocking
/// pool rather than a worker thread.
async fn prune_loop(state: AppState) {
    loop {
        if let Some(log) = state.snapshot().body_log.clone() {
            let _ = tokio::task::spawn_blocking(move || {
                let removed = log.prune(jiff::Timestamp::now());
                if removed > 0 {
                    tracing::info!(removed, dir = %log.root().display(), "pruned body log entries");
                }
            })
            .await;
        }
        tokio::time::sleep(PRUNE_INTERVAL).await;
    }
}
