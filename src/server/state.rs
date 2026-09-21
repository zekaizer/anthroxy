//! Per-process state. Everything a request needs lives in one immutable
//! [`Snapshot`]; a reload builds a new snapshot and swaps the pointer, so
//! in-flight requests keep the configuration they started with.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use serde::Serialize;

use super::{ClientToken, ServerBuildError};
use crate::activity::Activity;
use crate::config::Config;
use crate::ir::Model;
use crate::observability::BodyLog;
use crate::routing::{LiveCatalog, Registry};
use crate::stats::StatsLog;
use crate::upstream::UpstreamClient;

/// Configuration-derived state, built once per (re)load.
pub struct Snapshot {
    /// The configuration this snapshot was built from.
    pub config: Config,
    pub registry: Registry,
    pub live: Vec<LiveCatalog>,
    pub upstream: UpstreamClient,
    pub client_token: ClientToken,
    pub max_body_bytes: usize,
    /// Set when `logging.body_dir` is configured.
    pub body_log: Option<BodyLog>,
    /// Set unless `stats.enabled = false`.
    pub stats: Option<StatsLog>,
}

impl Snapshot {
    pub fn from_config(config: &Config) -> Result<Self, ServerBuildError> {
        // Routing and credentials first: a rejected configuration leaves no
        // directory behind.
        let registry = Registry::from_config(config)?;
        let upstream = UpstreamClient::from_config(&config.upstream, &config.backends)?;
        let body_log = match &config.logging.body_dir {
            Some(dir) => Some(BodyLog::open(dir, config.logging.body_retention).map_err(
                |source| ServerBuildError::BodyLog {
                    dir: dir.clone(),
                    source,
                },
            )?),
            None => None,
        };
        let stats = match config.stats.enabled {
            true => Some(
                StatsLog::open(&config.stats.dir, config.stats.retention).map_err(|source| {
                    ServerBuildError::Stats {
                        dir: config.stats.dir.clone(),
                        source,
                    }
                })?,
            ),
            false => None,
        };
        let live = registry
            .live_backends()
            .into_iter()
            .map(LiveCatalog::new)
            .collect();
        Ok(Self {
            config: config.clone(),
            registry,
            live,
            upstream,
            client_token: ClientToken::new(&config.server.token),
            max_body_bytes: config.server.max_body_bytes,
            body_log,
            stats,
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

/// Produces the configuration a reload applies, with the settings in it
/// that differ from what this process installed at startup and cannot change
/// without a restart (`server.listen` is detected by [`AppState::apply`]).
pub type Loader = Arc<dyn Fn() -> anyhow::Result<Loaded> + Send + Sync>;

pub struct Loaded {
    pub config: Config,
    pub restart_needed: Vec<String>,
}

/// What started a reload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReloadTrigger {
    Startup,
    Signal,
    Console,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ReloadOutcome {
    Applied {
        backends: usize,
        models: usize,
        /// Settings that changed in the file but need a restart.
        restart_needed: Vec<String>,
    },
    /// The previous configuration stayed in place.
    Rejected { error: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReloadEvent {
    pub at: jiff::Timestamp,
    pub trigger: ReloadTrigger,
    #[serde(flatten)]
    pub outcome: ReloadOutcome,
}

/// Reload events kept for the console.
const RELOAD_HISTORY: usize = 20;

/// Shared handle handed to every handler and to the reload task.
#[derive(Clone)]
pub struct AppState {
    current: Arc<RwLock<Arc<Snapshot>>>,
    /// Address the process bound at startup; a reload cannot change it.
    listen: SocketAddr,
    /// Reported as `created_at` of every model.
    pub started_at: jiff::Timestamp,
    /// The file a reload reads, and how.
    source: Arc<RwLock<Option<(PathBuf, Loader)>>>,
    /// Oldest first, at most [`RELOAD_HISTORY`].
    reloads: Arc<Mutex<VecDeque<ReloadEvent>>>,
    /// Exchanges in flight and recently finished; survives reloads.
    pub activity: Arc<Activity>,
    /// Model lists from the console's last probe, by backend name.
    probed: Arc<Mutex<HashMap<String, Vec<Model>>>>,
    /// Set once by a stop whose grace period ran out.
    cut: tokio::sync::watch::Sender<bool>,
    /// Held across a reload's load and apply, so two reloads (a SIGHUP and
    /// the console) cannot interleave and leave the older file current.
    reloading: Arc<Mutex<()>>,
    /// How long a request body may take to arrive whole.
    pub body_timeout: std::time::Duration,
}

impl AppState {
    pub fn new(snapshot: Snapshot, listen: SocketAddr) -> Self {
        let startup = ReloadEvent {
            at: jiff::Timestamp::now(),
            trigger: ReloadTrigger::Startup,
            outcome: ReloadOutcome::Applied {
                backends: snapshot.registry.backends().len(),
                models: snapshot.registry.routes().len(),
                restart_needed: Vec::new(),
            },
        };
        Self {
            current: Arc::new(RwLock::new(Arc::new(snapshot))),
            listen,
            started_at: jiff::Timestamp::now(),
            source: Arc::new(RwLock::new(None)),
            reloads: Arc::new(Mutex::new(VecDeque::from([startup]))),
            activity: Activity::new(),
            probed: Arc::new(Mutex::new(HashMap::new())),
            cut: tokio::sync::watch::Sender::new(false),
            reloading: Arc::new(Mutex::new(())),
            body_timeout: super::BODY_TIMEOUT,
        }
    }

    /// Ends every request still in flight, and any that arrives later: a
    /// handler that has not answered answers 503, a body still streaming
    /// fails (`server::shutdown`).
    pub fn cut_in_flight(&self) {
        self.cut.send_replace(true);
    }

    /// Reads `true` once [`cut_in_flight`](Self::cut_in_flight) ran; what a
    /// request drops unfinished after that was cut, not left by its client.
    pub fn cut_signal(&self) -> tokio::sync::watch::Receiver<bool> {
        self.cut.subscribe()
    }

    /// Resolves once [`cut_in_flight`](Self::cut_in_flight) ran.
    pub fn cut(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let mut cut = self.cut.subscribe();
        async move {
            let _ = cut.wait_for(|cut| *cut).await;
        }
    }

    /// Makes [`reload`](Self::reload) read `path` through `loader`.
    pub fn set_loader(&self, path: PathBuf, loader: Loader) {
        *self
            .source
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((path, loader));
    }

    /// The file a reload reads; `None` for a router built from a value.
    pub fn config_path(&self) -> Option<PathBuf> {
        self.source
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .map(|(path, _)| path.clone())
    }

    /// Loads and applies a configuration, logs the outcome and keeps it in the
    /// history. Blocks on file and credential setup; a router without a
    /// loader rejects the reload.
    pub fn reload(&self, trigger: ReloadTrigger) -> ReloadEvent {
        let _one_at_a_time = self
            .reloading
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let source = self
            .source
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let outcome = match source {
            None => ReloadOutcome::Rejected {
                error: "this router was started without a configuration file to reload from"
                    .to_owned(),
            },
            Some((path, load)) => self.reload_from(&path, &load),
        };
        let event = ReloadEvent {
            at: jiff::Timestamp::now(),
            trigger,
            outcome,
        };
        let mut reloads = self
            .reloads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if reloads.len() == RELOAD_HISTORY {
            reloads.pop_front();
        }
        reloads.push_back(event.clone());
        event
    }

    fn reload_from(&self, path: &Path, load: &Loader) -> ReloadOutcome {
        let applied = load().and_then(|loaded| {
            let report = self.apply(&loaded.config)?;
            crate::upstream::network::log(&loaded.config);
            Ok((report, loaded.restart_needed))
        });
        match applied {
            Ok((report, mut restart_needed)) => {
                if report.listen_changed {
                    restart_needed.insert(0, "server.listen".to_owned());
                }
                tracing::info!(
                    config = %path.display(),
                    backends = report.backends,
                    models = report.models,
                    "configuration reloaded"
                );
                for setting in &restart_needed {
                    tracing::warn!("{setting} changed in the file; restart the router to apply it");
                }
                ReloadOutcome::Applied {
                    backends: report.backends,
                    models: report.models,
                    restart_needed,
                }
            }
            Err(error) => {
                let error = format!("{error:#}");
                tracing::error!(%error, "reload failed; keeping the previous configuration");
                ReloadOutcome::Rejected { error }
            }
        }
    }

    /// Reload events, oldest first; the first is the startup configuration
    /// until the history is full.
    pub fn reloads(&self) -> Vec<ReloadEvent> {
        self.reloads
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    /// The address this process listens on.
    pub fn listen(&self) -> SocketAddr {
        self.listen
    }

    /// Model lists from the console's last probe, by backend name.
    pub fn probed(&self) -> HashMap<String, Vec<Model>> {
        self.probed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn set_probed(&self, listed: HashMap<String, Vec<Model>>) {
        *self
            .probed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = listed;
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
