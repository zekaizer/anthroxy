mod support;

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::response::Response;
use futures_util::StreamExt;
use serde_json::{Value, json};
use support::mock_upstream::{echo, json_response};
use support::router::{TOKEN, config_with_backend, messages_body, model_ids};
use support::{MockUpstream, TestRouter};

#[tokio::test]
async fn health_needs_no_auth() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router
        .http
        .get(router.url("/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

#[tokio::test]
async fn rejects_missing_or_wrong_client_token() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;

    let res = router
        .http
        .get(router.url("/v1/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "authentication_error");
    assert!(body["request_id"].is_string(), "{body}");

    let res = router
        .http
        .get(router.url("/v1/models"))
        .header("x-api-key", "wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);

    let res = router
        .http
        .get(router.url("/v1/models"))
        .header("authorization", format!("Bearer {TOKEN}"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "bearer form accepted");
    assert!(
        upstream.received().is_empty(),
        "auth failures never reach a backend"
    );
}

#[tokio::test]
async fn lists_configured_models_in_order() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;

    let res = router.get("/v1/models").send().await.unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(model_ids(&body), ["fast", "smart"]);
    assert_eq!(body["data"][0]["display_name"], "Fast Mock");
    assert_eq!(body["data"][0]["type"], "model");
    assert_eq!(body["first_id"], "fast");
    assert_eq!(body["last_id"], "smart");
    assert_eq!(body["has_more"], false);

    let res = router.get("/v1/models/smart").send().await.unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["id"], "smart");

    let res = router.get("/v1/models/nope").send().await.unwrap();
    assert_eq!(res.status(), 404);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["type"], "not_found_error");
}

#[tokio::test]
async fn drops_configured_fields_before_forwarding() {
    let upstream = MockUpstream::start(echo).await;
    let config = format!(
        r#"
[server]
listen = "127.0.0.1:0"
token = "{TOKEN}"

[backends.vllm]
url = "{}"
drop_fields = ["context_management", "metadata.user_id"]

[[models]]
id = "fast"
backend = "vllm"
upstream_model = "mock-fast-v1"
"#,
        upstream.url()
    );
    let router = TestRouter::start(&config).await;
    let mut body = messages_body("fast");
    body["context_management"] = json!({"edits": [{"type": "clear_tool_uses_20250919"}]});

    let res = router.post("/v1/messages", &body).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let sent = upstream.last().json();
    assert_eq!(sent.get("context_management"), None);
    assert_eq!(
        sent["metadata"],
        json!({}),
        "only the listed key is removed"
    );
    assert_eq!(sent["model"], "mock-fast-v1");
    assert_eq!(sent["future_field"]["nested"], json!([1, 2, 3]));
    let keys: Vec<&String> = sent.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        [
            "model",
            "max_tokens",
            "messages",
            "metadata",
            "future_field"
        ],
        "order kept"
    );
}

#[tokio::test]
async fn forwards_messages_with_rewritten_model_and_backend_credential() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;

    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .header(
            "anthropic-beta",
            "claude-code-20250219,interleaved-thinking-2025-05-14",
        )
        .header("user-agent", "claude-cli/2.1.0")
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["x-anthroxy-backend"], "mock");
    assert_eq!(res.headers()["x-anthroxy-model"], "fast");
    assert_eq!(res.headers()["x-anthroxy-upstream-model"], "mock-fast-v1");
    assert!(res.headers().contains_key("x-request-id"));
    assert_eq!(res.headers()["content-type"], "application/json");

    let seen = upstream.last();
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.path_and_query, "/v1/messages");
    let sent = seen.json();
    assert_eq!(
        sent["model"], "mock-fast-v1",
        "model rewritten to upstream name"
    );
    assert_eq!(
        sent["future_field"]["nested"],
        json!([1, 2, 3]),
        "unknown fields pass through"
    );
    assert_eq!(sent["metadata"]["user_id"], "u1");
    assert_eq!(
        seen.header("authorization"),
        Some("Bearer backend-secret-key")
    );
    assert_eq!(
        seen.header("x-api-key"),
        None,
        "client token never reaches the backend"
    );
    assert_eq!(seen.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(
        seen.header("anthropic-beta"),
        Some("claude-code-20250219,interleaved-thinking-2025-05-14,oauth-2025-04-20")
    );
    assert_eq!(seen.header("user-agent"), Some("claude-cli/2.1.0"));
    assert_eq!(seen.header("accept-encoding"), None);

    // Body relayed verbatim.
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["echo"]["body"]["model"], "mock-fast-v1");
}

#[tokio::test]
async fn model_named_by_alias_or_exact_id_is_not_rewritten_unnecessarily() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;

    // "smart" has no upstream_model: body is forwarded byte-for-byte.
    let raw = r#"{"model": "smart",   "max_tokens": 5, "messages": []}"#;
    let res = router
        .http
        .post(router.url("/v1/messages"))
        .header("x-api-key", TOKEN)
        .header("content-type", "application/json")
        .body(raw)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(std::str::from_utf8(&upstream.last().body).unwrap(), raw);

    // Alias resolves to the same route, model rewritten to the upstream name.
    let res = router
        .post("/v1/messages", &messages_body("claude-haiku-4-5"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(upstream.last().json()["model"], "mock-fast-v1");
    assert_eq!(res.headers()["x-anthroxy-model"], "fast");
}

#[tokio::test]
async fn unknown_model_is_404_listing_known_models() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;

    let res = router
        .post("/v1/messages", &messages_body("claude-opus-5"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["type"], "not_found_error");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("claude-opus-5") && message.contains("fast") && message.contains("smart"),
        "{message}"
    );
    assert!(upstream.received().is_empty());
}

#[tokio::test]
async fn unknown_model_uses_default_when_configured() {
    let upstream = MockUpstream::start(echo).await;
    let extra = "[routing]\ndefault_model = \"fast\"\n";
    let router = TestRouter::start(&config_with_backend(&upstream.url(), extra)).await;

    let res = router
        .post("/v1/messages", &messages_body("claude-opus-5"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["x-anthroxy-model"], "fast");
    assert_eq!(upstream.last().json()["model"], "mock-fast-v1");
}

#[tokio::test]
async fn count_tokens_is_routed_like_messages() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages/count_tokens", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(upstream.last().path_and_query, "/v1/messages/count_tokens");
}

#[tokio::test]
async fn query_string_is_preserved() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages?beta=true", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(upstream.last().path_and_query, "/v1/messages?beta=true");
}

#[tokio::test]
async fn malformed_body_is_400() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;

    let res = router
        .http
        .post(router.url("/v1/messages"))
        .header("x-api-key", TOKEN)
        .body("not json")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["type"], "invalid_request_error");

    let res = router
        .post("/v1/messages", &json!({"messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let body: Value = res.json().await.unwrap();
    assert!(body["error"]["message"].as_str().unwrap().contains("model"));

    // A JSON array also deserializes into the peek struct; the rewrite that
    // follows only handles an object.
    let res = router
        .post("/v1/messages", &json!(["fast", true]))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["type"], "invalid_request_error");
}

#[tokio::test]
async fn oversized_body_is_413() {
    let upstream = MockUpstream::start(echo).await;
    let config = config_with_backend(&upstream.url(), "")
        .replace("token = ", "max_body_bytes = 256\ntoken = ");
    let router = TestRouter::start(&config).await;
    let mut body = messages_body("fast");
    body["padding"] = Value::String("x".repeat(1024));
    let res = router.post("/v1/messages", &body).send().await.unwrap();
    assert_eq!(res.status(), 413);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["type"], "request_too_large");
}

#[tokio::test]
async fn unknown_path_is_404_in_anthropic_shape() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router.get("/v1/complete").send().await.unwrap();
    assert_eq!(res.status(), 404);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["type"], "not_found_error");
}

#[tokio::test]
async fn streams_sse_chunks_as_they_arrive() {
    let upstream = MockUpstream::start(|_| {
        let events = futures_util::stream::iter(0..3).then(|i| async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            Ok::<_, std::io::Error>(format!("event: ping\ndata: {{\"n\":{i}}}\n\n"))
        });
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .header("cache-control", "no-cache")
            .body(Body::from_stream(events))
            .unwrap()
    })
    .await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;

    let mut body = messages_body("fast");
    body["stream"] = Value::Bool(true);
    let started = Instant::now();
    let res = router.post("/v1/messages", &body).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/event-stream");

    let mut stream = res.bytes_stream();
    let mut arrivals = Vec::new();
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        arrivals.push(started.elapsed());
        text.push_str(std::str::from_utf8(&chunk.unwrap()).unwrap());
    }
    assert_eq!(text.matches("event: ping").count(), 3, "{text}");
    assert!(text.contains("data: {\"n\":2}"));
    let spread = *arrivals.last().unwrap() - arrivals[0];
    assert!(
        spread >= Duration::from_millis(200),
        "chunks were buffered: arrivals {arrivals:?}"
    );
}

#[tokio::test]
async fn retries_connection_failures_then_reports_backend() {
    // Nothing listens here.
    let config = config_with_backend(
        "http://127.0.0.1:1",
        "[upstream]\nretries = 2\nretry_backoff = \"10ms\"\n",
    );
    let router = TestRouter::start(&config).await;

    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 502);
    assert_eq!(res.headers()["x-anthroxy-backend"], "mock");
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "api_error");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("mock") && message.contains("3 attempt"),
        "{message}"
    );
    assert!(body["request_id"].is_string());
}

#[tokio::test]
async fn refreshes_command_credential_on_401_and_retries_once() {
    let upstream = MockUpstream::start(|received| {
        if received.header("authorization") == Some("Bearer tok2") {
            echo(received)
        } else {
            json_response(401, json!({"type":"error","error":{"type":"authentication_error","message":"bad token"}}))
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
    assert_eq!(res.status(), 200);
    let seen = upstream.received();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].header("authorization"), Some("Bearer tok1"));
    assert_eq!(seen[1].header("authorization"), Some("Bearer tok2"));

    // Second request reuses the cached tok2: exactly one more upstream call.
    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(upstream.received().len(), 3);
}

#[tokio::test]
async fn static_credential_is_not_retried_on_401() {
    let upstream = MockUpstream::start(|_| {
        json_response(
            401,
            json!({"type":"error","error":{"type":"authentication_error","message":"bad token"}}),
        )
    })
    .await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    assert_eq!(upstream.received().len(), 1);
}

#[tokio::test]
async fn upstream_error_body_is_annotated_with_backend() {
    let upstream = MockUpstream::start(|_| {
        json_response(
            429,
            json!({"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}),
        )
    })
    .await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 429);
    assert_eq!(res.headers()["x-anthroxy-backend"], "mock");
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["type"], "rate_limit_error");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("mock") && message.contains("429") && message.contains("slow down"),
        "{message}"
    );
    assert!(
        body["request_id"].is_string(),
        "router adds its request id when upstream has none"
    );
}

#[tokio::test]
async fn non_json_upstream_error_passes_through_verbatim() {
    let upstream = MockUpstream::start(|_| {
        Response::builder()
            .status(503)
            .header("content-type", "text/plain")
            .body(Body::from("backend down"))
            .unwrap()
    })
    .await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 503);
    assert_eq!(res.headers()["content-type"], "text/plain");
    assert_eq!(res.text().await.unwrap(), "backend down");
}

#[tokio::test]
async fn upstream_error_body_that_breaks_off_is_a_502() {
    // Headers and a first chunk go out, then the body breaks off.
    let upstream = MockUpstream::start(|_| {
        let chunks = futures_util::stream::iter(0..2).then(|i| async move {
            tokio::time::sleep(Duration::from_millis(50 * i)).await;
            if i == 0 {
                Ok("partial")
            } else {
                Err(std::io::Error::other("connection reset"))
            }
        });
        Response::builder()
            .status(500)
            .header("content-type", "text/plain")
            .body(Body::from_stream(chunks))
            .unwrap()
    })
    .await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 502, "the backend failed, not the client");
    assert_eq!(res.headers()["x-anthroxy-backend"], "mock");
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["type"], "api_error");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("mock") && message.contains("body"),
        "{message}"
    );
}

#[tokio::test]
async fn retries_configured_statuses() {
    let upstream = MockUpstream::start({
        let calls = std::sync::atomic::AtomicUsize::new(0);
        move |received| {
            if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                json_response(
                    503,
                    json!({"type":"error","error":{"type":"api_error","message":"warming up"}}),
                )
            } else {
                echo(received)
            }
        }
    })
    .await;
    let config = config_with_backend(
        &upstream.url(),
        "[upstream]\nretries = 1\nretry_backoff = \"5ms\"\nretry_on_status = [503]\n",
    );
    let router = TestRouter::start(&config).await;
    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(upstream.received().len(), 2);
}

#[tokio::test]
async fn custom_credential_header_reaches_the_backend() {
    let upstream = MockUpstream::start(echo).await;
    let config = config_with_backend(&upstream.url(), "").replace(
        r#"credential = { kind = "static", value = "backend-secret-key" }"#,
        r#"credential = { kind = "static", value = "backend-secret-key", header = { name = "api-key" } }"#,
    );
    let router = TestRouter::start(&config).await;

    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let seen = upstream.last();
    assert_eq!(seen.header("api-key"), Some("backend-secret-key"));
    assert_eq!(seen.header("authorization"), None);
    assert_eq!(seen.header("x-api-key"), None);
}
