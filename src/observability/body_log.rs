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

use crate::server::relay::RelayOutcome;

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
    pub request_headers: BTreeMap<String, String>,
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
/// recorder dropped before [`Recorder::finish`] records a client that left:
/// a handler awaiting a buffered body is dropped when its client goes away.
pub struct Recorder {
    dir: PathBuf,
    meta: Meta,
    started: Instant,
    response: Vec<u8>,
    response_file: &'static str,
    /// The initial write; the final write is ordered after it so `meta.json`
    /// always ends in its complete form. Taken by the final write.
    pending: Option<tokio::task::JoinHandle<()>>,
}

impl BodyLog {
    /// `retention` of zero disables pruning.
    pub fn open(root: &Path, retention: Duration) -> std::io::Result<Self> {
        std::fs::create_dir_all(root)?;
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
    pub fn begin(&self, record: RequestRecord, body: &Bytes, started: Instant) -> Recorder {
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
        }
    }
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
    let write = move || {
        let _guard = span.enter();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            tracing::error!(dir = %dir.display(), %error, "cannot create body log directory");
            return;
        }
        for (name, bytes) in files {
            let path = dir.join(name);
            let temp = dir.join(format!("{name}.tmp"));
            if let Err(error) =
                std::fs::write(&temp, bytes).and_then(|()| std::fs::rename(&temp, &path))
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
            self.record(&RelayOutcome::ClientDisconnected);
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
    fn zero_retention_never_prunes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("20200101T000000.000Z-rtr_ancient")).unwrap();
        let log = BodyLog::open(dir.path(), Duration::ZERO).unwrap();
        assert_eq!(log.prune(ts("2026-09-06T12:00:00Z")), 0);
        assert!(dir.path().join("20200101T000000.000Z-rtr_ancient").exists());
    }
}
