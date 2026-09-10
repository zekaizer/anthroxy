//! Per-model and per-day figures over a range of records.

use std::collections::BTreeMap;

use serde::Serialize;

use super::StatsRecord;
use crate::activity::Outcome;

/// The period a report covers, ending now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    Day,
    Week,
    Month,
    All,
}

impl Range {
    /// `1d`, `7d`, `30d` or `all`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "1d" => Some(Range::Day),
            "7d" => Some(Range::Week),
            "30d" => Some(Range::Month),
            "all" => Some(Range::All),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Range::Day => "1d",
            Range::Week => "7d",
            Range::Month => "30d",
            Range::All => "all",
        }
    }

    /// Start of the range; `None` for everything kept.
    pub fn since(self, now: jiff::Timestamp) -> Option<jiff::Timestamp> {
        let days = match self {
            Range::Day => 1,
            Range::Week => 7,
            Range::Month => 30,
            Range::All => return None,
        };
        now.checked_sub(jiff::SignedDuration::from_hours(24 * days))
            .ok()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub range: &'static str,
    pub since: Option<jiff::Timestamp>,
    pub total: Row,
    /// Most requests first.
    pub models: Vec<Row>,
    /// UTC dates, oldest first.
    pub days: Vec<Row>,
}

/// Figures for one group. Latency percentiles come from complete, successful
/// exchanges only; a fast 404 says nothing about a backend's speed.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Row {
    /// Model id, `(unrouted)` for requests no route served, or a date.
    pub key: String,
    pub backend: Option<String>,
    pub requests: u64,
    /// Status 400 or above, or a failure reported in the body.
    pub errors: u64,
    pub disconnects: u64,
    /// Exchanges that needed more than one attempt.
    pub retried: u64,
    pub ttfb_p50_ms: Option<u64>,
    pub ttfb_p95_ms: Option<u64>,
    pub duration_p50_ms: Option<u64>,
    pub duration_p95_ms: Option<u64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_creation_tokens: u64,
    /// Share of prompt tokens read from a cache.
    pub cache_hit_rate: Option<f64>,
    /// Output tokens over the time between first byte and end, streamed
    /// complete exchanges only.
    pub output_tokens_per_second: Option<f64>,
}

pub fn aggregate(records: &[StatsRecord], range: Range, since: Option<jiff::Timestamp>) -> Report {
    let mut total = Group::new("total");
    let mut models: BTreeMap<String, Group> = BTreeMap::new();
    let mut days: BTreeMap<String, Group> = BTreeMap::new();
    for record in records {
        total.add(record);
        let model = record.model.as_deref().unwrap_or("(unrouted)");
        models
            .entry(model.to_owned())
            .or_insert_with(|| Group::new(model))
            .add(record);
        let day = record
            .ts
            .to_zoned(jiff::tz::TimeZone::UTC)
            .date()
            .to_string();
        days.entry(day.clone())
            .or_insert_with(|| Group::new(&day))
            .add(record);
    }
    let mut models: Vec<Row> = models.into_values().map(Group::finish).collect();
    models.sort_by(|a, b| b.requests.cmp(&a.requests).then_with(|| a.key.cmp(&b.key)));
    Report {
        range: range.label(),
        since,
        total: total.finish(),
        models,
        days: days.into_values().map(Group::finish).collect(),
    }
}

/// A row being filled, with the samples its percentiles and rates need.
struct Group {
    row: Row,
    ttfb: Vec<u64>,
    duration: Vec<u64>,
    generating_ms: u64,
    generated: u64,
}

impl Group {
    fn new(key: &str) -> Self {
        Self {
            row: Row {
                key: key.to_owned(),
                ..Row::default()
            },
            ttfb: Vec::new(),
            duration: Vec::new(),
            generating_ms: 0,
            generated: 0,
        }
    }

    fn add(&mut self, record: &StatsRecord) {
        let row = &mut self.row;
        row.requests += 1;
        if row.backend.is_none() && row.key != "total" {
            row.backend.clone_from(&record.backend);
        }
        let failed =
            record.status.is_some_and(|status| status >= 400) || record.outcome == Outcome::Error;
        row.errors += u64::from(failed);
        row.disconnects += u64::from(record.outcome == Outcome::ClientDisconnected);
        row.retried += u64::from(record.attempts.is_some_and(|attempts| attempts > 1));
        if let Some(usage) = record.usage {
            row.input_tokens += usage.input;
            row.output_tokens += usage.output;
            row.cache_read_tokens += usage.cache_read;
            row.cache_creation_tokens += usage.cache_creation;
        }
        if failed || record.outcome != Outcome::Complete {
            return;
        }
        self.ttfb.extend(record.ttfb_ms);
        self.duration.extend(record.duration_ms);
        if let (true, Some(ttfb), Some(duration), Some(usage)) = (
            record.stream,
            record.ttfb_ms,
            record.duration_ms,
            record.usage,
        ) && duration > ttfb
            && usage.output > 0
        {
            self.generating_ms += duration - ttfb;
            self.generated += usage.output;
        }
    }

    fn finish(mut self) -> Row {
        let mut row = self.row;
        (row.ttfb_p50_ms, row.ttfb_p95_ms) = percentiles(&mut self.ttfb);
        (row.duration_p50_ms, row.duration_p95_ms) = percentiles(&mut self.duration);
        let prompt = row.input_tokens + row.cache_read_tokens + row.cache_creation_tokens;
        row.cache_hit_rate = (prompt > 0).then(|| row.cache_read_tokens as f64 / prompt as f64);
        row.output_tokens_per_second = (self.generating_ms > 0)
            .then(|| self.generated as f64 * 1000.0 / self.generating_ms as f64);
        row
    }
}

/// p50 and p95 by nearest rank.
fn percentiles(samples: &mut [u64]) -> (Option<u64>, Option<u64>) {
    if samples.is_empty() {
        return (None, None);
    }
    samples.sort_unstable();
    let rank = |p: f64| samples[((p * samples.len() as f64).ceil() as usize).max(1) - 1];
    (Some(rank(0.50)), Some(rank(0.95)))
}
