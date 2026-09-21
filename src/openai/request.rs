//! IR request → Chat Completions request body.

use std::borrow::Cow;

use serde_json::{Map, Value, json};

use crate::ir::{Part, Request, RequestMessage, Role, ToolChoice};

/// Text parts of one message are joined with a blank line.
const TEXT_SEPARATOR: &str = "\n\n";

/// Where a `Role::System` message that is not the first message goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SystemPlacement {
    /// A `system` message where it is.
    #[default]
    Keep,
    /// Its text appended to the leading `system` message.
    Merge,
    /// A `user` message where it is.
    User,
}

pub fn encode(request: &Request, placement: SystemPlacement) -> Vec<u8> {
    let mut messages = Vec::new();
    let mut system = request.system.clone().unwrap_or_default();
    if placement == SystemPlacement::Merge {
        for message in &request.messages {
            if message.role == Role::System {
                for part in &message.parts {
                    if let Part::Text(text) = part {
                        if !system.is_empty() {
                            system.push_str(TEXT_SEPARATOR);
                        }
                        system.push_str(text);
                    }
                }
            }
        }
    }
    if !system.is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    for message in &request.messages {
        match (message.role, placement) {
            (Role::System, SystemPlacement::Merge) => {}
            _ => encode_message(message, placement, &mut messages),
        }
    }
    let mut body = Map::new();
    body.insert("model".into(), json!(request.model));
    body.insert("messages".into(), Value::Array(messages));
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                let mut function = Map::new();
                function.insert("name".into(), json!(tool.name));
                if let Some(description) = &tool.description {
                    function.insert("description".into(), json!(description));
                }
                function.insert("parameters".into(), with_properties(&tool.parameters));
                json!({"type": "function", "function": function})
            })
            .collect();
        body.insert("tools".into(), Value::Array(tools));
    }
    if let Some(choice) = &request.tool_choice {
        let choice = match choice {
            ToolChoice::Auto => json!("auto"),
            ToolChoice::Required => json!("required"),
            ToolChoice::None => json!("none"),
            ToolChoice::Tool(name) => json!({"type": "function", "function": {"name": name}}),
        };
        body.insert("tool_choice".into(), choice);
    }
    if let Some(max_tokens) = request.max_tokens {
        body.insert("max_tokens".into(), json!(max_tokens));
    }
    if let Some(temperature) = request.temperature {
        body.insert("temperature".into(), json!(temperature));
    }
    if let Some(top_p) = request.top_p {
        body.insert("top_p".into(), json!(top_p));
    }
    if !request.stop.is_empty() {
        body.insert("stop".into(), json!(request.stop));
    }
    if request.disable_parallel_tool_calls {
        body.insert("parallel_tool_calls".into(), json!(false));
    }
    if let Some(effort) = &request.reasoning_effort {
        body.insert("reasoning_effort".into(), json!(effort));
    }
    if let Some(user) = &request.user {
        body.insert("user".into(), json!(user));
    }
    body.insert("stream".into(), json!(request.stream));
    if request.stream {
        body.insert("stream_options".into(), json!({"include_usage": true}));
    }
    serde_json::to_vec(&Value::Object(body)).expect("a JSON tree serializes")
}

/// A user message yields one `tool` message per tool result, then one
/// `user` message for the rest; an assistant message yields one message with
/// its text and tool calls.
fn encode_message(message: &RequestMessage, placement: SystemPlacement, out: &mut Vec<Value>) {
    let emitted_before = out.len();
    let mut pieces: Vec<Piece> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for part in &message.parts {
        match part {
            Part::Text(text) => pieces.push(Piece::Text(text)),
            Part::Image { media_type, data } => {
                pieces.push(Piece::Image(image_part(media_type, data)))
            }
            Part::ToolUse { id, name, input } => tool_calls.push(json!({
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": serde_json::to_string(input).expect("a JSON tree serializes"),
                }
            })),
            Part::ToolResult {
                tool_use_id,
                content,
                images,
            } => {
                // A tool message carries text only; its images ride in the
                // user message that follows, labelled with the call.
                let mut content = Cow::Borrowed(content.as_str());
                if !images.is_empty() {
                    let (count, verb, noun) = match images.len() {
                        1 => ("1 image".to_owned(), "follows", "Image"),
                        n => (format!("{n} images"), "follow", "Images"),
                    };
                    let content = content.to_mut();
                    if !content.is_empty() {
                        content.push_str(TEXT_SEPARATOR);
                    }
                    content.push_str(&format!(
                        "({count} from this tool call {verb} in the next user message)"
                    ));
                    pieces.push(Piece::Label(format!(
                        "{noun} from tool call {tool_use_id}:"
                    )));
                    pieces.extend(
                        images
                            .iter()
                            .map(|image| Piece::Image(image_part(&image.media_type, &image.data))),
                    );
                }
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": content,
                }));
            }
        }
    }
    let content = content_of(pieces);
    // A turn that produced nothing (its blocks were all dropped) still
    // occupies its place, so roles keep alternating for templates that
    // insist on it; a turn that only produced tool messages does not.
    if content.is_none() && tool_calls.is_empty() && out.len() > emitted_before {
        return;
    }
    let mut object = Map::new();
    object.insert(
        "role".into(),
        json!(match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System if placement == SystemPlacement::User => "user",
            Role::System => "system",
        }),
    );
    match (content, tool_calls.is_empty()) {
        (Some(content), _) => {
            object.insert("content".into(), content);
        }
        (None, true) => {
            object.insert("content".into(), json!(""));
        }
        (None, false) => {}
    }
    if !tool_calls.is_empty() {
        object.insert("tool_calls".into(), Value::Array(tool_calls));
    }
    out.push(Value::Object(object));
}

/// What a message's own content is made of, before it is known whether it
/// can be a plain string.
enum Piece<'a> {
    Text(&'a str),
    /// Text the router adds; owned.
    Label(String),
    Image(Value),
}

/// A plain string when there is only text, a parts array when an image is
/// among them, `None` when there is nothing.
fn content_of(pieces: Vec<Piece>) -> Option<Value> {
    if pieces.is_empty() {
        return None;
    }
    if pieces.iter().any(|piece| matches!(piece, Piece::Image(_))) {
        let parts = pieces
            .into_iter()
            .map(|piece| match piece {
                Piece::Text(text) => json!({"type": "text", "text": text}),
                Piece::Label(text) => json!({"type": "text", "text": text}),
                Piece::Image(part) => part,
            })
            .collect();
        return Some(Value::Array(parts));
    }
    let texts: Vec<&str> = pieces
        .iter()
        .map(|piece| match piece {
            Piece::Text(text) => *text,
            Piece::Label(text) => text.as_str(),
            Piece::Image(_) => unreachable!("no image among the pieces"),
        })
        .collect();
    Some(Value::String(texts.join(TEXT_SEPARATOR)))
}

/// An object schema always carries `properties`: some servers (LM Studio)
/// reject a tool whose schema has none, and a tool without arguments is a
/// legitimate Anthropic tool.
fn with_properties(schema: &Value) -> Value {
    match schema {
        Value::Object(fields)
            if fields.get("type").and_then(Value::as_str) == Some("object")
                && !fields.contains_key("properties") =>
        {
            let mut fields = fields.clone();
            fields.insert("properties".into(), json!({}));
            Value::Object(fields)
        }
        other => other.clone(),
    }
}

fn image_part(media_type: &str, data: &str) -> Value {
    json!({
        "type": "image_url",
        "image_url": {"url": format!("data:{media_type};base64,{data}")}
    })
}
