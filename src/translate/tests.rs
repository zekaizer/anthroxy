use bytes::Bytes;
use futures_util::StreamExt;
use http::StatusCode;
use serde_json::{Value, json};

use super::*;
use crate::anthropic::{DecodeError, ErrorType};
use crate::openai::ResponseError;

fn chunk(delta: Value, finish: Option<&str>) -> String {
    let body = json!({"id": "chatcmpl-1", "model": "qwen", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]});
    format!("data: {body}\n\n")
}

async fn translate(chunks: Vec<Result<&'static str, std::io::Error>>) -> String {
    let inner = futures_util::stream::iter(chunks.into_iter().map(|c| c.map(Bytes::from)));
    let mut out = String::new();
    let mut translator = Translator::new(inner, "fallback", "mock");
    while let Some(item) = translator.next().await {
        out.push_str(std::str::from_utf8(&item.unwrap()).unwrap());
    }
    out
}

fn events(sse: &str) -> Vec<String> {
    sse.split("\n\n")
        .filter(|f| !f.is_empty())
        .map(|f| {
            f.lines()
                .next()
                .unwrap()
                .trim_start_matches("event: ")
                .to_owned()
        })
        .collect()
}

#[tokio::test]
async fn frames_split_across_chunks_come_out_as_anthropic_events() {
    let first = chunk(json!({"role": "assistant", "content": "Hel"}), None);
    let (head, tail) = first.split_at(20);
    let rest = chunk(json!({"content": "lo"}), Some("stop"));
    let chunks: Vec<&'static str> = vec![
        Box::leak(head.to_owned().into_boxed_str()),
        Box::leak(tail.to_owned().into_boxed_str()),
        Box::leak(rest.into_boxed_str()),
        "data: [DONE]\n\n",
    ];
    let out = translate(chunks.into_iter().map(Ok).collect()).await;
    assert_eq!(
        events(&out),
        [
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop"
        ]
    );
    assert!(
        out.contains(r#""text":"Hel"}"#) && out.contains(r#""text":"lo"}"#),
        "{out}"
    );
}

#[tokio::test]
async fn transport_failure_becomes_one_error_event_then_silence() {
    let first = Box::leak(chunk(json!({"content": "partial"}), None).into_boxed_str());
    let out = translate(vec![
        Ok(first),
        Err(std::io::Error::other("connection reset")),
        Ok("data: [DONE]\n\n"),
    ])
    .await;
    let names = events(&out);
    assert_eq!(names.last().map(String::as_str), Some("error"), "{out}");
    assert_eq!(names.iter().filter(|n| *n == "error").count(), 1);
    assert!(!out.contains("message_stop"));
    assert!(
        out.contains("[backend mock]") && out.contains("connection reset"),
        "{out}"
    );
}

#[tokio::test]
async fn malformed_frame_becomes_an_error_event() {
    let out = translate(vec![Ok("data: {not json\n\n"), Ok("data: [DONE]\n\n")]).await;
    assert_eq!(events(&out), ["error"], "{out}");
    assert!(out.contains("[backend mock]"), "{out}");
}

#[tokio::test]
async fn end_of_input_without_done_closes_the_message() {
    let first = Box::leak(chunk(json!({"content": "x"}), Some("stop")).into_boxed_str());
    let out = translate(vec![Ok(first)]).await;
    assert_eq!(
        events(&out).last().map(String::as_str),
        Some("message_stop"),
        "{out}"
    );
}

#[tokio::test]
async fn empty_input_is_an_error_event() {
    let out = translate(vec![]).await;
    assert_eq!(events(&out), ["error"], "{out}");
}

#[test]
fn request_round_trips_through_the_ir() {
    let body = json!({
        "model": "m", "max_tokens": 5, "stream": true,
        "system": [{"type": "text", "text": "sys"}],
        "messages": [{"role": "user", "content": "hi"}],
        "metadata": {"user_id": "u"}
    });
    let out: Value =
        serde_json::from_slice(&request(&serde_json::to_vec(&body).unwrap(), "m").unwrap())
            .unwrap();
    assert_eq!(
        out,
        json!({
            "model": "m",
            "messages": [{"role": "system", "content": "sys"}, {"role": "user", "content": "hi"}],
            "max_tokens": 5,
            "user": "u",
            "stream": true,
            "stream_options": {"include_usage": true}
        })
    );
    assert_eq!(request(b"[]", "m"), Err(DecodeError::NotAnObject));
}

#[test]
fn response_round_trips_through_the_ir() {
    let body = json!({"id": "chatcmpl-2", "choices": [{"message": {"role": "assistant", "content": "yo"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1, "completion_tokens": 2}});
    let out: Value =
        serde_json::from_slice(&response(&serde_json::to_vec(&body).unwrap(), "fallback").unwrap())
            .unwrap();
    assert_eq!(out["model"], "fallback");
    assert_eq!(out["content"], json!([{"type": "text", "text": "yo"}]));
    assert_eq!(out["stop_reason"], "end_turn");
    assert_eq!(out["usage"], json!({"input_tokens": 1, "output_tokens": 2}));
    let error = response(br#"{"error": {"message": "boom"}}"#, "m");
    assert!(
        matches!(error, Err(ResponseError::Backend(ref m)) if m == "boom"),
        "{error:?}"
    );
}

#[test]
fn upstream_errors_take_their_type_from_the_status() {
    let doc = upstream_error(
        StatusCode::UNAUTHORIZED,
        br#"{"error": {"message": "bad key", "type": "invalid_request_error"}}"#,
        "mock",
        "rtr_1",
    );
    assert_eq!(doc.error.kind, ErrorType::AuthenticationError);
    assert_eq!(doc.error.message, "[backend mock, HTTP 401] bad key");
    assert_eq!(doc.request_id.as_deref(), Some("rtr_1"));
    let html = upstream_error(
        StatusCode::SERVICE_UNAVAILABLE,
        b"<html>gateway</html>",
        "mock",
        "rtr_2",
    );
    assert_eq!(doc.kind, "error");
    assert_eq!(html.error.kind, ErrorType::ApiError);
    assert_eq!(
        html.error.message,
        "[backend mock, HTTP 503] <html>gateway</html>"
    );
}

#[tokio::test]
async fn a_transport_error_ends_the_stream_at_once() {
    let first = Box::leak(chunk(json!({"content": "x"}), None).into_boxed_str());
    let inner = futures_util::stream::iter(vec![
        Ok(Bytes::from(first as &str)),
        Err(std::io::Error::other("reset")),
        Err(std::io::Error::other("still broken")),
    ]);
    let mut translator = Translator::new(inner, "m", "mock");
    let mut items = 0;
    let mut out = String::new();
    while let Some(item) = translator.next().await {
        items += 1;
        out.push_str(std::str::from_utf8(&item.unwrap()).unwrap());
        assert!(items <= 2, "kept polling a failed stream");
    }
    assert_eq!(
        events(&out).last().map(String::as_str),
        Some("error"),
        "{out}"
    );
}

#[tokio::test]
async fn event_only_frames_are_keep_alives() {
    let first = Box::leak(chunk(json!({"content": "x"}), Some("stop")).into_boxed_str());
    let out = translate(vec![
        Ok(": comment\n\n"),
        Ok("event: ping\n\n"),
        Ok(first),
        Ok("event: ping\ndata: \n\n"),
        Ok("data: [DONE]\n\n"),
    ])
    .await;
    assert_eq!(
        events(&out).last().map(String::as_str),
        Some("message_stop"),
        "{out}"
    );
    assert!(!out.contains("error"), "{out}");
}

#[tokio::test]
async fn a_tool_call_without_a_name_is_an_error_not_a_silent_loss() {
    let first = Box::leak(
        chunk(
            json!({"tool_calls": [{"index": 0, "id": "c", "function": {"arguments": "{}"}}]}),
            Some("tool_calls"),
        )
        .into_boxed_str(),
    );
    let out = translate(vec![Ok(first), Ok("data: [DONE]\n\n")]).await;
    assert_eq!(
        events(&out).last().map(String::as_str),
        Some("error"),
        "{out}"
    );
}

// ---- stream path and document path agree ----

use crate::ir::{Event, StopReason, Usage};

/// Folds Anthropic SSE text back into the document shape `encode_message`
/// produces, so both paths can be compared field by field.
fn document_from_sse(sse: &str) -> Value {
    let mut blocks: Vec<Value> = Vec::new();
    let mut message = json!({});
    let mut partial: std::collections::BTreeMap<u64, String> = Default::default();
    for frame in sse.split("\n\n").filter(|f| !f.is_empty()) {
        let data: Value = serde_json::from_str(
            frame
                .lines()
                .nth(1)
                .unwrap()
                .strip_prefix("data: ")
                .unwrap(),
        )
        .unwrap();
        match data["type"].as_str().unwrap() {
            "message_start" => message = data["message"].clone(),
            "content_block_start" => {
                assert_eq!(
                    data["index"].as_u64().unwrap() as usize,
                    blocks.len(),
                    "indexes are dense"
                );
                blocks.push(data["content_block"].clone());
            }
            "content_block_delta" => {
                let index = data["index"].as_u64().unwrap();
                let block = &mut blocks[index as usize];
                match data["delta"]["type"].as_str().unwrap() {
                    "thinking_delta" => {
                        let text = block["thinking"].as_str().unwrap().to_owned()
                            + data["delta"]["thinking"].as_str().unwrap();
                        block["thinking"] = json!(text);
                    }
                    "text_delta" => {
                        let text = block["text"].as_str().unwrap().to_owned()
                            + data["delta"]["text"].as_str().unwrap();
                        block["text"] = json!(text);
                    }
                    "input_json_delta" => partial
                        .entry(index)
                        .or_default()
                        .push_str(data["delta"]["partial_json"].as_str().unwrap()),
                    other => panic!("{other}"),
                }
            }
            "content_block_stop" => {}
            "message_delta" => {
                message["stop_reason"] = data["delta"]["stop_reason"].clone();
                message["usage"] = data["usage"].clone();
            }
            "message_stop" => {}
            other => panic!("unexpected {other} in {sse}"),
        }
    }
    for (index, json) in partial {
        blocks[index as usize]["input"] = serde_json::from_str(&json).unwrap_or(json!({}));
    }
    message["content"] = Value::Array(blocks);
    message
}

#[test]
fn streaming_and_document_paths_produce_the_same_message() {
    let cases: Vec<Vec<Event>> = vec![
        vec![
            Event::Start {
                id: "a".into(),
                model: "m".into(),
            },
            Event::ThinkingDelta("t1".into()),
            Event::ThinkingDelta("t2".into()),
            Event::TextDelta("x".into()),
            Event::ToolCallStart {
                index: 0,
                id: "c0".into(),
                name: "read".into(),
            },
            Event::ToolCallDelta {
                index: 0,
                arguments: "{\"p\":".into(),
            },
            Event::ToolCallStart {
                index: 1,
                id: "c1".into(),
                name: "bash".into(),
            },
            Event::ToolCallDelta {
                index: 1,
                arguments: "{}".into(),
            },
            Event::ToolCallDelta {
                index: 0,
                arguments: "1}".into(),
            },
            Event::Finish(StopReason::ToolUse),
            Event::Usage(Usage {
                input_tokens: 3,
                output_tokens: 4,
                cache_read_tokens: 0,
                thinking_tokens: 0,
            }),
            Event::Done,
        ],
        vec![
            Event::Start {
                id: "b".into(),
                model: "m".into(),
            },
            Event::TextDelta("a".into()),
            Event::ToolCallStart {
                index: 0,
                id: "c".into(),
                name: "t".into(),
            },
            Event::TextDelta("b".into()),
            Event::ThinkingDelta("late".into()),
            Event::Finish(StopReason::EndTurn),
            Event::Done,
        ],
        vec![
            Event::Start {
                id: "c".into(),
                model: "m".into(),
            },
            Event::TextDelta("only text".into()),
            Event::Finish(StopReason::MaxTokens),
            Event::Done,
        ],
        vec![
            Event::Start {
                id: "d".into(),
                model: "m".into(),
            },
            Event::ToolCallStart {
                index: 0,
                id: "c".into(),
                name: "t".into(),
            },
            Event::Done,
        ],
    ];
    for events in cases {
        let mut sse = String::new();
        let mut encoder = crate::anthropic::StreamEncoder::new("fallback");
        for event in events.clone() {
            encoder.encode(event, &mut sse);
        }
        let streamed = document_from_sse(&sse);
        let folded: Value = serde_json::from_slice(&crate::anthropic::encode_message(
            &crate::ir::Message::from_events(events).unwrap(),
            "fallback",
        ))
        .unwrap();
        assert_eq!(streamed, folded, "{sse}");
    }
}
