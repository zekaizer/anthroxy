//! Writes each proxied exchange to `<body_dir>/<time>-<request id>/`:
//! `request.json` (as sent upstream), `response.<json|sse|bin>` (as received)
//! and `meta.json` (routing, status, timing, headers with credentials
//! redacted). Enabled by `logging.body_dir`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use bytes::Bytes;
use http::{HeaderMap, StatusCode};
use serde::Serialize;

use crate::server::relay::{RelayObserver, RelayOutcome};

#[derive(Debug, Clone)]
pub struct BodyLog {
    root: PathBuf,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attempts: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_headers: Option<BTreeMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_bytes: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
}

/// Accumulates one exchange and writes it out when the response ends.
pub struct Recorder {
    dir: PathBuf,
    meta: Meta,
    started: Instant,
    response: Vec<u8>,
    response_ext: &'static str,
}

impl BodyLog {
    pub fn open(root: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(root)?;
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Starts a record and writes `request.json` plus a first `meta.json`.
    pub fn begin(&self, record: RequestRecord, body: &Bytes, started: Instant) -> Recorder {
        let dir = self
            .root
            .join(format!("{}-{}", dir_stamp(), record.request_id));
        let meta = Meta {
            request: record,
            status: None,
            attempts: None,
            latency_ms: None,
            response_headers: None,
            outcome: None,
            response_bytes: None,
            duration_ms: None,
        };
        write_files(
            dir.clone(),
            vec![
                ("request.json", body.to_vec()),
                ("meta.json", to_pretty_json(&meta)),
            ],
        );
        Recorder {
            dir,
            meta,
            started,
            response: Vec::new(),
            response_ext: "bin",
        }
    }
}

/// `YYYYMMDDTHHMMSS.mmmZ`, sortable and file-name safe.
fn dir_stamp() -> String {
    let now = jiff::Timestamp::now();
    format!(
        "{}.{:03}Z",
        now.strftime("%Y%m%dT%H%M%S"),
        now.subsec_millisecond()
    )
}

fn to_pretty_json(value: &impl Serialize) -> Vec<u8> {
    serde_json::to_vec_pretty(value).expect("records serialize")
}

/// Writes off the request path; failures are logged, never propagated.
fn write_files(dir: PathBuf, files: Vec<(&'static str, Vec<u8>)>) {
    let span = tracing::Span::current();
    let write = move || {
        let _guard = span.enter();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            tracing::error!(dir = %dir.display(), %error, "cannot create body log directory");
            return;
        }
        for (name, bytes) in files {
            let path = dir.join(name);
            if let Err(error) = std::fs::write(&path, bytes) {
                tracing::error!(path = %path.display(), %error, "cannot write body log file");
            }
        }
    };
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn_blocking(write);
        }
        Err(_) => write(),
    }
}

fn extension_for(headers: &HeaderMap) -> &'static str {
    let content_type = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if content_type.starts_with("text/event-stream") {
        "sse"
    } else if content_type.starts_with("application/json") {
        "json"
    } else {
        "bin"
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
        self.meta.status = Some(status.as_u16());
        self.meta.attempts = Some(attempts);
        self.meta.latency_ms = Some(latency_ms);
        self.meta.response_headers = Some(headers_for_record(headers));
        self.response_ext = extension_for(headers);
    }

    /// Records a fully buffered body and finishes.
    pub fn finish_with_body(mut self, body: &[u8]) {
        self.response.extend_from_slice(body);
        self.on_end(&RelayOutcome::Complete);
    }
}

impl RelayObserver for Recorder {
    fn on_chunk(&mut self, chunk: &Bytes) {
        self.response.extend_from_slice(chunk);
    }

    fn on_end(&mut self, outcome: &RelayOutcome) {
        self.meta.outcome = Some(match outcome {
            RelayOutcome::Complete => "complete".to_owned(),
            RelayOutcome::UpstreamError(error) => format!("upstream_error: {error}"),
            RelayOutcome::ClientDisconnected => "client_disconnected".to_owned(),
        });
        self.meta.response_bytes = Some(self.response.len());
        self.meta.duration_ms = Some(self.started.elapsed().as_millis() as u64);
        let response_name: &'static str = match self.response_ext {
            "sse" => "response.sse",
            "json" => "response.json",
            _ => "response.bin",
        };
        write_files(
            self.dir.clone(),
            vec![
                (response_name, std::mem::take(&mut self.response)),
                ("meta.json", to_pretty_json(&self.meta)),
            ],
        );
        tracing::debug!(dir = %self.dir.display(), "exchange recorded");
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
