//! IR request → Chat Completions request body.

use serde_json::{Map, Value, json};

use crate::ir::{Part, Request, RequestMessage, Role, ToolChoice};

/// Text parts of one message are joined with a blank line.
const TEXT_SEPARATOR: &str = "\n\n";

pub fn encode(request: &Request) -> Vec<u8> {
    let mut messages = Vec::new();
    if let Some(system) = &request.system {
        messages.push(json!({"role": "system", "content": system}));
    }
    for message in &request.messages {
        encode_message(message, &mut messages);
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
    if let Some(parallel) = request.parallel_tool_calls {
        body.insert("parallel_tool_calls".into(), json!(parallel));
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
fn encode_message(message: &RequestMessage, out: &mut Vec<Value>) {
    let before = out.len();
    let mut texts: Vec<&str> = Vec::new();
    let mut parts: Vec<Value> = Vec::new();
    let mut has_image = false;
    let mut tool_calls: Vec<Value> = Vec::new();
    for part in &message.parts {
        match part {
            Part::Text(text) => {
                texts.push(text);
                parts.push(json!({"type": "text", "text": text}));
            }
            Part::Image { media_type, data } => {
                has_image = true;
                parts.push(image_part(media_type, data));
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
                let mut content = content.clone();
                if !images.is_empty() {
                    let note = format!(
                        "({} from this tool call {} in the next user message)",
                        count_images(images.len()),
                        if images.len() == 1 {
                            "follows"
                        } else {
                            "follow"
                        }
                    );
                    if !content.is_empty() {
                        content.push_str(TEXT_SEPARATOR);
                    }
                    content.push_str(&note);
                    has_image = true;
                    parts.push(json!({
                        "type": "text",
                        "text": format!(
                            "{} from tool call {tool_use_id}:",
                            if images.len() == 1 { "Image" } else { "Images" }
                        ),
                    }));
                    for image in images {
                        parts.push(image_part(&image.media_type, &image.data));
                    }
                }
                out.push(json!({
                    "role": "tool",
                    "tool_call_id": tool_use_id,
                    "content": content,
                }));
            }
        }
    }
    // A turn that produced nothing (its blocks were all dropped) still
    // occupies its place, so roles keep alternating for templates that
    // insist on it.
    let content = if has_image {
        Some(Value::Array(parts))
    } else if !texts.is_empty() {
        Some(Value::String(texts.join(TEXT_SEPARATOR)))
    } else if tool_calls.is_empty() && out.len() == before {
        Some(Value::String(String::new()))
    } else {
        None
    };
    let mut object = Map::new();
    object.insert(
        "role".into(),
        json!(match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
        }),
    );
    if let Some(content) = content {
        object.insert("content".into(), content);
    }
    if !tool_calls.is_empty() {
        object.insert("tool_calls".into(), Value::Array(tool_calls));
    }
    if object.len() > 1 {
        out.push(Value::Object(object));
    }
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

fn count_images(n: usize) -> String {
    if n == 1 {
        "1 image".to_owned()
    } else {
        format!("{n} images")
    }
}
