//! Per-process state. Everything a request needs lives in one immutable
//! [`Snapshot`]; a reload builds a new snapshot and swaps the pointer, so
//! in-flight requests keep the configuration they started with.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use super::{ClientToken, ServerBuildError};
use crate::config::Config;
use crate::observability::BodyLog;
use crate::routing::Registry;
use crate::upstream::UpstreamClient;

/// Configuration-derived state, built once per (re)load.
pub struct Snapshot {
    pub registry: Registry,
    pub upstream: UpstreamClient,
    pub client_token: ClientToken,
    pub max_body_bytes: usize,
    /// Set when `logging.body_dir` is configured.
    pub body_log: Option<BodyLog>,
}

impl Snapshot {
    pub fn from_config(config: &Config) -> Result<Self, ServerBuildError> {
        let body_log = match &config.logging.body_dir {
            Some(dir) => Some(BodyLog::open(dir, config.logging.body_retention).map_err(
                |source| ServerBuildError::BodyLog {
                    dir: dir.clone(),
                    source,
                },
            )?),
            None => None,
        };
        Ok(Self {
            registry: Registry::from_config(config)?,
            upstream: UpstreamClient::from_config(&config.upstream)?,
            client_token: ClientToken::new(&config.server.token),
            max_body_bytes: config.server.max_body_bytes,
            body_log,
        })
    }
}

/// Outcome of a successful [`AppState::apply`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReloadReport {
    pub backends: usize,
    pub models: usize,
    /// The new file names a different `server.listen`; that needs a restart.
    pub listen_changed: bool,
}

/// Shared handle handed to every handler and to the reload task.
#[derive(Clone)]
pub struct AppState {
    current: Arc<RwLock<Arc<Snapshot>>>,
    /// Address the process bound at startup; a reload cannot change it.
    listen: SocketAddr,
    /// Reported as `created_at` of every model.
    pub started_at: jiff::Timestamp,
}

impl AppState {
    pub fn new(snapshot: Snapshot, listen: SocketAddr) -> Self {
        Self {
            current: Arc::new(RwLock::new(Arc::new(snapshot))),
            listen,
            started_at: jiff::Timestamp::now(),
        }
    }

    /// The configuration in force right now. Cheap: one pointer clone.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.current
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Builds a new snapshot from `config` and makes it current. On error the
    /// previous snapshot stays in place.
    pub fn apply(&self, config: &Config) -> Result<ReloadReport, ServerBuildError> {
        let snapshot = Snapshot::from_config(config)?;
        let report = ReloadReport {
            backends: snapshot.registry.backends().len(),
            models: snapshot.registry.routes().len(),
            listen_changed: config.server.listen != self.listen,
        };
        *self
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Arc::new(snapshot);
        Ok(report)
    }
}
