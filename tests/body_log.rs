mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::response::Response;
use futures_util::StreamExt;
use serde_json::Value;
use support::mock_upstream::echo;
use support::router::{config_with_backend, messages_body};
use support::{MockUpstream, TestRouter};

fn body(model: &str, stream: bool) -> Value {
    let mut body = messages_body(model);
    body["stream"] = Value::Bool(stream);
    body
}

/// Waits until `count` request directories carry a finished `meta.json`
/// (one with `outcome`). Files are renamed into place, so a present file is a
/// complete file.
async fn wait_for_entries(dir: &Path, count: usize) -> Vec<PathBuf> {
    for _ in 0..100 {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        entries.sort();
        let finished = entries
            .iter()
            .filter(|e| {
                std::fs::read(e.join("meta.json"))
                    .ok()
                    .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                    .is_some_and(|m| m.get("outcome").is_some())
            })
            .count();
        if finished >= count {
            return entries;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let listing = walkdir(dir);
    panic!(
        "body log never reached {count} complete entries in {}; found {listing:?}",
        dir.display()
    );
}

fn walkdir(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            out.push(e.path().display().to_string());
            if e.path().is_dir() {
                out.extend(walkdir(&e.path()));
            }
        }
    }
    out
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap())
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[tokio::test]
async fn records_request_and_json_response() {
    let upstream = MockUpstream::start(echo).await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!(
        "[backends.mock.headers]\ncf-access-client-secret = \"forced-secret\"\n[logging]\nbody_dir = \"{}\"\n",
        dir.path().join("bodies").display()
    );
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;

    let res = router
        .post("/v1/messages", &body("fast", false))
        .header("anthropic-beta", "x-beta")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let request_id = res.headers()["x-request-id"].to_str().unwrap().to_owned();
    let _ = res.bytes().await.unwrap();

    let entries = wait_for_entries(&dir.path().join("bodies"), 1).await;
    let entry = &entries[0];
    assert!(
        entry
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .ends_with(&request_id),
        "{entry:?}"
    );

    let request = read_json(&entry.join("request.json"));
    assert_eq!(
        request["model"], "mock-fast-v1",
        "the body as sent upstream"
    );

    let response = read_json(&entry.join("response.json"));
    assert_eq!(response["echo"]["body"]["model"], "mock-fast-v1");

    let meta = read_json(&entry.join("meta.json"));
    assert_eq!(meta["request_id"], request_id);
    assert_eq!(meta["backend"], "mock");
    assert_eq!(meta["model"], "fast");
    assert_eq!(meta["requested_model"], "fast");
    assert_eq!(meta["upstream_model"], "mock-fast-v1");
    assert_eq!(meta["stream"], false);
    assert_eq!(meta["method"], "POST");
    assert_eq!(meta["path"], "/v1/messages");
    assert_eq!(meta["status"], 200);
    assert_eq!(meta["attempts"], 1);
    assert_eq!(meta["outcome"], "complete");
    assert!(meta["response_bytes"].as_u64().unwrap() > 0);
    assert!(meta["duration_ms"].is_number());
    assert_eq!(meta["response_headers"]["content-type"], "application/json");
}

/// The recording is the header set the backend saw: nothing the router or its
/// HTTP client added is missing from it, each header says where it came from,
/// and no secret is written.
#[tokio::test]
async fn the_recording_names_every_header_the_backend_saw() {
    let upstream = MockUpstream::start(echo).await;
    let dir = tempfile::tempdir().unwrap();
    let config = format!(
        r#"
[server]
listen = "127.0.0.1:0"
token = "router-test-token"

[backends.mock]
url = "{}"
credential = {{ kind = "static", value = "backend-secret-key" }}
anthropic_beta = ["oauth-2025-04-20"]
drop_headers = ["@claude-code"]
headers = {{ "cf-access-client-secret" = "forced-secret", "x-gateway-route" = "gateway" }}

[[models]]
id = "fast"
backend = "mock"
upstream_model = "mock-fast-v1"

[logging]
body_dir = "{}"
"#,
        upstream.url(),
        dir.path().display()
    );
    let router = TestRouter::start(&config).await;

    let res = router
        .post("/v1/messages", &body("fast", false))
        .header("anthropic-beta", "x-beta")
        .header("x-app", "cli")
        .header("x-gateway-route", "from-the-client")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let _ = res.bytes().await.unwrap();

    let entries = wait_for_entries(dir.path(), 1).await;
    let meta = read_json(&entries[0].join("meta.json"));
    let recorded: Vec<(String, String, String)> = meta["request_headers"]
        .as_array()
        .expect("request_headers is a list of {name, value, source}")
        .iter()
        .map(|h| {
            (
                h["name"].as_str().unwrap().to_owned(),
                h["value"].as_str().unwrap().to_owned(),
                h["source"].as_str().unwrap().to_owned(),
            )
        })
        .collect();

    let response = read_json(&entries[0].join("response.json"));
    let mut seen: Vec<String> = response["echo"]["headers"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    seen.sort();
    let mut names: Vec<String> = recorded.iter().map(|(name, ..)| name.clone()).collect();
    names.sort();
    assert_eq!(names, seen, "every header the backend saw is recorded");

    let by_name = |name: &str| {
        recorded
            .iter()
            .find(|(n, ..)| n == name)
            .map(|(_, value, source)| (value.as_str(), source.as_str()))
            .unwrap_or_else(|| panic!("{name} not recorded: {recorded:?}"))
    };
    assert_eq!(
        by_name("authorization"),
        ("Bearer <redacted>", "credential"),
        "the credential is named and its value is not written"
    );
    assert_eq!(
        by_name("cf-access-client-secret"),
        ("<redacted>", "backend"),
        "a forced header may be a secret; the source says it came from the config"
    );
    assert_eq!(
        by_name("anthropic-beta"),
        ("x-beta,oauth-2025-04-20", "backend")
    );
    assert_eq!(by_name("content-type"), ("application/json", "default"));
    assert_eq!(by_name("host").1, "transport");

    let dropped: Vec<(String, String, String)> = meta["dropped_headers"]
        .as_array()
        .expect("dropped_headers is a list of {name, value, reason}")
        .iter()
        .map(|h| {
            (
                h["name"].as_str().unwrap().to_owned(),
                h["value"].as_str().unwrap().to_owned(),
                h["reason"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    let dropped_by_name = |name: &str| {
        dropped
            .iter()
            .find(|(n, ..)| n == name)
            .map(|(_, value, reason)| (value.as_str(), reason.as_str()))
            .unwrap_or_else(|| panic!("{name} not recorded as dropped: {dropped:?}"))
    };
    assert_eq!(
        dropped_by_name("x-api-key"),
        ("<redacted>", "client_credential"),
        "the token the client authenticated with is named, never written"
    );
    assert_eq!(dropped_by_name("x-app").1, "drop_headers");
    assert_eq!(
        dropped_by_name("x-app").0,
        "cli",
        "a dropped value is shown"
    );
    assert_eq!(
        dropped_by_name("x-gateway-route"),
        ("from-the-client", "overridden"),
        "a value the backend's `headers` replaced never reached it either"
    );
    assert_eq!(
        by_name("x-gateway-route"),
        ("<redacted>", "backend"),
        "and the backend's own value is what arrived"
    );
    let both: Vec<&String> = names
        .iter()
        .filter(|name| dropped.iter().any(|(n, ..)| n == *name))
        .collect();
    assert_eq!(
        both,
        ["content-length", "host", "x-gateway-route"],
        "a name is in both reports only when the HTTP client reframed it or the backend replaced the client's value"
    );
    assert_ne!(
        dropped_by_name("host").0,
        by_name("host").0,
        "the client addressed the router; the backend is addressed instead"
    );
}

#[tokio::test]
async fn a_recorded_response_stops_growing_at_the_cap() {
    let cap = anthroxy::observability::MAX_RECORDED_RESPONSE_BYTES;
    let frame = format!("data: {}\n\n", "x".repeat(1024 * 1024 - 8));
    let frames = cap / frame.len() + 2;
    let upstream = MockUpstream::start(move |_| {
        let frame = frame.clone();
        let events = futures_util::stream::iter(0..frames)
            .map(move |_| Ok::<_, std::io::Error>(frame.clone()));
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(events))
            .unwrap()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!("[logging]\nbody_dir = \"{}\"\n", dir.path().display());
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;
    let res = router
        .post("/v1/messages", &body("smart", true))
        .send()
        .await
        .unwrap();
    let relayed = res.bytes().await.unwrap().len();
    assert!(relayed > cap, "the client still gets everything");

    let entries = wait_for_entries(dir.path(), 1).await;
    let meta = read_json(&entries[0].join("meta.json"));
    assert_eq!(meta["truncated"], true, "{meta}");
    assert_eq!(meta["response_bytes"], relayed);
    let recorded = std::fs::metadata(entries[0].join("response.sse"))
        .unwrap()
        .len();
    assert!(recorded as usize <= cap, "{recorded}");
}

/// A pruned or deleted entry stays gone: the rest of its stream is dropped
/// rather than recreating the directory with a torso of the response.
#[tokio::test]
async fn an_entry_deleted_while_recording_is_not_recreated() {
    let upstream = MockUpstream::start(|_| {
        let events = futures_util::stream::iter(0..6).then(|i| async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            Ok::<_, std::io::Error>(format!("data: {{\"n\":{i}}}\n\n"))
        });
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(events))
            .unwrap()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!("[logging]\nbody_dir = \"{}\"\n", dir.path().display());
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;
    let res = router
        .post("/v1/messages", &body("smart", true))
        .send()
        .await
        .unwrap();
    let mut entries = Vec::new();
    for _ in 0..50 {
        entries = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        if !entries.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(entries.len(), 1);
    std::fs::remove_dir_all(&entries[0]).unwrap();
    let text = res.text().await.unwrap();
    assert_eq!(text.matches("data:").count(), 6);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let left: Vec<PathBuf> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn records_sse_stream_after_completion() {
    let upstream = MockUpstream::start(|_| {
        let events = futures_util::stream::iter(0..3).then(|i| async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok::<_, std::io::Error>(format!("data: {{\"n\":{i}}}\n\n"))
        });
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(events))
            .unwrap()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!("[logging]\nbody_dir = \"{}\"\n", dir.path().display());
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;

    let res = router
        .post("/v1/messages", &body("smart", true))
        .send()
        .await
        .unwrap();
    let text = res.text().await.unwrap();
    assert_eq!(text.matches("data:").count(), 3);

    let entries = wait_for_entries(dir.path(), 1).await;
    let sse = std::fs::read_to_string(entries[0].join("response.sse")).unwrap();
    assert_eq!(sse, text, "stream recorded verbatim");
    let meta = read_json(&entries[0].join("meta.json"));
    assert_eq!(meta["stream"], true);
    assert_eq!(meta["outcome"], "complete");
    assert_eq!(meta["response_bytes"], text.len());
}

#[tokio::test]
async fn records_upstream_error_bodies_too() {
    let upstream = MockUpstream::start(|_| {
        Response::builder()
            .status(500)
            .header("content-type", "text/plain")
            .body(Body::from("boom"))
            .unwrap()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!("[logging]\nbody_dir = \"{}\"\n", dir.path().display());
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;

    let res = router
        .post("/v1/messages", &body("fast", false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 500);

    let entries = wait_for_entries(dir.path(), 1).await;
    assert_eq!(
        std::fs::read_to_string(entries[0].join("response.bin")).unwrap(),
        "boom"
    );
    let meta = read_json(&entries[0].join("meta.json"));
    assert_eq!(meta["status"], 500);
    assert_eq!(meta["outcome"], "complete");
}

#[tokio::test]
async fn records_a_stream_that_broke_off_mid_body() {
    // Headers and one event go out, then the backend drops the connection.
    let upstream = MockUpstream::start(|_| {
        let chunks = futures_util::stream::iter(0..2).then(|i| async move {
            tokio::time::sleep(Duration::from_millis(20 * i)).await;
            if i == 0 {
                Ok("event: ping\ndata: {}\n\n")
            } else {
                Err(std::io::Error::other("connection reset"))
            }
        });
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(chunks))
            .unwrap()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!(
        "[logging]\nbody_dir = \"{}\"\n",
        dir.path().join("bodies").display()
    );
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;

    let res = router
        .post("/v1/messages", &body("fast", true))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "the status was already on its way");
    assert!(res.bytes().await.is_err(), "the body ends unfinished");

    let entries = wait_for_entries(&dir.path().join("bodies"), 1).await;
    let meta = read_json(&entries[0].join("meta.json"));
    assert_eq!(meta["status"], 200);
    assert!(
        meta["outcome"]
            .as_str()
            .unwrap()
            .starts_with("upstream_error:"),
        "{meta}"
    );
    let recorded = std::fs::read_to_string(entries[0].join("response.sse")).unwrap();
    assert_eq!(
        recorded, "event: ping\ndata: {}\n\n",
        "what did arrive is kept"
    );
}

#[tokio::test]
async fn records_a_request_that_never_reached_the_backend() {
    let dir = tempfile::tempdir().unwrap();
    let extra = format!(
        "[logging]\nbody_dir = \"{}\"\n\n[upstream]\nretries = 0\n",
        dir.path().join("bodies").display()
    );
    // Nothing listens there.
    let router = TestRouter::start(&config_with_backend("http://127.0.0.1:1", &extra)).await;

    let res = router
        .post("/v1/messages", &body("fast", false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 502);

    let entries = wait_for_entries(&dir.path().join("bodies"), 1).await;
    let meta = read_json(&entries[0].join("meta.json"));
    assert_eq!(meta["backend"], "mock");
    assert!(
        meta["outcome"]
            .as_str()
            .unwrap()
            .starts_with("upstream_error:"),
        "{meta}"
    );
    assert!(meta.get("status").is_none(), "no response ever arrived");
}

#[tokio::test]
async fn one_directory_per_request() {
    let upstream = MockUpstream::start(echo).await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!("[logging]\nbody_dir = \"{}\"\n", dir.path().display());
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;
    for _ in 0..3 {
        router
            .post("/v1/messages", &body("fast", false))
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
    }
    let entries = wait_for_entries(dir.path(), 3).await;
    assert_eq!(entries.len(), 3);
}

/// The recording is the conversation itself, so it must not be readable by
/// other users of the machine — `logging.body_dir` is often a shared path.
#[cfg(unix)]
#[tokio::test]
async fn recordings_are_private_to_the_user_running_the_router() {
    use std::os::unix::fs::PermissionsExt;

    let upstream = MockUpstream::start(echo).await;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("bodies");
    let extra = format!("[logging]\nbody_dir = \"{}\"\n", root.display());
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;

    let res = router
        .post("/v1/messages", &body("fast", false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let _ = res.bytes().await.unwrap();

    let entries = wait_for_entries(&root, 1).await;
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&root), 0o700, "body log root");
    assert_eq!(mode(&entries[0]), 0o700, "entry directory");
    for name in ["request.json", "response.json", "meta.json"] {
        assert_eq!(mode(&entries[0].join(name)), 0o600, "{name}");
    }
}

/// A stream is only worth watching while it runs, so the response file grows
/// as chunks arrive rather than appearing whole at the end.
#[tokio::test]
async fn records_a_stream_chunk_by_chunk_while_it_runs() {
    let upstream = MockUpstream::start(|_| {
        // Big enough that a chunk crosses the flush threshold on its own.
        let events = futures_util::stream::iter(0..6).then(|i| async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            Ok::<_, std::io::Error>(format!(
                "data: {{\"n\":{i},\"pad\":\"{}\"}}\n\n",
                "x".repeat(16 * 1024)
            ))
        });
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(events))
            .unwrap()
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!("[logging]\nbody_dir = \"{}\"\n", dir.path().display());
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;

    let mut res = router
        .post("/v1/messages", &body("smart", true))
        .send()
        .await
        .unwrap()
        .bytes_stream();

    // Read two events, then look at the log while four are still to come.
    let mut read = 0;
    while read < 2 {
        let chunk = res.next().await.expect("stream ended early").unwrap();
        read += chunk.iter().filter(|b| **b == b'\n').count() / 2;
    }
    let partial = wait_for_growing_response(dir.path()).await;
    assert!(
        partial > 0,
        "the response file should hold what has arrived, not wait for the end"
    );

    while let Some(chunk) = res.next().await {
        chunk.unwrap();
    }
    let entries = wait_for_entries(dir.path(), 1).await;
    let sse = std::fs::read_to_string(entries[0].join("response.sse")).unwrap();
    assert_eq!(
        sse.matches("data:").count(),
        6,
        "every event ends up recorded"
    );
    assert!(
        sse.len() > partial,
        "the file kept growing after the look, not rewritten from scratch"
    );
}

/// Bytes in the one entry's response file, once it has any. The wait is
/// bounded well under what the rest of the stream takes, so seeing bytes here
/// means they were written while the exchange was still running.
async fn wait_for_growing_response(dir: &Path) -> usize {
    for _ in 0..20 {
        let entry = std::fs::read_dir(dir)
            .ok()
            .and_then(|rd| rd.flatten().map(|e| e.path()).next());
        if let Some(entry) = entry {
            let size = crate::support::body_log::response_size(&entry);
            if size > 0 {
                return size;
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("no response bytes were recorded while the stream was running");
}
