//! What the router remembers about each exchange: in memory for the
//! console, and one statistics line per `/v1/messages` request.

mod support;

use std::path::Path;
use std::time::Duration;

use anthroxy::activity::{ExchangeView, Outcome};
use anthroxy::anthropic::TokenUsage;
use anthroxy::config::BackendKind;
use anthroxy::stats::StatsRecord;
use axum::body::Body;
use axum::response::Response;
use serde_json::json;
use support::mock_upstream::{echo, json_response};
use support::openai::{chunk, config_with_openai_backend, sse_response, usage_and_done};
use support::router::{config_with_backend, messages_body};
use support::{MockUpstream, TestRouter};

const ANTHROPIC_SSE: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"mock-fast-v1\",\"usage\":{\"input_tokens\":12,\"cache_read_input_tokens\":300,\"cache_creation_input_tokens\":4,\"output_tokens\":1}}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"secret answer\"}}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":9}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

fn sse(body: &'static str) -> Response {
    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from(body))
        .unwrap()
}

fn stats_files(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.sort();
    files
}

fn read_stats(dir: &Path) -> Vec<StatsRecord> {
    stats_files(dir)
        .iter()
        .flat_map(|path| {
            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect::<Vec<StatsRecord>>()
        })
        .collect()
}

async fn stats_records(dir: &Path, count: usize) -> Vec<StatsRecord> {
    for _ in 0..300 {
        let records = read_stats(dir);
        if records.len() >= count {
            return records;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "statistics never reached {count} record(s): {:?}",
        read_stats(dir)
    );
}

async fn finished(router: &TestRouter, id: &str) -> ExchangeView {
    for _ in 0..300 {
        if let Some(view) = router.reload.activity.find(id)
            && view.outcome.is_some()
        {
            return view;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "exchange {id} never finished: {:?}",
        router.reload.activity.find(id)
    );
}

fn request_id(res: &reqwest::Response) -> String {
    res.headers()["x-request-id"].to_str().unwrap().to_owned()
}

#[tokio::test]
async fn a_streamed_exchange_is_tracked_and_persisted_with_its_usage() {
    let upstream = MockUpstream::start(|_| sse(ANTHROPIC_SSE)).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let mut body = messages_body("claude-haiku-4-5");
    body["stream"] = json!(true);
    let res = router.post("/v1/messages", &body).send().await.unwrap();
    let id = request_id(&res);
    assert_eq!(
        res.text().await.unwrap(),
        ANTHROPIC_SSE,
        "relayed byte for byte"
    );

    let view = finished(&router, &id).await;
    assert_eq!(view.outcome, Some(Outcome::Complete));
    assert_eq!(view.model.as_deref(), Some("fast"));
    assert_eq!(view.backend.as_deref(), Some("mock"));
    assert_eq!(view.matched, Some("alias"));
    assert_eq!(view.status, Some(200));
    assert!(view.peer.is_some() && view.ttfb_ms.is_some());
    let usage = Some(TokenUsage {
        input: 12,
        output: 9,
        cache_read: 300,
        cache_creation: 4,
    });
    assert_eq!(view.usage, usage);

    let records = stats_records(&router.stats_dir, 1).await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, id);
    assert_eq!(records[0].usage, usage);
    assert_eq!(records[0].model.as_deref(), Some("fast"));
    assert_eq!(records[0].upstream_model.as_deref(), Some("mock-fast-v1"));
    let text = std::fs::read_to_string(&stats_files(&router.stats_dir)[0]).unwrap();
    assert!(
        !text.contains("secret answer") && !text.contains("\"u1\""),
        "no content in statistics: {text}"
    );
}

#[tokio::test]
async fn an_unknown_model_is_tallied_tracked_and_persisted() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &messages_body("ghost"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let view = finished(&router, &request_id(&res)).await;
    assert_eq!(
        (view.status, view.outcome),
        (Some(404), Some(Outcome::Error))
    );
    assert!(view.error.as_deref().unwrap().contains("ghost"), "{view:?}");
    let names = router.reload.activity.names();
    assert_eq!(names.len(), 1);
    assert_eq!((names[0].name.as_str(), names[0].unknown), ("ghost", 1));

    let records = stats_records(&router.stats_dir, 1).await;
    assert_eq!(
        (records[0].status, records[0].model.clone()),
        (Some(404), None)
    );
    assert_eq!(records[0].requested_model.as_deref(), Some("ghost"));
}

#[tokio::test]
async fn an_upstream_error_keeps_its_body_and_a_drop_fields_hint() {
    let upstream = MockUpstream::start(|_| {
        json_response(
            400,
            json!({"object": "error", "message": "[{'type': 'extra_forbidden', 'loc': ('body', 'future_field'), 'msg': 'Extra inputs are not permitted'}]", "type": "BadRequestError", "code": 400}),
        )
    })
    .await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let view = finished(&router, &request_id(&res)).await;
    assert_eq!(view.outcome, Some(Outcome::Error));
    assert!(
        view.error_body
            .as_deref()
            .unwrap()
            .contains("extra_forbidden")
    );
    assert_eq!(view.hints.len(), 1, "{view:?}");
    assert_eq!(
        view.hints[0].snippet.as_deref(),
        Some("[backends.mock]\ndrop_fields = [\"future_field\"]")
    );
}

#[tokio::test]
async fn an_openai_stream_reports_the_usage_the_client_received() {
    let upstream = MockUpstream::start(|_| {
        let mut frames = vec![
            chunk(json!({"role": "assistant", "content": "hi"}), None),
            chunk(json!({}), Some("stop")),
        ];
        frames.extend(usage_and_done(21, 8));
        sse_response(frames, Duration::ZERO)
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let mut body = messages_body("qwen");
    body["stream"] = json!(true);
    let res = router.post("/v1/messages", &body).send().await.unwrap();
    let id = request_id(&res);
    assert!(res.text().await.unwrap().contains("message_stop"));
    let view = finished(&router, &id).await;
    assert_eq!(view.kind, Some(BackendKind::OpenAi));
    assert_eq!(view.outcome, Some(Outcome::Complete));
    assert_eq!(
        view.usage,
        Some(TokenUsage {
            input: 21,
            output: 8,
            ..TokenUsage::default()
        })
    );
    assert_eq!(
        stats_records(&router.stats_dir, 1).await[0].usage,
        view.usage
    );
}

#[tokio::test]
async fn count_tokens_is_tracked_but_not_persisted() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages/count_tokens", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    let counted = request_id(&res);
    let _ = res.text().await;
    finished(&router, &counted).await;
    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    let message = request_id(&res);
    let _ = res.text().await;

    let records = stats_records(&router.stats_dir, 1).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let records_later = read_stats(&router.stats_dir);
    assert_eq!(records.len(), 1);
    assert_eq!(records_later.len(), 1);
    assert_eq!(records[0].id, message);
    let recent: Vec<String> = router
        .reload
        .activity
        .recent()
        .iter()
        .map(|v| v.id.clone())
        .collect();
    assert!(recent.contains(&counted), "{recent:?}");
}

#[tokio::test]
async fn disabled_statistics_write_nothing() {
    let upstream = MockUpstream::start(echo).await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!(
        "\n[stats]\nenabled = false\ndir = \"{}\"\n",
        dir.path().join("stats").display()
    );
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;
    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    let id = request_id(&res);
    let _ = res.text().await;
    finished(&router, &id).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!dir.path().join("stats").exists());
}

#[tokio::test]
async fn a_recorded_exchange_names_its_recording() {
    let upstream = MockUpstream::start(echo).await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!(
        "\n[logging]\nbody_dir = \"{}\"\n",
        dir.path().join("bodies").display()
    );
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;
    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    let id = request_id(&res);
    let _ = res.text().await;
    let view = finished(&router, &id).await;
    let entry = view.recording.expect("a recorded exchange names its entry");
    assert!(entry.ends_with(&id), "{entry}");
    assert!(dir.path().join("bodies").join(&entry).is_dir());
}

#[tokio::test]
async fn how_the_model_name_matched_is_persisted() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(
        &upstream.url(),
        "\n[routing]\ndefault_model = \"fast\"\n",
    ))
    .await;
    for model in ["smart", "claude-haiku-4-5", "claude-opus-9"] {
        let res = router
            .post("/v1/messages", &messages_body(model))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200, "{model}");
        let _ = res.text().await;
    }
    let mut seen: Vec<(String, Option<String>)> = stats_records(&router.stats_dir, 3)
        .await
        .into_iter()
        .map(|r| (r.requested_model.unwrap(), r.matched))
        .collect();
    seen.sort();
    assert_eq!(
        seen,
        [
            ("claude-haiku-4-5".to_owned(), Some("alias".to_owned())),
            ("claude-opus-9".to_owned(), Some("default".to_owned())),
            ("smart".to_owned(), Some("exact".to_owned())),
        ]
    );
}

#[tokio::test]
async fn a_credential_re_acquired_after_a_401_is_tracked_and_persisted() {
    let upstream = MockUpstream::start(|received| {
        if received.header("authorization") == Some("Bearer tok2") {
            echo(received)
        } else {
            json_response(
                401,
                json!({"type":"error","error":{"type":"authentication_error","message":"stale"}}),
            )
        }
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("n");
    std::fs::write(&counter, "0").unwrap();
    let command = format!(
        "n=$(cat {c}); n=$((n+1)); echo $n > {c}; echo tok$n",
        c = counter.display()
    );
    let config = config_with_backend(&upstream.url(), "").replace(
        r#"credential = { kind = "static", value = "backend-secret-key" }"#,
        &format!(r#"credential = {{ kind = "command", command = '{command}' }}"#),
    );
    let router = TestRouter::start(&config).await;

    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    let id = request_id(&res);
    assert_eq!(res.status(), 200);
    let _ = res.text().await;

    let view = finished(&router, &id).await;
    assert_eq!(view.attempts, Some(2));
    assert!(view.credential_refreshed, "{view:?}");
    let records = stats_records(&router.stats_dir, 1).await;
    assert!(records[0].credential_refreshed, "{:?}", records[0]);
}
