//! The line format. Fields are added, never renamed; `v` changes when a
//! field changes meaning.

use serde::{Deserialize, Serialize};

use crate::activity::{ExchangeView, Outcome, Source};
use crate::anthropic::TokenUsage;

pub const SCHEMA: u32 = 1;

/// Routing, status, timings and usage of one exchange; never content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsRecord {
    pub v: u32,
    /// When the request arrived.
    pub ts: jiff::Timestamp,
    pub id: String,
    pub source: Source,
    #[serde(default)]
    pub requested_model: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// How `requested_model` found its route: `exact`, `alias` or `default`.
    #[serde(default)]
    pub matched: Option<String>,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub upstream_model: Option<String>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub attempts: Option<u32>,
    #[serde(default)]
    pub latency_ms: Option<u64>,
    #[serde(default)]
    pub ttfb_ms: Option<u64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub bytes: u64,
    pub outcome: Outcome,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
}

impl StatsRecord {
    /// `None` for an exchange that has not finished.
    pub fn from_view(view: &ExchangeView) -> Option<Self> {
        Some(Self {
            v: SCHEMA,
            ts: view.received_at,
            id: view.id.clone(),
            source: view.source,
            requested_model: view.requested_model.clone(),
            model: view.model.clone(),
            matched: view.matched.map(str::to_owned),
            backend: view.backend.clone(),
            upstream_model: view.upstream_model.clone(),
            stream: view.stream,
            status: view.status,
            attempts: view.attempts,
            latency_ms: view.latency_ms,
            ttfb_ms: view.ttfb_ms,
            duration_ms: view.duration_ms,
            bytes: view.bytes,
            outcome: view.outcome?,
            usage: view.usage,
        })
    }
}
