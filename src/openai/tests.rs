use serde_json::{Value, json};

use super::*;
use crate::ir::{Part, Request, RequestMessage, Role, Tool, ToolChoice};

fn request(messages: Vec<RequestMessage>) -> Request {
    Request {
        model: "m".into(),
        system: None,
        messages,
        tools: vec![],
        tool_choice: None,
        max_tokens: Some(100),
        temperature: None,
        top_p: None,
        stop: vec![],
        stream: false,
    }
}

fn user(parts: Vec<Part>) -> RequestMessage {
    RequestMessage {
        role: Role::User,
        parts,
    }
}

fn assistant(parts: Vec<Part>) -> RequestMessage {
    RequestMessage {
        role: Role::Assistant,
        parts,
    }
}

fn encoded(request: &Request) -> Value {
    serde_json::from_slice(&encode_request(request)).unwrap()
}

#[test]
fn text_only_messages_are_plain_strings() {
    let body = encoded(&request(vec![user(vec![Part::Text("hi".into())])]));
    assert_eq!(
        body,
        json!({"model": "m", "messages": [{"role": "user", "content": "hi"}], "max_tokens": 100, "stream": false})
    );
}

#[test]
fn system_becomes_the_first_message() {
    let mut r = request(vec![user(vec![Part::Text("hi".into())])]);
    r.system = Some("be brief".into());
    assert_eq!(
        encoded(&r)["messages"],
        json!([{"role": "system", "content": "be brief"}, {"role": "user", "content": "hi"}])
    );
}

#[test]
fn tool_loop_history() {
    let r = request(vec![
        user(vec![Part::Text("read a.rs".into())]),
        assistant(vec![
            Part::Text("Reading".into()),
            Part::ToolUse {
                id: "toolu_1".into(),
                name: "read".into(),
                input: json!({"path": "a.rs"}),
            },
            Part::ToolUse {
                id: "toolu_2".into(),
                name: "bash".into(),
                input: json!({"cmd": "ls"}),
            },
        ]),
        user(vec![
            Part::ToolResult {
                tool_use_id: "toolu_1".into(),
                content: "fn main() {}".into(),
            },
            Part::ToolResult {
                tool_use_id: "toolu_2".into(),
                content: "a.rs".into(),
            },
            Part::Text("thanks".into()),
        ]),
        assistant(vec![Part::ToolUse {
            id: "toolu_3".into(),
            name: "read".into(),
            input: json!({}),
        }]),
    ]);
    assert_eq!(
        encoded(&r)["messages"],
        json!([
            {"role": "user", "content": "read a.rs"},
            {"role": "assistant", "content": "Reading", "tool_calls": [
                {"id": "toolu_1", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"a.rs\"}"}},
                {"id": "toolu_2", "type": "function", "function": {"name": "bash", "arguments": "{\"cmd\":\"ls\"}"}}
            ]},
            {"role": "tool", "tool_call_id": "toolu_1", "content": "fn main() {}"},
            {"role": "tool", "tool_call_id": "toolu_2", "content": "a.rs"},
            {"role": "user", "content": "thanks"},
            {"role": "assistant", "tool_calls": [
                {"id": "toolu_3", "type": "function", "function": {"name": "read", "arguments": "{}"}}
            ]}
        ])
    );
}

#[test]
fn images_make_a_parts_array() {
    let r = request(vec![user(vec![
        Part::Text("what is this".into()),
        Part::Image {
            media_type: "image/png".into(),
            data: "AAAA".into(),
        },
    ])]);
    assert_eq!(
        encoded(&r)["messages"],
        json!([{"role": "user", "content": [
            {"type": "text", "text": "what is this"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,AAAA"}}
        ]}])
    );
}

#[test]
fn several_text_parts_are_joined() {
    let r = request(vec![assistant(vec![
        Part::Text("a".into()),
        Part::Text("b".into()),
    ])]);
    assert_eq!(
        encoded(&r)["messages"],
        json!([{"role": "assistant", "content": "a\n\nb"}])
    );
}

#[test]
fn tools_tool_choice_and_sampling() {
    let mut r = request(vec![user(vec![Part::Text("hi".into())])]);
    r.tools = vec![
        Tool {
            name: "read".into(),
            description: Some("Read".into()),
            parameters: json!({"type": "object"}),
        },
        Tool {
            name: "bare".into(),
            description: None,
            parameters: json!({"type": "object"}),
        },
    ];
    r.temperature = Some(0.5);
    r.top_p = Some(0.9);
    r.stop = vec!["END".into()];
    r.max_tokens = None;
    let with = |choice: Option<ToolChoice>| {
        let mut r = r.clone();
        r.tool_choice = choice;
        encoded(&r)
    };
    let body = with(Some(ToolChoice::Auto));
    assert_eq!(
        body["tools"],
        json!([
            {"type": "function", "function": {"name": "read", "description": "Read", "parameters": {"type": "object"}}},
            {"type": "function", "function": {"name": "bare", "parameters": {"type": "object"}}}
        ])
    );
    assert_eq!(body["tool_choice"], json!("auto"));
    assert_eq!(body["temperature"], json!(0.5));
    assert_eq!(body["top_p"], json!(0.9));
    assert_eq!(body["stop"], json!(["END"]));
    assert!(body.get("max_tokens").is_none());
    assert_eq!(
        with(Some(ToolChoice::Required))["tool_choice"],
        json!("required")
    );
    assert_eq!(with(Some(ToolChoice::None))["tool_choice"], json!("none"));
    assert_eq!(
        with(Some(ToolChoice::Tool("read".into())))["tool_choice"],
        json!({"type": "function", "function": {"name": "read"}})
    );
    assert!(with(None).get("tool_choice").is_none());
}

#[test]
fn streaming_asks_for_usage() {
    let mut r = request(vec![user(vec![Part::Text("hi".into())])]);
    r.stream = true;
    let body = encoded(&r);
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
}

// ---- chunks ----

use crate::ir::{Event, StopReason, Usage};

fn chunk(delta: Value, finish: Option<&str>) -> String {
    json!({
        "id": "chatcmpl-1", "object": "chat.completion.chunk", "created": 1, "model": "qwen",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]
    })
    .to_string()
}

fn start() -> Event {
    Event::Start {
        id: "chatcmpl-1".into(),
        model: "qwen".into(),
    }
}

#[test]
fn first_chunk_starts_the_message_once() {
    let mut d = ChunkDecoder::new();
    assert_eq!(
        d.decode(&chunk(json!({"role": "assistant", "content": ""}), None))
            .unwrap(),
        vec![start()]
    );
    assert_eq!(
        d.decode(&chunk(json!({"content": "Hi"}), None)).unwrap(),
        vec![Event::TextDelta("Hi".into())]
    );
}

#[test]
fn reasoning_fields_and_content_parts() {
    let mut d = ChunkDecoder::new();
    assert_eq!(
        d.decode(&chunk(json!({"reasoning_content": "think"}), None))
            .unwrap(),
        vec![start(), Event::ThinkingDelta("think".into())]
    );
    assert_eq!(
        d.decode(&chunk(json!({"reasoning": " more"}), None))
            .unwrap(),
        vec![Event::ThinkingDelta(" more".into())]
    );
    assert_eq!(
        d.decode(&chunk(
            json!({"content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]}),
            None
        ))
        .unwrap(),
        vec![Event::TextDelta("a".into()), Event::TextDelta("b".into())]
    );
    assert_eq!(
        d.decode(&chunk(
            json!({"content": null, "reasoning_content": null}),
            None
        ))
        .unwrap(),
        vec![]
    );
}

#[test]
fn tool_calls_stream_by_index() {
    let mut d = ChunkDecoder::new();
    assert_eq!(
        d.decode(&chunk(
            json!({"tool_calls": [{"index": 0, "id": "call_a", "type": "function", "function": {"name": "read", "arguments": ""}}]}),
            None
        ))
        .unwrap(),
        vec![
            start(),
            Event::ToolCallStart {
                index: 0,
                id: "call_a".into(),
                name: "read".into()
            }
        ]
    );
    assert_eq!(
        d.decode(&chunk(
            json!({"tool_calls": [
                {"index": 0, "function": {"arguments": "{\"path\""}},
                {"index": 1, "id": "call_b", "function": {"name": "bash", "arguments": "{\"cmd\":"}}
            ]}),
            None
        ))
        .unwrap(),
        vec![
            Event::ToolCallDelta {
                index: 0,
                arguments: "{\"path\"".into()
            },
            Event::ToolCallStart {
                index: 1,
                id: "call_b".into(),
                name: "bash".into()
            },
            Event::ToolCallDelta {
                index: 1,
                arguments: "{\"cmd\":".into()
            },
        ]
    );
    assert_eq!(
        d.decode(&chunk(json!({}), Some("tool_calls"))).unwrap(),
        vec![Event::Finish(StopReason::ToolUse)]
    );
}

#[test]
fn tool_call_without_a_name_yet_is_held_until_it_arrives() {
    let mut d = ChunkDecoder::new();
    d.decode(&chunk(json!({"role": "assistant"}), None))
        .unwrap();
    assert_eq!(
        d.decode(&chunk(json!({"tool_calls": [{"index": 0, "id": "call_a", "function": {"arguments": "{\"a\":"}}]}), None))
            .unwrap(),
        vec![]
    );
    assert_eq!(
        d.decode(&chunk(
            json!({"tool_calls": [{"index": 0, "function": {"name": "read", "arguments": "1}"}}]}),
            None
        ))
        .unwrap(),
        vec![
            Event::ToolCallStart {
                index: 0,
                id: "call_a".into(),
                name: "read".into()
            },
            Event::ToolCallDelta {
                index: 0,
                arguments: "{\"a\":1}".into()
            },
        ]
    );
}

#[test]
fn missing_id_and_index_are_filled_in() {
    let mut d = ChunkDecoder::new();
    d.decode(&chunk(json!({"role": "assistant"}), None))
        .unwrap();
    let events = d
        .decode(&chunk(json!({"tool_calls": [{"function": {"name": "read", "arguments": "{}"}}, {"function": {"name": "bash", "arguments": "{}"}}]}), None))
        .unwrap();
    match &events[..] {
        [
            Event::ToolCallStart {
                index: 0,
                id: a,
                name: n1,
            },
            Event::ToolCallDelta { index: 0, .. },
            Event::ToolCallStart {
                index: 1,
                id: b,
                name: n2,
            },
            Event::ToolCallDelta { index: 1, .. },
        ] => {
            assert_eq!((n1.as_str(), n2.as_str()), ("read", "bash"));
            assert!(
                a.starts_with("call_") && b.starts_with("call_") && a != b,
                "{a} {b}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn finish_reasons_usage_done_and_errors() {
    let mut d = ChunkDecoder::new();
    d.decode(&chunk(json!({"role": "assistant"}), None))
        .unwrap();
    for (reason, expected) in [
        ("stop", StopReason::EndTurn),
        ("length", StopReason::MaxTokens),
        ("tool_calls", StopReason::ToolUse),
        ("content_filter", StopReason::EndTurn),
        ("function_call", StopReason::ToolUse),
    ] {
        assert_eq!(
            d.decode(&chunk(json!({}), Some(reason))).unwrap(),
            vec![Event::Finish(expected)],
            "{reason}"
        );
    }
    let usage_only = json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "model": "qwen", "choices": [],
        "usage": {"prompt_tokens": 12, "completion_tokens": 34, "total_tokens": 46}});
    assert_eq!(
        d.decode(&usage_only.to_string()).unwrap(),
        vec![Event::Usage(Usage {
            input_tokens: 12,
            output_tokens: 34
        })]
    );
    assert_eq!(d.decode("[DONE]").unwrap(), vec![Event::Done]);
    assert_eq!(d.decode(" [DONE] ").unwrap(), vec![Event::Done]);
    assert_eq!(
        d.decode(r#"{"error": {"message": "overloaded", "type": "server_error"}}"#)
            .unwrap(),
        vec![Event::Error("overloaded".into())]
    );
    assert_eq!(
        d.decode(r#"{"error": "plain text"}"#).unwrap(),
        vec![Event::Error("plain text".into())]
    );
    assert!(matches!(d.decode("not json"), Err(ChunkError::NotJson(_))));
    assert!(matches!(d.decode("[1, 2]"), Err(ChunkError::NotJson(_))));
}

#[test]
fn a_usage_chunk_may_also_carry_a_delta() {
    let mut d = ChunkDecoder::new();
    let both = json!({"id": "chatcmpl-1", "model": "qwen",
        "choices": [{"index": 0, "delta": {"content": "!"}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 1, "completion_tokens": 2}});
    assert_eq!(
        d.decode(&both.to_string()).unwrap(),
        vec![
            start(),
            Event::TextDelta("!".into()),
            Event::Finish(StopReason::EndTurn),
            Event::Usage(Usage {
                input_tokens: 1,
                output_tokens: 2
            }),
        ]
    );
}

// ---- completed responses ----

#[test]
fn completed_response_yields_the_stream_events() {
    let body = json!({
        "id": "chatcmpl-9", "object": "chat.completion", "model": "qwen",
        "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
            "role": "assistant",
            "reasoning_content": "plan",
            "content": "Reading",
            "tool_calls": [
                {"id": "call_a", "type": "function", "function": {"name": "read", "arguments": "{\"path\":\"a\"}"}},
                {"type": "function", "function": {"name": "bash", "arguments": "{}"}}
            ]
        }}],
        "usage": {"prompt_tokens": 5, "completion_tokens": 7}
    });
    let events = decode_response(&serde_json::to_vec(&body).unwrap()).unwrap();
    assert_eq!(events.len(), 9, "{events:?}");
    assert_eq!(
        events[0],
        Event::Start {
            id: "chatcmpl-9".into(),
            model: "qwen".into()
        }
    );
    assert_eq!(events[1], Event::ThinkingDelta("plan".into()));
    assert_eq!(events[2], Event::TextDelta("Reading".into()));
    assert_eq!(
        events[3],
        Event::ToolCallStart {
            index: 0,
            id: "call_a".into(),
            name: "read".into()
        }
    );
    assert_eq!(
        events[4],
        Event::ToolCallDelta {
            index: 0,
            arguments: "{\"path\":\"a\"}".into()
        }
    );
    assert!(
        matches!(&events[5], Event::ToolCallStart { index: 1, id, name } if id.starts_with("call_") && name == "bash")
    );
    assert_eq!(
        events[6],
        Event::ToolCallDelta {
            index: 1,
            arguments: "{}".into()
        }
    );
    assert_eq!(events[7], Event::Finish(StopReason::ToolUse));
    assert_eq!(
        events[8],
        Event::Usage(Usage {
            input_tokens: 5,
            output_tokens: 7
        })
    );
    assert_eq!(
        decode_response(br#"{"choices": []}"#),
        Err(ResponseError::NoChoices)
    );
    assert!(matches!(
        decode_response(b"[]"),
        Err(ResponseError::NotJson(_))
    ));
}

// ---- error bodies ----

#[test]
fn error_message_prefers_the_documented_field() {
    assert_eq!(
        error_message(
            br#"{"error": {"message": "bad key", "type": "invalid_request_error", "code": null}}"#
        ),
        "bad key"
    );
    assert_eq!(error_message(br#"{"error": "quota"}"#), "quota");
    assert_eq!(
        error_message(br#"{"detail": "Not Found"}"#),
        r#"{"detail": "Not Found"}"#
    );
    assert_eq!(
        error_message(b"  <html>gateway</html>\n"),
        "<html>gateway</html>"
    );
    assert_eq!(error_message(b""), "");
    let long = "x".repeat(300);
    assert_eq!(error_message(long.as_bytes()).len(), 200);
    assert_eq!(error_message(b"\xff\xfe"), "\u{FFFD}\u{FFFD}");
}

// ---- review hardening ----

#[test]
fn unnumbered_deltas_continue_the_latest_call() {
    let mut d = ChunkDecoder::new();
    d.decode(&chunk(json!({"role": "assistant"}), None))
        .unwrap();
    assert_eq!(
        d.decode(&chunk(json!({"tool_calls": [{"id": "call_1", "function": {"name": "read", "arguments": ""}}]}), None))
            .unwrap(),
        vec![Event::ToolCallStart {
            index: 0,
            id: "call_1".into(),
            name: "read".into()
        }]
    );
    assert_eq!(
        d.decode(&chunk(
            json!({"tool_calls": [{"function": {"arguments": "{\"path\":"}}]}),
            None
        ))
        .unwrap(),
        vec![Event::ToolCallDelta {
            index: 0,
            arguments: "{\"path\":".into()
        }]
    );
    assert_eq!(
        d.decode(&chunk(
            json!({"tool_calls": [{"function": {"arguments": "\"a\"}"}}]}),
            None
        ))
        .unwrap(),
        vec![Event::ToolCallDelta {
            index: 0,
            arguments: "\"a\"}".into()
        }]
    );
    let next = d
        .decode(&chunk(json!({"tool_calls": [{"id": "call_2", "function": {"name": "bash", "arguments": "{}"}}]}), None))
        .unwrap();
    assert!(
        matches!(next[0], Event::ToolCallStart { index: 1, .. }),
        "{next:?}"
    );
}

#[test]
fn a_call_that_never_gets_a_name_is_reported_at_the_end() {
    let mut d = ChunkDecoder::new();
    d.decode(&chunk(json!({"role": "assistant"}), None))
        .unwrap();
    d.decode(&chunk(
        json!({"tool_calls": [{"index": 3, "id": "call_x", "function": {"arguments": "{}"}}]}),
        None,
    ))
    .unwrap();
    let tail = d.finish();
    assert!(
        matches!(&tail[..], [Event::Error(m)] if m.contains("3") && m.contains("name")),
        "{tail:?}"
    );
    assert_eq!(ChunkDecoder::new().finish(), vec![]);
}

#[test]
fn vllm_style_error_objects_are_errors() {
    let mut d = ChunkDecoder::new();
    let vllm =
        r#"{"object":"error","message":"prompt too long","type":"BadRequestError","code":400}"#;
    assert_eq!(
        d.decode(vllm).unwrap(),
        vec![Event::Error("prompt too long".into())]
    );
    assert_eq!(error_message(vllm.as_bytes()), "prompt too long");
    assert!(matches!(
        decode_response(vllm.as_bytes()),
        Ok(events) if events == vec![Event::Error("prompt too long".into())]
    ));
}

#[test]
fn a_null_error_field_is_not_an_error() {
    let mut d = ChunkDecoder::new();
    let with_null = json!({"id": "chatcmpl-1", "model": "qwen", "error": null,
        "choices": [{"index": 0, "delta": {"content": "ok"}, "finish_reason": null}]});
    assert_eq!(
        d.decode(&with_null.to_string()).unwrap(),
        vec![start(), Event::TextDelta("ok".into())]
    );
    let body = json!({"id": "chatcmpl-9", "error": null,
        "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]});
    let events = decode_response(&serde_json::to_vec(&body).unwrap()).unwrap();
    assert!(
        events.contains(&Event::TextDelta("ok".into())),
        "{events:?}"
    );
}

#[test]
fn a_turn_whose_parts_were_all_dropped_still_keeps_role_alternation() {
    let r = request(vec![
        user(vec![Part::Text("go".into())]),
        assistant(vec![]),
        user(vec![Part::Text("continue".into())]),
        user(vec![Part::ToolResult {
            tool_use_id: "t".into(),
            content: "r".into(),
        }]),
    ]);
    assert_eq!(
        encoded(&r)["messages"],
        json!([
            {"role": "user", "content": "go"},
            {"role": "assistant", "content": ""},
            {"role": "user", "content": "continue"},
            {"role": "tool", "tool_call_id": "t", "content": "r"}
        ])
    );
}
