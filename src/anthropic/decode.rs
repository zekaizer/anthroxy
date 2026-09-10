//! Messages API request → IR (ADR-0010). Fields with no counterpart in the
//! IR are dropped; block types the IR cannot carry are an error, so content
//! the model needs is never silently lost.

use serde_json::Value;

use crate::ir::{Part, Request, RequestMessage, Role, Tool, ToolChoice};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("request body is not a JSON object")]
    NotAnObject,
    #[error("`{0}` is missing or has the wrong type")]
    Field(&'static str),
    #[error("messages[{index}].role `{role}` is not `user` or `assistant`")]
    Role { index: usize, role: String },
    #[error("messages[{index}] has a `{block}` block, which cannot be sent to this backend")]
    UnsupportedBlock { index: usize, block: String },
    #[error(
        "tool `{name}` has no `{field}`; only tools with an input schema can be sent to this backend"
    )]
    Tool { name: String, field: &'static str },
}

/// Parts of a message are joined with a blank line when flattened to text.
const TEXT_SEPARATOR: &str = "\n\n";

pub fn decode(body: &[u8]) -> Result<Request, DecodeError> {
    let root: Value = serde_json::from_slice(body).map_err(|_| DecodeError::NotAnObject)?;
    let root = root.as_object().ok_or(DecodeError::NotAnObject)?;
    let model = root
        .get("model")
        .and_then(Value::as_str)
        .ok_or(DecodeError::Field("model"))?
        .to_owned();
    let messages = root
        .get("messages")
        .and_then(Value::as_array)
        .ok_or(DecodeError::Field("messages"))?
        .iter()
        .enumerate()
        .map(|(index, message)| decode_message(index, message))
        .collect::<Result<Vec<_>, _>>()?;
    let system = match root.get("system") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(Value::Array(blocks)) => Some(
            blocks
                .iter()
                .map(|block| field_str(block, "text"))
                .collect::<Result<Vec<_>, _>>()?
                .join(TEXT_SEPARATOR),
        ),
        Some(_) => return Err(DecodeError::Field("system")),
    };
    let tools = match root.get("tools") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(tools)) => tools
            .iter()
            .map(decode_tool)
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(DecodeError::Field("tools")),
    };
    let tool_choice = match root.get("tool_choice") {
        None | Some(Value::Null) => None,
        Some(choice) => Some(match field_str(choice, "type")? {
            "auto" => ToolChoice::Auto,
            "any" => ToolChoice::Required,
            "none" => ToolChoice::None,
            "tool" => ToolChoice::Tool(field_str(choice, "name")?.to_owned()),
            _ => return Err(DecodeError::Field("tool_choice.type")),
        }),
    };
    let stop = match root.get("stop_sequences") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or(DecodeError::Field("stop_sequences"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(DecodeError::Field("stop_sequences")),
    };
    Ok(Request {
        model,
        system,
        messages,
        tools,
        tool_choice,
        max_tokens: root.get("max_tokens").and_then(Value::as_u64),
        temperature: root.get("temperature").and_then(Value::as_f64),
        top_p: root.get("top_p").and_then(Value::as_f64),
        stop,
        stream: root.get("stream").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn decode_message(index: usize, message: &Value) -> Result<RequestMessage, DecodeError> {
    let role = match field_str(message, "role")? {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        other => {
            return Err(DecodeError::Role {
                index,
                role: other.to_owned(),
            });
        }
    };
    let parts = match message.get("content") {
        Some(Value::String(text)) => vec![Part::Text(text.clone())],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| decode_block(index, block).transpose())
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(DecodeError::Field("content")),
    };
    Ok(RequestMessage { role, parts })
}

/// `None` for blocks that are dropped on purpose (thinking).
fn decode_block(index: usize, block: &Value) -> Result<Option<Part>, DecodeError> {
    let part = match field_str(block, "type")? {
        "text" => Part::Text(field_str(block, "text")?.to_owned()),
        "image" => {
            let source = block.get("source").ok_or(DecodeError::Field("source"))?;
            if field_str(source, "type")? != "base64" {
                return Err(DecodeError::UnsupportedBlock {
                    index,
                    block: "image with a non-base64 source".to_owned(),
                });
            }
            Part::Image {
                media_type: field_str(source, "media_type")?.to_owned(),
                data: field_str(source, "data")?.to_owned(),
            }
        }
        "tool_use" => Part::ToolUse {
            id: field_str(block, "id")?.to_owned(),
            name: field_str(block, "name")?.to_owned(),
            input: block
                .get("input")
                .cloned()
                .unwrap_or(Value::Object(Default::default())),
        },
        "tool_result" => Part::ToolResult {
            tool_use_id: field_str(block, "tool_use_id")?.to_owned(),
            content: flatten_text(block.get("content"))?,
        },
        "thinking" | "redacted_thinking" => return Ok(None),
        other => {
            return Err(DecodeError::UnsupportedBlock {
                index,
                block: other.to_owned(),
            });
        }
    };
    Ok(Some(part))
}

/// Text of a tool result: a string as is, blocks by their text with images
/// left out.
fn flatten_text(content: Option<&Value>) -> Result<String, DecodeError> {
    match content {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(text)) => Ok(text.clone()),
        Some(Value::Array(blocks)) => Ok(blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
            .map(|block| field_str(block, "text"))
            .collect::<Result<Vec<_>, _>>()?
            .join(TEXT_SEPARATOR)),
        Some(_) => Err(DecodeError::Field("content")),
    }
}

fn decode_tool(tool: &Value) -> Result<Tool, DecodeError> {
    let name = field_str(tool, "name")?.to_owned();
    let parameters = tool
        .get("input_schema")
        .cloned()
        .ok_or_else(|| DecodeError::Tool {
            name: name.clone(),
            field: "input_schema",
        })?;
    Ok(Tool {
        name,
        description: tool
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned),
        parameters,
    })
}

fn field_str<'a>(value: &'a Value, field: &'static str) -> Result<&'a str, DecodeError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .ok_or(DecodeError::Field(field))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn decode_json(value: Value) -> Result<Request, DecodeError> {
        decode(&serde_json::to_vec(&value).unwrap())
    }

    fn base() -> Value {
        json!({"model": "m", "max_tokens": 100, "messages": [{"role": "user", "content": "hi"}]})
    }

    #[test]
    fn minimal_request() {
        let request = decode_json(base()).unwrap();
        assert_eq!(
            request,
            Request {
                model: "m".into(),
                system: None,
                messages: vec![RequestMessage {
                    role: Role::User,
                    parts: vec![Part::Text("hi".into())],
                }],
                tools: vec![],
                tool_choice: None,
                max_tokens: Some(100),
                temperature: None,
                top_p: None,
                stop: vec![],
                stream: false,
            }
        );
    }

    #[test]
    fn system_string_or_blocks_become_one_text() {
        let mut v = base();
        v["system"] = json!("be brief");
        assert_eq!(decode_json(v).unwrap().system.as_deref(), Some("be brief"));

        let mut v = base();
        v["system"] = json!([
            {"type": "text", "text": "a", "cache_control": {"type": "ephemeral"}},
            {"type": "text", "text": "b"}
        ]);
        assert_eq!(decode_json(v).unwrap().system.as_deref(), Some("a\n\nb"));
    }

    #[test]
    fn content_blocks_map_to_parts_and_thinking_is_dropped() {
        let mut v = base();
        v["messages"] = json!([
            {"role": "user", "content": [
                {"type": "text", "text": "look", "cache_control": {"type": "ephemeral"}},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}
            ]},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "hmm", "signature": "sig"},
                {"type": "redacted_thinking", "data": "xx"},
                {"type": "text", "text": "I will read"},
                {"type": "tool_use", "id": "toolu_1", "name": "read", "input": {"path": "a.rs"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}"},
                {"type": "tool_result", "tool_use_id": "toolu_2", "content": [
                    {"type": "text", "text": "line 1"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "BBBB"}},
                    {"type": "text", "text": "line 2"}
                ], "is_error": true},
                {"type": "tool_result", "tool_use_id": "toolu_3"}
            ]}
        ]);
        let messages = decode_json(v).unwrap().messages;
        assert_eq!(
            messages,
            vec![
                RequestMessage {
                    role: Role::User,
                    parts: vec![
                        Part::Text("look".into()),
                        Part::Image {
                            media_type: "image/png".into(),
                            data: "AAAA".into()
                        },
                    ],
                },
                RequestMessage {
                    role: Role::Assistant,
                    parts: vec![
                        Part::Text("I will read".into()),
                        Part::ToolUse {
                            id: "toolu_1".into(),
                            name: "read".into(),
                            input: json!({"path": "a.rs"}),
                        },
                    ],
                },
                RequestMessage {
                    role: Role::User,
                    parts: vec![
                        Part::ToolResult {
                            tool_use_id: "toolu_1".into(),
                            content: "fn main() {}".into(),
                        },
                        Part::ToolResult {
                            tool_use_id: "toolu_2".into(),
                            content: "line 1\n\nline 2".into(),
                        },
                        Part::ToolResult {
                            tool_use_id: "toolu_3".into(),
                            content: String::new(),
                        },
                    ],
                },
            ]
        );
    }

    #[test]
    fn tools_and_tool_choice() {
        let mut v = base();
        v["tools"] = json!([
            {"name": "read", "description": "Read a file", "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}},
            {"name": "bare", "input_schema": {"type": "object"}}
        ]);
        let with = |choice: Value| {
            let mut v = v.clone();
            v["tool_choice"] = choice;
            decode_json(v).unwrap()
        };
        let request = with(json!({"type": "auto", "disable_parallel_tool_use": true}));
        assert_eq!(
            request.tools,
            vec![
                Tool {
                    name: "read".into(),
                    description: Some("Read a file".into()),
                    parameters: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
                },
                Tool {
                    name: "bare".into(),
                    description: None,
                    parameters: json!({"type": "object"}),
                },
            ]
        );
        assert_eq!(request.tool_choice, Some(ToolChoice::Auto));
        assert_eq!(
            with(json!({"type": "any"})).tool_choice,
            Some(ToolChoice::Required)
        );
        assert_eq!(
            with(json!({"type": "none"})).tool_choice,
            Some(ToolChoice::None)
        );
        assert_eq!(
            with(json!({"type": "tool", "name": "read"})).tool_choice,
            Some(ToolChoice::Tool("read".into()))
        );
        assert_eq!(decode_json(v).unwrap().tool_choice, None);
    }

    #[test]
    fn sampling_fields_and_ignored_fields() {
        let mut v = base();
        v["temperature"] = json!(0.5);
        v["top_p"] = json!(0.9);
        v["top_k"] = json!(40);
        v["stop_sequences"] = json!(["END"]);
        v["stream"] = json!(true);
        v["metadata"] = json!({"user_id": "u"});
        v["thinking"] = json!({"type": "enabled", "budget_tokens": 1024});
        v["context_management"] = json!({"edits": []});
        let request = decode_json(v).unwrap();
        assert_eq!(request.temperature, Some(0.5));
        assert_eq!(request.top_p, Some(0.9));
        assert_eq!(request.stop, vec!["END".to_owned()]);
        assert!(request.stream);
    }

    #[test]
    fn unsupported_block_types_are_errors() {
        let mut v = base();
        v["messages"] = json!([{"role": "user", "content": [
            {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "x"}}
        ]}]);
        assert_eq!(
            decode_json(v),
            Err(DecodeError::UnsupportedBlock {
                index: 0,
                block: "document".into()
            })
        );
    }

    #[test]
    fn a_tool_without_a_schema_is_named_in_the_error() {
        let mut v = base();
        v["tools"] = json!([{"type": "web_search_20250305", "name": "web_search"}]);
        assert_eq!(
            decode_json(v),
            Err(DecodeError::Tool {
                name: "web_search".into(),
                field: "input_schema"
            })
        );
    }

    #[test]
    fn structural_errors() {
        assert_eq!(decode(b"[]"), Err(DecodeError::NotAnObject));
        assert_eq!(decode(b"not json"), Err(DecodeError::NotAnObject));
        assert_eq!(
            decode_json(json!({"model": "m"})),
            Err(DecodeError::Field("messages"))
        );
        let mut v = base();
        v["messages"] = json!([{"role": "system", "content": "x"}]);
        assert_eq!(
            decode_json(v),
            Err(DecodeError::Role {
                index: 0,
                role: "system".into()
            })
        );
        let mut v = base();
        v["messages"] = json!([{"role": "user", "content": [{"type": "text"}]}]);
        assert_eq!(decode_json(v), Err(DecodeError::Field("text")));
    }
}
