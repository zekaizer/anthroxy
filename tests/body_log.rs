mod support;

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::body::Body;
use axum::response::Response;
use futures_util::StreamExt;
use serde_json::{Value, json};
use support::mock_upstream::echo;
use support::router::config_with_backend;
use support::{MockUpstream, TestRouter};

fn body(model: &str, stream: bool) -> Value {
    json!({"model": model, "max_tokens": 8, "stream": stream, "messages": [{"role": "user", "content": "hi"}]})
}

/// Waits for the asynchronous writer to finish `count` request directories.
async fn wait_for_entries(dir: &Path, count: usize) -> Vec<PathBuf> {
    for _ in 0..100 {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
            .unwrap_or_default();
        entries.sort();
        let complete = entries
            .iter()
            .filter(|e| e.join("meta.json").exists() && e.join("request.json").exists())
            .count();
        if complete >= count
            && entries
                .iter()
                .all(|e| std::fs::read_dir(e).unwrap().count() >= 3)
        {
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
        "[logging]\nbody_dir = \"{}\"\n",
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
    assert_eq!(
        meta["request_headers"]["anthropic-beta"],
        "x-beta,oauth-2025-04-20"
    );
    assert_eq!(
        meta["request_headers"]["authorization"], "<redacted>",
        "credentials never land on disk"
    );
    assert_eq!(meta["response_headers"]["content-type"], "application/json");
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
