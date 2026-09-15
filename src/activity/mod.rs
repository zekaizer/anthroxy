//! What the router is doing and has recently done, kept in memory for the
//! console (ADR-0011).

mod exchange;
pub mod hints;
mod names;

#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use crate::anthropic::TokenUsage;
use crate::config::BackendKind;

pub use exchange::{Exchange, Tracked};
pub use hints::Hint;
pub use names::NameCount;

/// Finished exchanges kept.
pub const RECENT: usize = 200;
/// An upstream error body is kept up to this many bytes.
pub const ERROR_BODY_BYTES: usize = 16 * 1024;

/// Who sent a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Client,
    /// A test request the console sent.
    Console,
}

/// How an exchange ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Complete,
    /// The router, the backend or the stream reported a failure.
    Error,
    ClientDisconnected,
}

/// One request as far as the router has got with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExchangeView {
    pub id: String,
    pub received_at: jiff::Timestamp,
    pub source: Source,
    pub peer: Option<String>,
    pub method: String,
    pub path: String,
    /// What the client named, cut.
    pub requested_model: Option<String>,
    pub model: Option<String>,
    /// `exact`, `alias` or `default`.
    pub matched: Option<&'static str>,
    pub backend: Option<String>,
    pub kind: Option<BackendKind>,
    pub upstream_model: Option<String>,
    pub stream: bool,
    pub status: Option<u16>,
    pub attempts: Option<u32>,
    /// A rejected credential was re-acquired and the request sent again.
    pub credential_refreshed: bool,
    /// From arrival to the backend's response headers.
    pub latency_ms: Option<u64>,
    /// From arrival to the first body byte handed to the client.
    pub ttfb_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    pub bytes: u64,
    pub usage: Option<TokenUsage>,
    /// `None` while in flight.
    pub outcome: Option<Outcome>,
    pub error: Option<String>,
    /// The upstream error body, up to [`ERROR_BODY_BYTES`].
    pub error_body: Option<String>,
    pub hints: Vec<Hint>,
    /// Body log entry name, when recorded.
    pub recording: Option<String>,
}

/// Process-wide; survives reloads.
#[derive(Default)]
pub struct Activity {
    in_flight: Mutex<Vec<Arc<Mutex<ExchangeView>>>>,
    /// Newest last.
    recent: Mutex<VecDeque<Arc<ExchangeView>>>,
    names: Mutex<names::NameTally>,
}

impl Activity {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Starts tracking a request that just arrived; `cut` is the stop's cut
    /// signal.
    pub fn begin(
        self: &Arc<Self>,
        id: &str,
        source: Source,
        peer: Option<String>,
        method: &str,
        path: &str,
        cut: tokio::sync::watch::Receiver<bool>,
    ) -> Exchange {
        Exchange::new(
            self.clone(),
            ExchangeView {
                id: id.to_owned(),
                received_at: jiff::Timestamp::now(),
                source,
                peer,
                method: method.to_owned(),
                path: path.to_owned(),
                requested_model: None,
                model: None,
                matched: None,
                backend: None,
                kind: None,
                upstream_model: None,
                stream: false,
                status: None,
                attempts: None,
                credential_refreshed: false,
                latency_ms: None,
                ttfb_ms: None,
                duration_ms: None,
                bytes: 0,
                usage: None,
                outcome: None,
                error: None,
                error_body: None,
                hints: Vec::new(),
                recording: None,
            },
            cut,
        )
    }

    /// Exchanges in flight, oldest first.
    pub fn in_flight(&self) -> Vec<ExchangeView> {
        lock(&self.in_flight)
            .iter()
            .map(|view| lock(view).clone())
            .collect()
    }

    /// Finished exchanges, newest first.
    pub fn recent(&self) -> Vec<Arc<ExchangeView>> {
        lock(&self.recent).iter().rev().cloned().collect()
    }

    /// An exchange in flight or recent; the newest when an id repeats.
    pub fn find(&self, id: &str) -> Option<ExchangeView> {
        if let Some(view) = lock(&self.in_flight)
            .iter()
            .map(|view| lock(view))
            .find(|view| view.id == id)
        {
            return Some(view.clone());
        }
        lock(&self.recent)
            .iter()
            .rev()
            .find(|view| view.id == id)
            .map(|view| (**view).clone())
    }

    /// Model names that matched no route, most recently seen first.
    pub fn names(&self) -> Vec<NameCount> {
        lock(&self.names).list()
    }

    fn names_mut(&self) -> std::sync::MutexGuard<'_, names::NameTally> {
        lock(&self.names)
    }

    fn register(&self, view: &Arc<Mutex<ExchangeView>>) {
        lock(&self.in_flight).push(view.clone());
    }

    fn complete(&self, view: &Arc<Mutex<ExchangeView>>) -> Arc<ExchangeView> {
        let done = Arc::new(lock(view).clone());
        lock(&self.in_flight).retain(|v| !Arc::ptr_eq(v, view));
        let mut recent = lock(&self.recent);
        if recent.len() == RECENT {
            recent.pop_front();
        }
        recent.push_back(done.clone());
        done
    }
}

/// A panic elsewhere must not take the console down with it.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
