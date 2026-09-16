//! Writes each proxied exchange to `<body_dir>/<time>-<request id>/`:
//! `request.json` (as sent upstream), `response.<json|sse|bin>` (as received)
//! and `meta.json` (routing, status, timing, headers with credentials
//! redacted). Enabled by `logging.body_dir`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use serde::Serialize;
use tokio::sync::watch;

use crate::private_fs::{PendingWrite, create_dir_private, write_private};
use crate::server::relay::RelayOutcome;
use crate::upstream::{DroppedHeader, SentHeader};

#[derive(Debug, Clone)]
pub struct BodyLog {
    root: PathBuf,
    /// `None` keeps entries forever.
    retention: Option<Duration>,
}

/// What the router knew about a request when it forwarded it.
#[derive(Debug, Clone, Serialize)]
pub struct RequestRecord {
    pub request_id: String,
    pub received_at: String,
    pub method: String,
    pub path: String,
    pub requested_model: String,
    pub model: String,
    pub upstream_model: String,
    pub backend: String,
    pub stream: bool,
    /// Every header the backend received, each saying where it came from.
    pub request_headers: Vec<SentHeader>,
    /// What the client sent that the backend never saw, each saying why.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dropped_headers: Vec<DroppedHeader>,
    /// Claude Code's `x-claude-code-session-id`, as the client sent it: the
    /// recording still groups by session when the backend never sees it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// `messages` in the client's body.
    pub messages: usize,
    /// The client's last prompt, as [`crate::anthropic::RequestSummary`] cuts it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// What the request sends instead of a prompt, as
    /// [`crate::anthropic::RequestSummary`] words it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct Meta {
    #[serde(flatten)]
    request: RequestRecord,
    /// Present once response headers arrived.
    #[serde(flatten)]
    response: Option<ResponseMeta>,
    /// Present once the body ended.
    #[serde(flatten)]
    end: Option<EndMeta>,
}

#[derive(Debug, Clone, Serialize)]
struct ResponseMeta {
    status: u16,
    attempts: u32,
    latency_ms: u64,
    response_headers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
struct EndMeta {
    outcome: String,
    response_bytes: usize,
    duration_ms: u64,
}

/// Accumulates one exchange and writes it out when the response ends. A
/// recorder dropped before [`Recorder::finish`] records a client that left, or
/// a stop once one cut what was in flight: a handler awaiting a buffered body
/// is dropped when its client goes away.
pub struct Recorder {
    dir: PathBuf,
    meta: Meta,
    started: Instant,
    response: Vec<u8>,
    response_file: &'static str,
    /// The initial write; the final write is ordered after it so `meta.json`
    /// always ends in its complete form. Taken by the final write.
    pending: Option<tokio::task::JoinHandle<()>>,
    /// Tells a stop's cut from a client that left when dropped unfinished.
    cut: watch::Receiver<bool>,
}

impl BodyLog {
    /// `retention` of zero disables pruning.
    pub fn open(root: &Path, retention: Duration) -> std::io::Result<Self> {
        create_dir_private(root)?;
        Ok(Self {
            root: root.to_path_buf(),
            retention: (!retention.is_zero()).then_some(retention),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Deletes entries whose stamp is older than `now - retention`. Returns
    /// how many were removed. Names that do not carry a stamp are left alone.
    pub fn prune(&self, now: jiff::Timestamp) -> usize {
        let Some(retention) = self.retention else {
            return 0;
        };
        let Ok(cutoff) =
            now.checked_sub(jiff::SignedDuration::try_from(retention).unwrap_or_default())
        else {
            return 0;
        };
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(stamp) = name.to_str().and_then(entry_stamp) else {
                continue;
            };
            if stamp < cutoff && entry.file_type().is_ok_and(|t| t.is_dir()) {
                match std::fs::remove_dir_all(entry.path()) {
                    Ok(()) => removed += 1,
                    Err(error) => {
                        tracing::warn!(path = %entry.path().display(), %error, "cannot prune body log entry");
                    }
                }
            }
        }
        removed
    }

    /// Starts a record and writes `request.json` plus a first `meta.json`.
    pub fn begin(
        &self,
        record: RequestRecord,
        body: &Bytes,
        started: Instant,
        cut: watch::Receiver<bool>,
    ) -> Recorder {
        let dir = self.root.join(format!(
            "{}-{}",
            dir_stamp(jiff::Timestamp::now()),
            record.request_id
        ));
        let meta = Meta {
            request: record,
            response: None,
            end: None,
        };
        let pending = write_files(
            dir.clone(),
            vec![
                ("request.json", body.clone()),
                ("meta.json", to_pretty_json(&meta)),
            ],
            None,
        );
        Recorder {
            dir,
            meta,
            started,
            response: Vec::new(),
            response_file: "response.bin",
            pending: Some(pending),
            cut,
        }
    }
}

/// The files an entry may hold.
pub const ENTRY_FILES: [&str; 5] = [
    "meta.json",
    "request.json",
    "response.json",
    "response.sse",
    "response.bin",
];

/// One recorded exchange as the console lists it, read from its directory
/// and `meta.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntrySummary {
    pub name: String,
    pub at: jiff::Timestamp,
    pub request_id: String,
    pub bytes: u64,
    pub files: Vec<String>,
    pub path: Option<String>,
    pub model: Option<String>,
    pub backend: Option<String>,
    pub status: Option<u16>,
    pub outcome: Option<String>,
    /// The client asked for a stream; absent in entries recorded before the
    /// field existed.
    pub stream: Option<bool>,
    pub messages: Option<u64>,
    pub prompt: Option<String>,
    /// Absent for a request that carries its prompt, and in entries recorded
    /// before the field existed.
    pub step: Option<String>,
    /// Claude Code's `x-claude-code-session-id` request header.
    pub session: Option<String>,
    /// `meta.json` is there but does not read as JSON, so every field above it
    /// filled is empty for want of a value, not because the router had none.
    pub unreadable: bool,
}

/// The newest entries and how many there are in all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Listing {
    pub total: usize,
    pub entries: Vec<EntrySummary>,
}

impl BodyLog {
    /// Entries, newest first, at most `limit`.
    pub fn list(&self, limit: usize) -> Listing {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return Listing::default();
        };
        let mut names: Vec<(String, jiff::Timestamp)> = entries
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_owned();
                let at = entry_stamp(&name)?;
                Some((name, at))
            })
            .collect();
        names.sort_by(|a, b| b.0.cmp(&a.0));
        Listing {
            total: names.len(),
            entries: names
                .into_iter()
                .take(limit)
                .map(|(name, at)| self.summary(name, at))
                .collect(),
        }
    }

    fn summary(&self, name: String, at: jiff::Timestamp) -> EntrySummary {
        let dir = self.root.join(&name);
        let mut files = Vec::new();
        let mut bytes = 0;
        for file in ENTRY_FILES {
            if let Ok(metadata) = std::fs::metadata(dir.join(file)) {
                files.push(file.to_owned());
                bytes += metadata.len();
            }
        }
        let raw = std::fs::read(dir.join("meta.json")).ok();
        let meta: serde_json::Value = raw
            .as_ref()
            .and_then(|raw| serde_json::from_slice(raw).ok())
            .unwrap_or_default();
        let text = |key: &str| meta.get(key).and_then(|v| v.as_str()).map(str::to_owned);
        EntrySummary {
            request_id: name.get(21..).unwrap_or_default().to_owned(),
            at,
            bytes,
            files,
            path: text("path"),
            model: text("model"),
            backend: text("backend"),
            status: meta
                .get("status")
                .and_then(|v| v.as_u64())
                .and_then(|v| u16::try_from(v).ok()),
            outcome: text("outcome"),
            stream: meta.get("stream").and_then(serde_json::Value::as_bool),
            messages: meta.get("messages").and_then(|v| v.as_u64()),
            prompt: text("prompt"),
            step: text("step"),
            session: text("session"),
            unreadable: raw.is_some() && !meta.is_object(),
            name,
        }
    }

    /// `file` of entry `name`, when both name something the log wrote.
    pub fn file(&self, name: &str, file: &str) -> Option<PathBuf> {
        if !is_entry_name(name) || !ENTRY_FILES.contains(&file) {
            return None;
        }
        let path = self.root.join(name).join(file);
        path.is_file().then_some(path)
    }

    /// Deletes entry `name`; `false` when there is no such entry.
    pub fn remove(&self, name: &str) -> std::io::Result<bool> {
        let dir = self.root.join(name);
        if !is_entry_name(name) || !dir.is_dir() {
            return Ok(false);
        }
        std::fs::remove_dir_all(dir).map(|()| true)
    }

    /// Deletes every entry; returns how many went. Other names are left alone.
    pub fn remove_all(&self) -> usize {
        self.list(usize::MAX)
            .entries
            .iter()
            .filter(|entry| self.remove(&entry.name).unwrap_or(false))
            .count()
    }
}

/// A directory name the log wrote: a stamp and a request id, nothing that
/// leaves the root.
fn is_entry_name(name: &str) -> bool {
    entry_stamp(name).is_some() && !name.contains(['/', '\\']) && !name.contains("..")
}

/// `YYYYMMDDTHHMMSS.mmmZ`, sortable and file-name safe.
fn dir_stamp(now: jiff::Timestamp) -> String {
    format!(
        "{}.{:03}Z",
        now.strftime("%Y%m%dT%H%M%S"),
        now.subsec_millisecond()
    )
}

/// Inverse of [`dir_stamp`] for an entry directory name (`<stamp>-<id>`).
fn entry_stamp(name: &str) -> Option<jiff::Timestamp> {
    // 20260906T023829.457Z
    let stamp = name.get(..20)?;
    if !stamp.ends_with('Z') || name.as_bytes().get(20) != Some(&b'-') {
        return None;
    }
    let parsed = jiff::civil::DateTime::strptime("%Y%m%dT%H%M%S.%3f", &stamp[..19]).ok()?;
    parsed
        .to_zoned(jiff::tz::TimeZone::UTC)
        .ok()
        .map(|z| z.timestamp())
}

fn to_pretty_json(value: &impl Serialize) -> Bytes {
    Bytes::from(serde_json::to_vec_pretty(value).expect("records serialize"))
}

/// Writes off the request path, after `after` when given; failures are
/// logged, never propagated. Each file is written to a temporary name and
/// renamed, so readers never observe a partial file. Called from inside the
/// runtime: a handler, or the relay stream's poll and drop.
fn write_files(
    dir: PathBuf,
    files: Vec<(&'static str, Bytes)>,
    after: Option<tokio::task::JoinHandle<()>>,
) -> tokio::task::JoinHandle<()> {
    let span = tracing::Span::current();
    let pending = PendingWrite::begin();
    let write = move || {
        let _pending = pending;
        let _guard = span.enter();
        if let Err(error) = create_dir_private(&dir) {
            tracing::error!(dir = %dir.display(), %error, "cannot create body log directory");
            return;
        }
        for (name, bytes) in files {
            let path = dir.join(name);
            let temp = dir.join(format!("{name}.tmp"));
            if let Err(error) =
                write_private(&temp, &bytes).and_then(|()| std::fs::rename(&temp, &path))
            {
                tracing::error!(path = %path.display(), %error, "cannot write body log file");
            }
        }
    };
    tokio::spawn(async move {
        if let Some(previous) = after {
            let _ = previous.await;
        }
        let _ = tokio::task::spawn_blocking(write).await;
    })
}

/// File name for the response body, by content type.
fn response_file(headers: &HeaderMap) -> &'static str {
    let content_type = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if crate::upstream::is_event_stream(headers) {
        "response.sse"
    } else if content_type.starts_with("application/json") {
        "response.json"
    } else {
        "response.bin"
    }
}

impl Recorder {
    /// The entry's directory name under the body log root.
    pub fn entry(&self) -> String {
        self.dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Called once response headers arrive.
    pub fn response_started(
        &mut self,
        status: StatusCode,
        headers: &HeaderMap,
        attempts: u32,
        latency_ms: u64,
    ) {
        self.meta.response = Some(ResponseMeta {
            status: status.as_u16(),
            attempts,
            latency_ms,
            response_headers: headers_for_record(headers),
        });
        self.response_file = response_file(headers);
    }

    /// Records a fully buffered body and finishes.
    pub fn finish_with_body(mut self, body: &[u8]) {
        self.response.extend_from_slice(body);
        self.finish(&RelayOutcome::Complete);
    }

    pub fn chunk(&mut self, chunk: &Bytes) {
        self.response.extend_from_slice(chunk);
    }

    /// Writes the response body and the final `meta.json`.
    pub fn finish(mut self, outcome: &RelayOutcome) {
        self.record(outcome);
    }

    fn record(&mut self, outcome: &RelayOutcome) {
        self.meta.end = Some(EndMeta {
            outcome: match outcome {
                RelayOutcome::Complete => "complete".to_owned(),
                RelayOutcome::UpstreamError(error) => format!("upstream_error: {error}"),
                RelayOutcome::ClientDisconnected => "client_disconnected".to_owned(),
                RelayOutcome::Stopped => "stopped".to_owned(),
            },
            response_bytes: self.response.len(),
            duration_ms: self.started.elapsed().as_millis() as u64,
        });
        write_files(
            self.dir.clone(),
            vec![
                (
                    self.response_file,
                    Bytes::from(std::mem::take(&mut self.response)),
                ),
                ("meta.json", to_pretty_json(&self.meta)),
            ],
            self.pending.take(),
        );
        tracing::debug!(dir = %self.dir.display(), "exchange recorded");
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        if self.meta.end.is_none() {
            let outcome = RelayOutcome::dropped(&self.cut);
            self.record(&outcome);
        }
    }
}

/// Header map as JSON-able strings; sensitive values are redacted.
pub fn headers_for_record(headers: &HeaderMap) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (name, value) in headers {
        let text = if value.is_sensitive() {
            "<redacted>".to_owned()
        } else {
            value.to_str().unwrap_or("<binary>").to_owned()
        };
        out.entry(name.to_string())
            .and_modify(|existing: &mut String| {
                existing.push_str(", ");
                existing.push_str(&text);
            })
            .or_insert(text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(text: &str) -> jiff::Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn stamp_round_trips_through_directory_names() {
        let now = ts("2026-09-06T02:38:29.457Z");
        let name = format!("{}-rtr_abc", dir_stamp(now));
        assert_eq!(name, "20260906T023829.457Z-rtr_abc");
        assert_eq!(entry_stamp(&name), Some(now));
        assert_eq!(entry_stamp("notes"), None);
        assert_eq!(
            entry_stamp("20260906T023829Z-rtr_abc"),
            None,
            "millis are mandatory"
        );
    }

    #[test]
    fn prune_removes_only_old_stamped_entries() {
        let dir = tempfile::tempdir().unwrap();
        let log = BodyLog::open(dir.path(), Duration::from_secs(3600)).unwrap();
        let now = ts("2026-09-06T12:00:00Z");
        for name in [
            "20260906T105959.000Z-rtr_old",   // 2h old
            "20260906T113000.000Z-rtr_fresh", // 30m old
            "20260906T120000.000Z-rtr_now",
            "unrelated-directory",
        ] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
            std::fs::write(dir.path().join(name).join("meta.json"), "{}").unwrap();
        }
        std::fs::write(dir.path().join("stray-file"), "").unwrap();
        assert_eq!(log.prune(now), 1);
        let mut left: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            vec![
                "20260906T113000.000Z-rtr_fresh",
                "20260906T120000.000Z-rtr_now",
                "stray-file",
                "unrelated-directory"
            ]
        );
        assert_eq!(log.prune(now), 0, "idempotent");
    }

    #[test]
    fn entries_list_newest_first_and_only_their_own_files_are_reachable() {
        let dir = tempfile::tempdir().unwrap();
        let log = BodyLog::open(dir.path(), Duration::ZERO).unwrap();
        for (name, status) in [
            ("20260911T100000.000Z-rtr_old", 200),
            ("20260911T110000.000Z-rtr_new", 400),
        ] {
            let entry = dir.path().join(name);
            std::fs::create_dir(&entry).unwrap();
            std::fs::write(
                entry.join("meta.json"),
                format!(r#"{{"model": "fast", "backend": "mock", "path": "/v1/messages", "status": {status}, "outcome": "complete", "stream": true, "messages": 3, "prompt": "Read it", "step": "← Read", "session": "s-1", "request_headers": []}}"#),
            )
            .unwrap();
            std::fs::write(entry.join("request.json"), "{}").unwrap();
            std::fs::write(entry.join("stray.txt"), "not listed").unwrap();
        }
        std::fs::create_dir(dir.path().join("unrelated")).unwrap();

        let entries = log.list(10).entries;
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "20260911T110000.000Z-rtr_new",
                "20260911T100000.000Z-rtr_old"
            ]
        );
        let newest = &entries[0];
        assert_eq!(newest.request_id, "rtr_new");
        assert_eq!(newest.at, ts("2026-09-11T11:00:00Z"));
        assert_eq!(newest.files, ["meta.json", "request.json"]);
        assert_eq!(newest.status, Some(400));
        assert_eq!(newest.model.as_deref(), Some("fast"));
        assert_eq!(newest.messages, Some(3));
        assert_eq!(newest.prompt.as_deref(), Some("Read it"));
        assert_eq!(newest.step.as_deref(), Some("← Read"));
        assert_eq!(newest.stream, Some(true));
        assert_eq!(newest.session.as_deref(), Some("s-1"));
        assert!(!newest.unreadable);
        assert!(newest.bytes > 2);

        // A `meta.json` that does not read as JSON is said to be unreadable,
        // rather than listed as an exchange that never finished.
        let broken = dir.path().join("20260911T120000.000Z-rtr_broken");
        std::fs::create_dir(&broken).unwrap();
        std::fs::write(broken.join("meta.json"), "{not json").unwrap();
        let listed = log.list(10).entries;
        assert!(listed[0].unreadable, "{:?}", listed[0]);
        assert_eq!(listed[0].outcome, None);
        assert!(listed[1..].iter().all(|e| !e.unreadable));
        std::fs::remove_dir_all(&broken).unwrap();
        let newest_only = log.list(1);
        assert_eq!(newest_only.entries.len(), 1);
        assert_eq!(
            newest_only.total, 2,
            "the count covers what the limit left out"
        );

        assert!(
            log.file("20260911T100000.000Z-rtr_old", "meta.json")
                .is_some()
        );
        for (name, file) in [
            ("20260911T100000.000Z-rtr_old", "stray.txt"),
            ("20260911T100000.000Z-rtr_old", "../meta.json"),
            ("unrelated", "meta.json"),
            ("20260911T100000.000Z-rtr_old/..", "meta.json"),
            ("20260911T100000.000Z-../../etc", "meta.json"),
        ] {
            assert!(log.file(name, file).is_none(), "{name}/{file}");
        }

        assert!(!log.remove("unrelated").unwrap());
        assert!(dir.path().join("unrelated").exists());
        assert!(log.remove("20260911T100000.000Z-rtr_old").unwrap());
        assert!(!log.remove("20260911T100000.000Z-rtr_old").unwrap());
        assert_eq!(log.remove_all(), 1);
        assert_eq!(log.list(10), Listing::default());
        assert!(dir.path().join("unrelated").exists());
    }

    #[test]
    fn zero_retention_never_prunes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("20200101T000000.000Z-rtr_ancient")).unwrap();
        let log = BodyLog::open(dir.path(), Duration::ZERO).unwrap();
        assert_eq!(log.prune(ts("2026-09-06T12:00:00Z")), 0);
        assert!(dir.path().join("20200101T000000.000Z-rtr_ancient").exists());
    }
}
