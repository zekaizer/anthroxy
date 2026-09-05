use std::sync::Arc;

use super::ClientToken;
use crate::routing::Registry;
use crate::upstream::{Backends, UpstreamClient};

/// Shared, immutable per-process state handed to every handler.
#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    pub backends: Arc<Backends>,
    pub upstream: Arc<UpstreamClient>,
    pub client_token: Arc<ClientToken>,
    pub max_body_bytes: usize,
    /// Reported as `created_at` of every model.
    pub started_at: jiff::Timestamp,
}
