//! A `kind = "openai"` backend: Messages in, Chat Completions out, and back.

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::response::Response;
use futures_util::StreamExt;
use serde_json::{Value, json};

use support::mock_upstream::{echo, json_response};
use support::openai::{
    chunk, completion, config_with_openai_backend, events, frame, sse_response, usage_and_done,
};
use support::router::{config_with_backend, messages_body};
use support::{MockUpstream, TestRouter};

fn claude_code_request(stream: bool) -> Value {
    json!({
        "model": "qwen",
        "max_tokens": 4096,
        "stream": stream,
        "temperature": 1.0,
        "top_k": 40,
        "stop_sequences": ["END"],
        "system": [
            {"type": "text", "text": "You are Claude Code.", "cache_control": {"type": "ephemeral"}},
            {"type": "text", "text": "Be brief."}
        ],
        "messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "read a.rs"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}
            ]},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "plan", "signature": "sig"},
                {"type": "text", "text": "Reading"},
                {"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {"path": "a.rs"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": [{"type": "text", "text": "fn main() {}"}]}
            ]},
            {"role": "system", "content": [{"type": "text", "text": "# Environment\ncwd changed", "cache_control": {"type": "ephemeral"}}]}
        ],
        "output_config": {"effort": "high"},
        "tools": [{"name": "Read", "description": "Read a file", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}}],
        "tool_choice": {"type": "auto"},
        "metadata": {"user_id": "u1"},
        "thinking": {"type": "enabled", "budget_tokens": 1024},
        "context_management": {"edits": []}
    })
}

#[tokio::test]
async fn translates_messages_request_to_chat_completions() {
    let upstream =
        MockUpstream::start(|_| completion(json!({"role": "assistant", "content": "ok"}), "stop"))
            .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;

    let res = router
        .post("/v1/messages?beta=true", &claude_code_request(false))
        .header("anthropic-beta", "context-management-2025-06-27")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());

    let received = upstream.last();
    assert_eq!(received.path_and_query, "/v1/chat/completions");
    assert_eq!(
        received.header("authorization"),
        Some("Bearer backend-secret-key")
    );
    assert_eq!(received.header("anthropic-version"), None);
    assert_eq!(received.header("anthropic-beta"), None);
    assert_eq!(
        received.json(),
        json!({
            "model": "qwen-32b",
            "messages": [
                {"role": "system", "content": "You are Claude Code.\n\nBe brief."},
                {"role": "user", "content": [
                    {"type": "text", "text": "read a.rs"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}
                ]},
                {"role": "assistant", "content": "Reading", "tool_calls": [
                    {"id": "toolu_1", "type": "function", "function": {"name": "Read", "arguments": "{\"path\":\"a.rs\"}"}}
                ]},
                {"role": "tool", "tool_call_id": "toolu_1", "content": "fn main() {}"},
                {"role": "system", "content": "# Environment\ncwd changed"}
            ],
            "tools": [{"type": "function", "function": {"name": "Read", "description": "Read a file", "parameters": {"type": "object", "properties": {"path": {"type": "string"}}}}}],
            "tool_choice": "auto",
            "max_tokens": 4096,
            "temperature": 1.0,
            "stop": ["END"],
            "stream": false
        })
    );
}

#[tokio::test]
async fn drop_fields_apply_before_translation() {
    let upstream =
        MockUpstream::start(|_| completion(json!({"role": "assistant", "content": "ok"}), "stop"))
            .await;
    let config = config_with_openai_backend(&upstream.url(), "").replace(
        "kind = \"openai\"",
        "kind = \"openai\"\ndrop_fields = [\"temperature\"]",
    );
    let router = TestRouter::start(&config).await;
    let res = router
        .post("/v1/messages", &claude_code_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(upstream.last().json().get("temperature").is_none());
}

#[tokio::test]
async fn streams_reasoning_text_and_tool_calls_as_anthropic_events() {
    let upstream = MockUpstream::start(|_| {
        let mut frames = vec![
            chunk(json!({"role": "assistant", "content": ""}), None),
            chunk(json!({"reasoning_content": "plan "}), None),
            chunk(json!({"reasoning_content": "it"}), None),
            chunk(json!({"content": "Read"}), None),
            chunk(json!({"content": "ing"}), None),
            chunk(json!({"tool_calls": [{"index": 0, "id": "call_a", "type": "function", "function": {"name": "Read", "arguments": ""}}]}), None),
            chunk(json!({"tool_calls": [{"index": 0, "function": {"arguments": "{\"path\":"}}]}), None),
            chunk(json!({"tool_calls": [{"index": 0, "function": {"arguments": "\"a.rs\"}"}}]}), None),
            chunk(json!({}), Some("tool_calls")),
        ];
        frames.extend(usage_and_done(12, 34));
        sse_response(frames, Duration::from_millis(1))
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;

    let res = router
        .post("/v1/messages", &claude_code_request(true))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    assert_eq!(res.headers()["x-anthroxy-backend"], "mock");
    let text = res.text().await.unwrap();
    assert_eq!(
        events(&text),
        vec![
            (
                "message_start".to_owned(),
                json!({"type": "message_start", "message": {"id": "chatcmpl-1", "type": "message", "role": "assistant", "model": "qwen-32b", "content": [], "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 0, "output_tokens": 0}}})
            ),
            (
                "content_block_start".to_owned(),
                json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": ""}})
            ),
            (
                "content_block_delta".to_owned(),
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "plan "}})
            ),
            (
                "content_block_delta".to_owned(),
                json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "it"}})
            ),
            (
                "content_block_stop".to_owned(),
                json!({"type": "content_block_stop", "index": 0})
            ),
            (
                "content_block_start".to_owned(),
                json!({"type": "content_block_start", "index": 1, "content_block": {"type": "text", "text": ""}})
            ),
            (
                "content_block_delta".to_owned(),
                json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "Read"}})
            ),
            (
                "content_block_delta".to_owned(),
                json!({"type": "content_block_delta", "index": 1, "delta": {"type": "text_delta", "text": "ing"}})
            ),
            (
                "content_block_stop".to_owned(),
                json!({"type": "content_block_stop", "index": 1})
            ),
            (
                "content_block_start".to_owned(),
                json!({"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "call_a", "name": "Read", "input": {}}})
            ),
            (
                "content_block_delta".to_owned(),
                json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"path\":"}})
            ),
            (
                "content_block_delta".to_owned(),
                json!({"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "\"a.rs\"}"}})
            ),
            (
                "content_block_stop".to_owned(),
                json!({"type": "content_block_stop", "index": 2})
            ),
            (
                "message_delta".to_owned(),
                json!({"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {"input_tokens": 12, "output_tokens": 34}})
            ),
            ("message_stop".to_owned(), json!({"type": "message_stop"})),
        ]
    );
    let sent = upstream.last().json();
    assert_eq!(sent["stream"], json!(true));
    assert_eq!(sent["stream_options"], json!({"include_usage": true}));
}

#[tokio::test]
async fn parallel_tool_calls_become_separate_blocks() {
    let upstream = MockUpstream::start(|_| {
        let mut frames = vec![
            chunk(json!({"role": "assistant"}), None),
            chunk(json!({"tool_calls": [
                {"index": 0, "id": "call_a", "function": {"name": "Read", "arguments": "{\"pa"}},
                {"index": 1, "id": "call_b", "function": {"name": "Bash", "arguments": "{\"cmd\":"}}
            ]}), None),
            chunk(json!({"tool_calls": [{"index": 1, "function": {"arguments": "\"ls\"}"}}]}), None),
            chunk(json!({"tool_calls": [{"index": 0, "function": {"arguments": "th\":\"a\"}"}}]}), None),
            chunk(json!({}), Some("tool_calls")),
        ];
        frames.extend(usage_and_done(1, 2));
        sse_response(frames, Duration::from_millis(1))
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let text = router
        .post("/v1/messages", &claude_code_request(true))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let mut arguments = std::collections::BTreeMap::new();
    let mut names = std::collections::BTreeMap::new();
    for (event, data) in events(&text) {
        match event.as_str() {
            "content_block_start" => {
                names.insert(
                    data["index"].as_u64().unwrap(),
                    data["content_block"]["id"].as_str().unwrap().to_owned(),
                );
            }
            "content_block_delta" if data["delta"]["type"] == "input_json_delta" => {
                arguments
                    .entry(data["index"].as_u64().unwrap())
                    .or_insert_with(String::new)
                    .push_str(data["delta"]["partial_json"].as_str().unwrap());
            }
            _ => {}
        }
    }
    assert_eq!(names[&0], "call_a");
    assert_eq!(names[&1], "call_b");
    assert_eq!(arguments[&0], "{\"path\":\"a\"}");
    assert_eq!(arguments[&1], "{\"cmd\":\"ls\"}");
}

#[tokio::test]
async fn finish_reasons_map_to_stop_reasons() {
    for (finish, expected) in [
        ("stop", "end_turn"),
        ("length", "max_tokens"),
        ("tool_calls", "tool_use"),
        ("content_filter", "end_turn"),
    ] {
        let upstream = MockUpstream::start(move |_| {
            let mut frames = vec![chunk(
                json!({"role": "assistant", "content": "x"}),
                Some(finish),
            )];
            frames.extend(usage_and_done(1, 1));
            sse_response(frames, Duration::ZERO)
        })
        .await;
        let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
        let text = router
            .post("/v1/messages", &claude_code_request(true))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        let (_, delta) = events(&text)
            .into_iter()
            .find(|(e, _)| e == "message_delta")
            .unwrap_or_else(|| panic!("{finish}: {text}"));
        assert_eq!(delta["delta"]["stop_reason"], expected, "{finish}");
    }
}

#[tokio::test]
async fn mid_stream_drop_yields_error_event() {
    let upstream = MockUpstream::start(|_| {
        let chunks = futures_util::stream::iter(0..3).then(|i| async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            match i {
                0 => Ok(chunk(
                    json!({"role": "assistant", "content": "partial"}),
                    None,
                )),
                1 => Ok(chunk(json!({"content": " text"}), None)),
                _ => Err(std::io::Error::other("connection reset")),
            }
        });
        Response::builder()
            .status(200)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(chunks))
            .unwrap()
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &claude_code_request(true))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let mut stream = res.bytes_stream();
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => text.push_str(std::str::from_utf8(&bytes).unwrap()),
            Err(_) => break,
        }
    }
    let all = events(&text);
    let (last, data) = all.last().unwrap();
    assert_eq!(last, "error", "{text}");
    assert_eq!(data["error"]["type"], "api_error");
    let message = data["error"]["message"].as_str().unwrap();
    assert!(message.starts_with("[backend mock]"), "{message}");
    assert!(all.iter().filter(|(e, _)| e == "error").count() == 1);
    assert!(
        all.iter()
            .any(|(e, d)| e == "content_block_delta" && d["delta"]["text"] == "partial")
    );
}

#[tokio::test]
async fn error_chunk_mid_stream_yields_error_event() {
    let upstream = MockUpstream::start(|_| {
        sse_response(
            vec![
                chunk(json!({"role": "assistant", "content": "hi"}), None),
                frame(&json!({"error": {"message": "overloaded", "type": "server_error"}})),
                chunk(json!({"content": "ignored"}), Some("stop")),
                "data: [DONE]\n\n".to_owned(),
            ],
            Duration::ZERO,
        )
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let text = router
        .post("/v1/messages", &claude_code_request(true))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let all = events(&text);
    let (last, data) = all.last().unwrap();
    assert_eq!(last, "error", "{text}");
    assert_eq!(data["error"]["message"], "overloaded");
    assert!(
        !text.contains("ignored") && !text.contains("message_stop"),
        "{text}"
    );
}

#[tokio::test]
async fn upstream_content_type_variants_are_normalized() {
    let upstream = MockUpstream::start(|_| {
        let mut frames = vec![chunk(
            json!({"role": "assistant", "content": "x"}),
            Some("stop"),
        )];
        frames.extend(usage_and_done(1, 1));
        let mut response = sse_response(frames, Duration::ZERO);
        response.headers_mut().insert(
            "content-type",
            "text/event-stream; charset=utf-8".parse().unwrap(),
        );
        response
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &claude_code_request(true))
        .send()
        .await
        .unwrap();
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    assert_eq!(res.headers()["cache-control"], "no-cache");
    assert_eq!(res.headers().get_all("content-type").iter().count(), 1);
}

#[tokio::test]
async fn upstream_is_drained_after_an_error_event() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = MockUpstream::start(|_| {
        sse_response(
            vec![
                chunk(json!({"role": "assistant", "content": "hi"}), None),
                frame(&json!({"error": {"message": "overloaded"}})),
                chunk(json!({"content": "after-the-error"}), Some("stop")),
                "data: [DONE]\n\n".to_owned(),
            ],
            Duration::from_millis(10),
        )
    })
    .await;
    let extra = format!("[logging]\nbody_dir = \"{}\"\n", dir.path().display());
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), &extra)).await;
    let text = router
        .post("/v1/messages", &claude_code_request(true))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!text.contains("after-the-error"), "{text}");

    let mut entry = None;
    for _ in 0..100 {
        let found = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| {
                std::fs::read(p.join("meta.json"))
                    .map(|m| String::from_utf8_lossy(&m).contains("outcome"))
                    .unwrap_or(false)
            });
        if found.is_some() {
            entry = found;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let entry = entry.expect("a finished body log entry");
    let meta: Value =
        serde_json::from_slice(&std::fs::read(entry.join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["outcome"], "complete", "upstream was read to the end");
    let recorded = std::fs::read_to_string(entry.join("response.sse")).unwrap();
    assert!(
        recorded.contains("after-the-error"),
        "the backend's bytes are recorded in full"
    );
}

#[tokio::test]
async fn openai_error_becomes_anthropic_error() {
    let upstream = MockUpstream::start(|received| {
        if received.header("authorization") == Some("Bearer backend-secret-key") {
            json_response(401, json!({"error": {"message": "bad key", "type": "invalid_request_error", "param": null, "code": "invalid_api_key"}}))
        } else {
            json_response(500, json!({}))
        }
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &claude_code_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 401);
    assert_eq!(res.headers()["content-type"], "application/json");
    assert_eq!(res.headers()["x-anthroxy-backend"], "mock");
    let request_id = res.headers()["x-request-id"].to_str().unwrap().to_owned();
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "authentication_error");
    assert_eq!(body["error"]["message"], "[backend mock, HTTP 401] bad key");
    assert_eq!(body["request_id"], request_id);

    let upstream = MockUpstream::start(|_| {
        Response::builder()
            .status(503)
            .header("content-type", "text/html")
            .body(Body::from("<html>gateway down</html>"))
            .unwrap()
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &claude_code_request(true))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 503);
    assert_eq!(
        res.headers()["content-type"],
        "application/json",
        "the router's document replaces the backend's content-type"
    );
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["type"], "api_error");
    assert_eq!(
        body["error"]["message"],
        "[backend mock, HTTP 503] <html>gateway down</html>"
    );
}

#[tokio::test]
async fn non_stream_response_becomes_message_document() {
    let upstream = MockUpstream::start(|_| {
        completion(
            json!({
                "role": "assistant",
                "reasoning_content": "plan",
                "content": "Reading",
                "tool_calls": [{"id": "call_a", "type": "function", "function": {"name": "Read", "arguments": "{\"path\":\"a\"}"}}]
            }),
            "tool_calls",
        )
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &claude_code_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "application/json");
    let body: Value = res.json().await.unwrap();
    assert_eq!(
        body,
        json!({
            "id": "chatcmpl-9",
            "type": "message",
            "role": "assistant",
            "model": "qwen-32b",
            "content": [
                {"type": "thinking", "thinking": "plan"},
                {"type": "text", "text": "Reading"},
                {"type": "tool_use", "id": "call_a", "name": "Read", "input": {"path": "a"}}
            ],
            "stop_reason": "tool_use",
            "stop_sequence": null,
            "usage": {"input_tokens": 5, "output_tokens": 7}
        })
    );

    let upstream = MockUpstream::start(|_| json_response(200, json!({"unexpected": true}))).await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &claude_code_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 502);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["type"], "api_error");
    assert!(
        body["error"]["message"].as_str().unwrap().contains("mock"),
        "{body}"
    );
}

#[tokio::test]
async fn events_are_not_buffered() {
    let upstream = MockUpstream::start(|_| {
        let mut frames = vec![
            chunk(json!({"role": "assistant", "content": "a"}), None),
            chunk(json!({"content": "b"}), None),
            chunk(json!({"content": "c"}), Some("stop")),
        ];
        frames.extend(usage_and_done(1, 3));
        sse_response(frames, Duration::from_millis(150))
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let started = Instant::now();
    let res = router
        .post("/v1/messages", &claude_code_request(true))
        .send()
        .await
        .unwrap();
    let mut stream = res.bytes_stream();
    let mut arrivals = Vec::new();
    while let Some(chunk) = stream.next().await {
        chunk.unwrap();
        arrivals.push(started.elapsed());
    }
    assert!(arrivals.len() >= 3, "{arrivals:?}");
    let spread = *arrivals.last().unwrap() - arrivals[0];
    assert!(
        spread >= Duration::from_millis(250),
        "chunks were buffered: {arrivals:?}"
    );
}

#[tokio::test]
async fn count_tokens_on_openai_backend_is_404() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let upstream = MockUpstream::start(move |received| {
        seen.fetch_add(1, Ordering::SeqCst);
        echo(received)
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages/count_tokens", &messages_body("qwen"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    assert_eq!(res.headers()["x-anthroxy-backend"], "mock");
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["type"], "not_found_error");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("count_tokens"),
        "{body}"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "the backend is never asked"
    );
}

#[tokio::test]
async fn untranslatable_requests_are_400_naming_the_block() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let mut body = messages_body("qwen");
    body["messages"] = json!([{"role": "user", "content": [
        {"type": "server_tool_use", "id": "x", "name": "web_search", "input": {}}
    ]}]);
    let res = router.post("/v1/messages", &body).send().await.unwrap();
    assert_eq!(res.status(), 400);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["error"]["type"], "invalid_request_error");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("server_tool_use") && message.contains("mock"),
        "{message}"
    );
}

#[tokio::test]
async fn body_log_records_the_openai_request() {
    let dir = tempfile::tempdir().unwrap();
    let upstream =
        MockUpstream::start(|_| completion(json!({"role": "assistant", "content": "ok"}), "stop"))
            .await;
    let extra = format!("[logging]\nbody_dir = \"{}\"\n", dir.path().display());
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), &extra)).await;
    let res = router
        .post("/v1/messages", &claude_code_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    let mut entry = None;
    for _ in 0..100 {
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.join("meta.json").exists() && p.join("response.json").exists())
            .collect();
        if let Some(found) = entries.into_iter().next() {
            entry = Some(found);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let entry = entry.expect("a finished body log entry");
    let request = std::fs::read(entry.join("request.json")).unwrap();
    assert_eq!(
        request,
        upstream.last().body.to_vec(),
        "request.json is what went upstream"
    );
    let meta: Value =
        serde_json::from_slice(&std::fs::read(entry.join("meta.json")).unwrap()).unwrap();
    assert_eq!(meta["path"], "/v1/chat/completions");
    assert_eq!(meta["backend"], "mock");
    assert_eq!(meta["outcome"], "complete");
    let response: Value =
        serde_json::from_slice(&std::fs::read(entry.join("response.json")).unwrap()).unwrap();
    assert_eq!(
        response["object"], "chat.completion",
        "response is the backend's own document"
    );
}

#[tokio::test]
async fn client_disconnect_mid_stream_leaves_the_router_healthy() {
    let upstream = MockUpstream::start(|_| {
        let mut frames = vec![chunk(json!({"role": "assistant", "content": "a"}), None)];
        frames.extend((0..20).map(|i| chunk(json!({"content": format!("{i}")}), None)));
        frames.extend(usage_and_done(1, 21));
        sse_response(frames, Duration::from_millis(50))
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &claude_code_request(true))
        .send()
        .await
        .unwrap();
    let mut stream = res.bytes_stream();
    stream.next().await.unwrap().unwrap();
    drop(stream);

    let res = router
        .post("/v1/messages", &claude_code_request(true))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let text = res.text().await.unwrap();
    assert_eq!(
        events(&text).last().map(|(e, _)| e.as_str()),
        Some("message_stop"),
        "{text}"
    );
    let health: Value = router
        .get("/healthz")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["status"], "ok");
}

#[tokio::test]
async fn a_document_answer_to_a_streaming_request_is_still_translated() {
    let upstream = MockUpstream::start(|_| {
        completion(
            json!({"role": "assistant", "content": "no stream here"}),
            "stop",
        )
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &claude_code_request(true))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(
        res.headers()["content-type"],
        "text/event-stream",
        "the client asked for events and can only read events"
    );
    let text = res.text().await.unwrap();
    let all = events(&text);
    assert_eq!(
        all.first().map(|(e, _)| e.as_str()),
        Some("message_start"),
        "{text}"
    );
    assert_eq!(
        all.last().map(|(e, _)| e.as_str()),
        Some("message_stop"),
        "{text}"
    );
    assert!(
        all.iter()
            .any(|(e, d)| e == "content_block_delta" && d["delta"]["text"] == "no stream here"),
        "{text}"
    );
    let (_, delta) = all.iter().find(|(e, _)| e == "message_delta").unwrap();
    assert_eq!(
        delta["usage"],
        json!({"input_tokens": 5, "output_tokens": 7})
    );
}

#[tokio::test]
async fn body_log_marks_a_client_that_left_before_a_buffered_answer() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = MockUpstream::start(|_| {
        // A backend that takes longer than the client is willing to wait.
        let slow = futures_util::stream::once(async {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            Ok::<_, std::io::Error>(json!({"choices": [{"message": {"role": "assistant", "content": "late"}, "finish_reason": "stop"}]}).to_string())
        });
        Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(Body::from_stream(slow))
            .unwrap()
    })
    .await;
    let extra = format!("[logging]\nbody_dir = \"{}\"\n", dir.path().display());
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), &extra)).await;
    let gone = router
        .post("/v1/messages", &claude_code_request(false))
        .timeout(Duration::from_millis(300))
        .send()
        .await;
    assert!(gone.is_err(), "the client gave up first");

    let mut outcome = None;
    for _ in 0..150 {
        if let Some(found) = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find_map(|p| {
                let meta: Value =
                    serde_json::from_slice(&std::fs::read(p.join("meta.json")).ok()?).ok()?;
                meta.get("outcome").map(|o| o.as_str().unwrap().to_owned())
            })
        {
            outcome = Some(found);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(outcome.as_deref(), Some("client_disconnected"));
}

#[tokio::test]
async fn an_image_read_by_a_tool_reaches_the_model() {
    let upstream = MockUpstream::start(|_| {
        completion(
            json!({"role": "assistant", "content": "a blue square"}),
            "stop",
        )
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let mut body = messages_body("qwen");
    body["messages"] = json!([
        {"role": "user", "content": "read shape.png"},
        {"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {"file_path": "shape.png"}}]},
        {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}
        ]}]}
    ]);
    let res = router.post("/v1/messages", &body).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let sent = upstream.last().json();
    let messages = sent["messages"].as_array().unwrap();
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(messages[2]["tool_call_id"], "toolu_1");
    assert!(messages[2]["content"].as_str().unwrap().contains("image"));
    assert_eq!(messages[3]["role"], "user");
    assert_eq!(
        messages[3]["content"][1]["image_url"]["url"],
        "data:image/png;base64,AAAA"
    );
}

#[tokio::test]
async fn a_pdf_read_by_a_tool_becomes_a_note_instead_of_killing_the_turn() {
    let upstream = MockUpstream::start(|_| {
        completion(
            json!({"role": "assistant", "content": "I cannot read PDFs here"}),
            "stop",
        )
    })
    .await;
    let router = TestRouter::start(&config_with_openai_backend(&upstream.url(), "")).await;
    let mut body = messages_body("qwen");
    body["messages"] = json!([
        {"role": "user", "content": "read secret.pdf"},
        {"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {"file_path": "secret.pdf"}}]},
        {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "toolu_1", "content": [
            {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBERi0xLjQK"}}
        ]}]}
    ]);
    let res = router.post("/v1/messages", &body).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let sent = upstream.last().json();
    let tool = &sent["messages"][2];
    assert_eq!(tool["role"], "tool");
    let note = tool["content"].as_str().unwrap();
    assert!(
        note.contains("application/pdf") && note.contains("omitted"),
        "{note}"
    );
}

#[tokio::test]
async fn anthropic_kind_never_translates() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.unwrap();
    assert_eq!(body["echo"]["path"], "/v1/messages");
    assert_eq!(
        body["echo"]["body"]["future_field"],
        json!({"nested": [1, 2, 3]})
    );
}
