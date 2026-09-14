//! HTTP ingress: authentication, model discovery and the proxy handler.

mod annotate;
mod auth;
mod buffered;
mod error;
mod handlers;
pub mod relay;
mod request_id;
mod routes;
mod shutdown;
mod state;

use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;

use crate::config::Config;

pub use annotate::backend_prefix;
pub use auth::ClientToken;
pub use error::RouterError;
pub use request_id::RequestId;
pub use state::{
    AppState, Loaded, Loader, ReloadEvent, ReloadOutcome, ReloadReport, ReloadTrigger, Snapshot,
};

/// How often expired body-log entries are swept.
const PRUNE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(600);

/// How long a stop waits, once past its grace period, for connections to
/// close and again for records to be written.
const AFTER_CUT: std::time::Duration = std::time::Duration::from_secs(2);

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
    #[error("cannot open statistics directory {dir}: {source}")]
    Stats {
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

    /// Serves until `shutdown` resolves, then gives in-flight requests up to
    /// `grace` to finish. Past it they are cut ([`AppState::cut_in_flight`]).
    /// Returns once connections closed and the records of every exchange
    /// were written, each wait bounded by `AFTER_CUT`. Body-log pruning runs
    /// alongside on whichever snapshot is current and stops with the server.
    pub async fn serve(
        self,
        shutdown: impl std::future::Future<Output = ()> + Send + 'static,
        grace: std::time::Duration,
    ) -> std::io::Result<()> {
        let pruner = tokio::spawn(prune_loop(self.state.clone()));
        let (stopping, stopped) = tokio::sync::oneshot::channel::<()>();
        let drained = axum::serve(
            self.listener,
            self.app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            shutdown.await;
            let _ = stopping.send(());
        })
        .into_future();
        let deadline = async move {
            // A sender dropped unsent is a server that ended on its own.
            if stopped.await.is_err() {
                std::future::pending::<()>().await;
            }
            tokio::time::sleep(grace).await;
        };
        tokio::pin!(drained);
        let result = tokio::select! {
            result = &mut drained => result,
            () = deadline => {
                tracing::warn!(
                    in_flight = self.state.activity.in_flight().len(),
                    grace_ms = grace.as_millis() as u64,
                    "cutting requests still in flight"
                );
                self.state.cut_in_flight();
                tokio::time::timeout(AFTER_CUT, drained)
                    .await
                    .unwrap_or_else(|_| {
                        tracing::warn!("connections still open after the cut");
                        Ok(())
                    })
            }
        };
        pruner.abort();
        if tokio::time::timeout(AFTER_CUT, crate::private_fs::settled())
            .await
            .is_err()
        {
            tracing::warn!("stopping before every record was written");
        }
        result
    }
}

/// The sweeps walk directories synchronously, so they run on the blocking
/// pool rather than a worker thread.
async fn prune_loop(state: AppState) {
    loop {
        let snapshot = state.snapshot();
        let (body_log, stats) = (snapshot.body_log.clone(), snapshot.stats.clone());
        drop(snapshot);
        let _ = tokio::task::spawn_blocking(move || {
            let now = jiff::Timestamp::now();
            if let Some(log) = body_log {
                let removed = log.prune(now);
                if removed > 0 {
                    tracing::info!(removed, dir = %log.root().display(), "pruned body log entries");
                }
            }
            if let Some(stats) = stats {
                let removed = stats.prune(now);
                if removed > 0 {
                    tracing::info!(removed, dir = %stats.dir().display(), "pruned statistics files");
                }
            }
        })
        .await;
        tokio::time::sleep(PRUNE_INTERVAL).await;
    }
}
