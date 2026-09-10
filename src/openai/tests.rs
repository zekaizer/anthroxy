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
