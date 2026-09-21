//! Messages API request → IR (ADR-0010). Fields with no counterpart in the
//! IR are dropped; block types the IR cannot carry are an error, so content
//! the model needs is never silently lost.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;

use serde::Serialize;
use serde_json::Value;

use crate::ir::{Image, Part, Request, RequestMessage, Role, TEXT_SEPARATOR, Tool, ToolChoice};
use crate::text::short;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("request body is not a JSON object")]
    NotAnObject,
    #[error("`{0}` is missing or has the wrong type")]
    Field(&'static str),
    #[error("messages[{index}].role `{role}` is not `user`, `assistant` or `system`")]
    Role { index: usize, role: String },
    #[error("messages[{index}] has a `{block}` block, which cannot be sent to this backend")]
    UnsupportedBlock { index: usize, block: String },
    #[error(
        "tool `{name}` has no `{field}`; only tools with an input schema can be sent to this backend"
    )]
    Tool { name: String, field: &'static str },
    #[error("messages[{index}] has a `{block}` block, which a `{role}` message cannot carry")]
    WrongRole {
        index: usize,
        block: String,
        role: String,
    },
}

pub fn decode(body: &[u8]) -> Result<Request, DecodeError> {
    let root: Value = serde_json::from_slice(body).map_err(|_| DecodeError::NotAnObject)?;
    let root = root.as_object().ok_or(DecodeError::NotAnObject)?;
    let model = root
        .get("model")
        .and_then(Value::as_str)
        .ok_or(DecodeError::Field("model"))?
        .to_owned();
    let raw_messages = root
        .get("messages")
        .and_then(Value::as_array)
        .ok_or(DecodeError::Field("messages"))?;
    // A tool marked `defer_loading` is invisible to the model until a
    // `tool_reference` names it, so an unreferenced one is not sent.
    let raw_tools = optional_array(root, "tools")?;
    let referenced = if raw_tools.iter().any(deferred) {
        referenced_tools(raw_messages)
    } else {
        HashSet::new()
    };
    let tools = raw_tools
        .iter()
        .filter(|tool| {
            !deferred(tool)
                || tool
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| referenced.contains(name))
        })
        .map(decode_tool)
        .collect::<Result<Vec<_>, _>>()?;
    let mut definitions = Definitions {
        by_name: tools
            .iter()
            .map(|tool| (tool.name.as_str(), tool))
            .collect(),
        spelled: HashSet::new(),
    };
    let messages = raw_messages
        .iter()
        .enumerate()
        .map(|(index, message)| decode_message(index, message, &mut definitions))
        .collect::<Result<Vec<_>, _>>()?;
    let system = match root.get("system") {
        Some(Value::String(text)) => Some(text.clone()),
        _ => Some(
            optional_array(root, "system")?
                .iter()
                .map(|block| field_str(block, "text"))
                .collect::<Result<Vec<_>, _>>()?
                .join(TEXT_SEPARATOR),
        ),
    }
    .filter(|text| !text.is_empty());
    let choice = root.get("tool_choice").filter(|c| !c.is_null());
    let disable_parallel_tool_calls = choice
        .and_then(|c| c.get("disable_parallel_tool_use"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let tool_choice = match choice {
        None => None,
        Some(choice) => Some(match field_str(choice, "type")? {
            "auto" => ToolChoice::Auto,
            "any" => ToolChoice::Required,
            "none" => ToolChoice::None,
            "tool" => ToolChoice::Tool(field_str(choice, "name")?.to_owned()),
            _ => return Err(DecodeError::Field("tool_choice.type")),
        }),
    };
    let stop = optional_array(root, "stop_sequences")?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or(DecodeError::Field("stop_sequences"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Request {
        model,
        system,
        messages,
        tools,
        tool_choice,
        max_tokens: optional(root, "max_tokens", Value::as_u64)?,
        temperature: optional(root, "temperature", Value::as_f64)?,
        top_p: optional(root, "top_p", Value::as_f64)?,
        stop,
        stream: optional(root, "stream", Value::as_bool)?.unwrap_or(false),
        user: root
            .get("metadata")
            .and_then(|m| m.get("user_id"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        reasoning_effort: root
            .get("output_config")
            .and_then(|o| o.get("effort"))
            .and_then(Value::as_str)
            .map(|effort| match effort {
                "max" => "high".to_owned(),
                other => other.to_owned(),
            }),
        disable_parallel_tool_calls,
    })
}

/// An array field that may be absent (then empty), but not of another type.
fn optional_array<'a>(
    root: &'a serde_json::Map<String, Value>,
    field: &'static str,
) -> Result<&'a [Value], DecodeError> {
    match root.get(field) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(items)) => Ok(items),
        Some(_) => Err(DecodeError::Field(field)),
    }
}

/// A field that may be absent, but not of another type: a value the
/// backend would otherwise silently replace with its own default.
fn optional<T>(
    root: &serde_json::Map<String, Value>,
    field: &'static str,
    as_type: impl Fn(&Value) -> Option<T>,
) -> Result<Option<T>, DecodeError> {
    match root.get(field) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => as_type(value).map(Some).ok_or(DecodeError::Field(field)),
    }
}

/// The request's tools by name, and which of them a `tool_reference` has
/// already spelled out; a later reference names the tool only, so the text
/// grows with the tools, not with the references.
struct Definitions<'a> {
    by_name: HashMap<&'a str, &'a Tool>,
    spelled: HashSet<String>,
}

fn decode_message(
    index: usize,
    message: &Value,
    definitions: &mut Definitions,
) -> Result<RequestMessage, DecodeError> {
    let role_name = field_str(message, "role")?;
    let role = match role_name {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        "system" => Role::System,
        other => {
            return Err(DecodeError::Role {
                index,
                role: short(other),
            });
        }
    };
    let parts = match message.get("content") {
        Some(Value::String(text)) => vec![Part::Text(text.clone())],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| decode_block(index, block, definitions).transpose())
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(DecodeError::Field("content")),
    };
    if let Some(part) = parts.iter().find(|part| !allowed(role, part)) {
        return Err(DecodeError::WrongRole {
            index,
            block: part.kind().to_owned(),
            role: role_name.to_owned(),
        });
    }
    Ok(RequestMessage { role, parts })
}

/// What each role may carry, as the Messages API defines it.
fn allowed(role: Role, part: &Part) -> bool {
    matches!(
        (role, part),
        (
            Role::User,
            Part::Text(_) | Part::Image { .. } | Part::ToolResult { .. }
        ) | (Role::Assistant, Part::Text(_) | Part::ToolUse { .. })
            | (Role::System, Part::Text(_))
    )
}

/// `None` for blocks that are dropped on purpose (thinking).
fn decode_block(
    index: usize,
    block: &Value,
    definitions: &mut Definitions,
) -> Result<Option<Part>, DecodeError> {
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
        "tool_result" => {
            let (content, images) = tool_result_content(index, block.get("content"), definitions)?;
            Part::ToolResult {
                tool_use_id: field_str(block, "tool_use_id")?.to_owned(),
                content,
                images,
                is_error: block
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            }
        }
        "document" => Part::Text(document_text(block)?),
        "thinking" | "redacted_thinking" => return Ok(None),
        other => {
            return Err(DecodeError::UnsupportedBlock {
                index,
                block: short(other),
            });
        }
    };
    Ok(Some(part))
}

/// A `document` block as text: its content for a plain-text source, a note
/// saying what was left out otherwise. Chat Completions servers seldom take
/// files, and a note lets the model tell the user instead of the turn
/// failing.
fn document_text(block: &Value) -> Result<String, DecodeError> {
    let source = block.get("source").ok_or(DecodeError::Field("source"))?;
    if field_str(source, "type")? == "text" {
        return Ok(field_str(source, "data")?.to_owned());
    }
    let media_type = source
        .get("media_type")
        .and_then(Value::as_str)
        .unwrap_or("unknown type");
    let data = source.get("data").and_then(Value::as_str).unwrap_or("");
    let padding = data.bytes().rev().take_while(|&b| b == b'=').count();
    let bytes = (data.len() / 4 * 3).saturating_sub(padding);
    let title = block
        .get("title")
        .and_then(Value::as_str)
        .map(|t| format!(" \"{t}\""))
        .unwrap_or_default();
    Ok(format!(
        "[document{title} ({media_type}, {bytes} bytes) omitted: this backend cannot receive documents]"
    ))
}

/// A tool result as text plus its images: a string as is, blocks by their
/// text, image blocks apart, `tool_reference` blocks as one `<functions>`
/// block of the definitions they name. Any other block type is an error, as
/// at message level.
fn tool_result_content(
    index: usize,
    content: Option<&Value>,
    definitions: &mut Definitions,
) -> Result<(String, Vec<Image>), DecodeError> {
    match content {
        None | Some(Value::Null) => Ok((String::new(), Vec::new())),
        Some(Value::String(text)) => Ok((text.clone(), Vec::new())),
        Some(Value::Array(blocks)) => {
            let mut texts = Vec::new();
            let mut images = Vec::new();
            let mut references = Vec::new();
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("tool_reference") {
                    references.push(field_str(block, "tool_name")?);
                    continue;
                }
                match decode_block(index, block, definitions)? {
                    Some(Part::Text(text)) => texts.push(text),
                    Some(Part::Image { media_type, data }) => {
                        images.push(Image { media_type, data })
                    }
                    Some(part) => {
                        return Err(DecodeError::UnsupportedBlock {
                            index,
                            block: part.kind().to_owned(),
                        });
                    }
                    None => {}
                }
            }
            if !references.is_empty() {
                texts.push(functions_block(&references, definitions));
            }
            Ok((texts.join(TEXT_SEPARATOR), images))
        }
        Some(_) => Err(DecodeError::Field("content")),
    }
}

/// The definitions `tool_reference` blocks name, in the form Claude Code's
/// `ToolSearch` tool tells the model to expect: one `<function>` line of
/// JSON per tool. The Anthropic API expands a reference into the tool's
/// definition; a Chat Completions server sees only text, so the definition
/// is spelled out here, once per request: a repeated name, and a name not
/// among the request's tools, is written alone. `</` is escaped so no
/// definition can close the line or the block.
fn functions_block(names: &[&str], definitions: &mut Definitions) -> String {
    #[derive(Serialize)]
    struct Function<'a> {
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<&'a str>,
        name: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        parameters: Option<&'a Value>,
    }
    let mut out = String::from("<functions>\n");
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(*name) {
            continue;
        }
        let tool = definitions
            .by_name
            .get(name)
            .filter(|_| definitions.spelled.insert((*name).to_owned()));
        let function = Function {
            description: tool.and_then(|tool| tool.description.as_deref()),
            name,
            parameters: tool.map(|tool| &tool.parameters),
        };
        let function = serde_json::to_string(&function)
            .expect("a JSON tree serializes")
            .replace("</", "<\\/");
        writeln!(out, "<function>{function}</function>").expect("String never fails to write");
    }
    out.push_str("</functions>");
    out
}

fn deferred(tool: &Value) -> bool {
    tool.get("defer_loading")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Every name a `tool_reference` block in the conversation carries.
fn referenced_tools(messages: &[Value]) -> HashSet<&str> {
    messages
        .iter()
        .filter_map(|message| message.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .filter_map(|block| block.get("content").and_then(Value::as_array))
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_reference"))
        .filter_map(|block| block.get("tool_name").and_then(Value::as_str))
        .collect()
}

fn decode_tool(tool: &Value) -> Result<Tool, DecodeError> {
    let name = field_str(tool, "name")?.to_owned();
    let parameters = tool
        .get("input_schema")
        .cloned()
        .ok_or_else(|| DecodeError::Tool {
            name: short(&name),
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
                user: None,
                reasoning_effort: None,
                disable_parallel_tool_calls: false,
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
    fn content_blocks_map_to_parts_thinking_is_dropped_and_result_images_kept() {
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
                            images: vec![],
                            is_error: false,
                        },
                        Part::ToolResult {
                            tool_use_id: "toolu_2".into(),
                            content: "line 1\n\nline 2".into(),
                            images: vec![Image {
                                media_type: "image/png".into(),
                                data: "BBBB".into()
                            }],
                            is_error: true,
                        },
                        Part::ToolResult {
                            tool_use_id: "toolu_3".into(),
                            content: String::new(),
                            images: vec![],
                            is_error: false,
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
            {"type": "server_tool_use", "id": "x", "name": "web_search", "input": {}}
        ]}]);
        assert_eq!(
            decode_json(v),
            Err(DecodeError::UnsupportedBlock {
                index: 0,
                block: "server_tool_use".into()
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
    fn wrongly_typed_sampling_fields_are_errors_not_defaults() {
        for (field, value) in [
            ("max_tokens", json!(100.5)),
            ("max_tokens", json!("100")),
            ("max_tokens", json!(-1)),
            ("temperature", json!("0.5")),
            ("top_p", json!(true)),
            ("stream", json!("yes")),
        ] {
            let mut v = base();
            v[field] = value.clone();
            assert_eq!(
                decode_json(v),
                Err(DecodeError::Field(field)),
                "{field} = {value}"
            );
        }
        let mut v = base();
        v["temperature"] = json!(1);
        assert_eq!(decode_json(v).unwrap().temperature, Some(1.0));
    }

    #[test]
    fn empty_system_is_no_system() {
        for empty in [json!(""), json!([])] {
            let mut v = base();
            v["system"] = empty;
            assert_eq!(decode_json(v).unwrap().system, None);
        }
    }

    #[test]
    fn unsupported_blocks_inside_tool_results_are_errors_too() {
        let mut v = base();
        v["messages"] = json!([{"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t", "content": [
                {"type": "server_tool_use", "id": "x", "name": "web_search", "input": {}}
            ]}
        ]}]);
        assert_eq!(
            decode_json(v),
            Err(DecodeError::UnsupportedBlock {
                index: 0,
                block: "server_tool_use".into()
            })
        );
    }

    #[test]
    fn parts_in_the_wrong_role_are_errors() {
        let mut v = base();
        v["messages"] = json!([{"role": "assistant", "content": [
            {"type": "tool_result", "tool_use_id": "t", "content": "r"}
        ]}]);
        assert_eq!(
            decode_json(v),
            Err(DecodeError::WrongRole {
                index: 0,
                block: "tool_result".into(),
                role: "assistant".into()
            })
        );
        let mut v = base();
        v["messages"] = json!([{"role": "user", "content": [
            {"type": "tool_use", "id": "t", "name": "n", "input": {}}
        ]}]);
        assert_eq!(
            decode_json(v),
            Err(DecodeError::WrongRole {
                index: 0,
                block: "tool_use".into(),
                role: "user".into()
            })
        );
    }

    #[test]
    fn mid_conversation_system_messages_keep_their_place() {
        let mut v = base();
        v["messages"] = json!([
            {"role": "user", "content": "hi"},
            {"role": "system", "content": [{"type": "text", "text": "env changed", "cache_control": {"type": "ephemeral"}}]},
            {"role": "assistant", "content": "ok"}
        ]);
        let messages = decode_json(v).unwrap().messages;
        assert_eq!(messages[1].role, Role::System);
        assert_eq!(messages[1].parts, vec![Part::Text("env changed".into())]);

        let mut v = base();
        v["messages"] = json!([{"role": "system", "content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}
        ]}]);
        assert_eq!(
            decode_json(v),
            Err(DecodeError::WrongRole {
                index: 0,
                block: "image".into(),
                role: "system".into()
            })
        );
    }

    #[test]
    fn documents_become_a_note_or_their_text() {
        let mut v = base();
        v["messages"] = json!([{"role": "user", "content": [
            {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBERi0xLjQK"}, "title": "secret.pdf"},
            {"type": "document", "source": {"type": "text", "media_type": "text/plain", "data": "plain words"}},
            {"type": "tool_result", "tool_use_id": "t", "content": [
                {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "JVBERi0xLjQK"}}
            ]}
        ]}]);
        let parts = decode_json(v).unwrap().messages.remove(0).parts;
        assert_eq!(
            parts[0],
            Part::Text("[document \"secret.pdf\" (application/pdf, 9 bytes) omitted: this backend cannot receive documents]".into())
        );
        assert_eq!(parts[1], Part::Text("plain words".into()));
        assert_eq!(
            parts[2],
            Part::ToolResult {
                tool_use_id: "t".into(),
                content: "[document (application/pdf, 9 bytes) omitted: this backend cannot receive documents]".into(),
                images: vec![],
                is_error: false,
            }
        );
    }

    #[test]
    fn a_document_with_less_data_than_padding_is_a_note_not_a_panic() {
        let mut v = base();
        v["messages"] = json!([{"role": "user", "content": [
            {"type": "document", "source": {"type": "base64", "media_type": "application/pdf", "data": "=="}}
        ]}]);
        let parts = decode_json(v).unwrap().messages.remove(0).parts;
        assert_eq!(
            parts[0],
            Part::Text("[document (application/pdf, 0 bytes) omitted: this backend cannot receive documents]".into())
        );
    }

    #[test]
    fn user_id_effort_and_parallel_flag_are_kept() {
        let mut v = base();
        v["metadata"] = json!({"user_id": "{\"device_id\":\"d\"}"});
        v["output_config"] = json!({"effort": "max"});
        v["tool_choice"] = json!({"type": "auto", "disable_parallel_tool_use": true});
        let request = decode_json(v).unwrap();
        assert_eq!(request.user.as_deref(), Some("{\"device_id\":\"d\"}"));
        assert_eq!(request.reasoning_effort.as_deref(), Some("high"));
        assert!(request.disable_parallel_tool_calls);

        let mut v = base();
        v["output_config"] = json!({"effort": "low"});
        v["tool_choice"] = json!({"type": "auto", "disable_parallel_tool_use": false});
        let request = decode_json(v).unwrap();
        assert_eq!(request.reasoning_effort.as_deref(), Some("low"));
        assert!(!request.disable_parallel_tool_calls);
        assert_eq!(decode_json(base()).unwrap().user, None);
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
        v["messages"] = json!([{"role": "tool", "content": "x"}]);
        assert_eq!(
            decode_json(v),
            Err(DecodeError::Role {
                index: 0,
                role: "tool".into()
            })
        );
        let mut v = base();
        v["messages"] = json!([{"role": "user", "content": [{"type": "text"}]}]);
        assert_eq!(decode_json(v), Err(DecodeError::Field("text")));
    }

    #[test]
    fn a_tool_reference_becomes_the_definition_tool_search_promised() {
        let mut v = base();
        v["tools"] = json!([
            {"name": "ToolSearch", "input_schema": {"type": "object"}},
            {"name": "mcp__x__grep", "description": "Grep", "input_schema": {"type": "object", "properties": {"q": {"type": "string"}}}, "defer_loading": true},
            {"name": "mcp__x__ls", "input_schema": {"type": "object"}, "defer_loading": true}
        ]);
        v["messages"] = json!([{"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t", "content": [
                {"type": "tool_reference", "tool_name": "mcp__x__grep"},
                {"type": "tool_reference", "tool_name": "mcp__x__ls"}
            ]}
        ]}]);
        let request = decode_json(v).unwrap();
        assert_eq!(
            request.messages[0].parts,
            vec![Part::ToolResult {
                tool_use_id: "t".into(),
                content: concat!(
                    "<functions>\n",
                    "<function>{\"description\":\"Grep\",\"name\":\"mcp__x__grep\",\"parameters\":{\"type\":\"object\",\"properties\":{\"q\":{\"type\":\"string\"}}}}</function>\n",
                    "<function>{\"name\":\"mcp__x__ls\",\"parameters\":{\"type\":\"object\"}}</function>\n",
                    "</functions>"
                )
                .into(),
                images: vec![],
                is_error: false,
            }]
        );
    }

    #[test]
    fn a_tool_reference_to_a_tool_not_sent_keeps_its_name() {
        let mut v = base();
        v["messages"] = json!([{"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t", "content": [
                {"type": "text", "text": "found:"},
                {"type": "tool_reference", "tool_name": "gone"}
            ]}
        ]}]);
        let request = decode_json(v).unwrap();
        assert_eq!(
            request.messages[0].parts,
            vec![Part::ToolResult {
                tool_use_id: "t".into(),
                content:
                    "found:\n\n<functions>\n<function>{\"name\":\"gone\"}</function>\n</functions>"
                        .into(),
                images: vec![],
                is_error: false,
            }]
        );
    }

    #[test]
    fn a_definition_is_spelled_out_once_and_named_after_that() {
        let mut v = base();
        v["tools"] = json!([
            {"name": "mcp__x__grep", "input_schema": {"type": "object"}, "defer_loading": true}
        ]);
        v["messages"] = json!([
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "a", "content": [
                {"type": "tool_reference", "tool_name": "mcp__x__grep"},
                {"type": "tool_reference", "tool_name": "mcp__x__grep"}
            ]}]},
            {"role": "assistant", "content": "ok"},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "b", "content": [
                {"type": "tool_reference", "tool_name": "mcp__x__grep"}
            ]}]}
        ]);
        let request = decode_json(v).unwrap();
        let text = |index: usize| match &request.messages[index].parts[0] {
            Part::ToolResult { content, .. } => content.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            text(0),
            "<functions>\n<function>{\"name\":\"mcp__x__grep\",\"parameters\":{\"type\":\"object\"}}</function>\n</functions>"
        );
        assert_eq!(
            text(2),
            "<functions>\n<function>{\"name\":\"mcp__x__grep\"}</function>\n</functions>"
        );
    }

    #[test]
    fn a_close_tag_in_a_definition_cannot_end_the_function_line() {
        let mut v = base();
        v["tools"] = json!([
            {"name": "a</function>", "description": "d</functions>", "input_schema": {}, "defer_loading": true}
        ]);
        v["messages"] = json!([{"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t", "content": [
                {"type": "tool_reference", "tool_name": "a</function>"}
            ]}
        ]}]);
        let request = decode_json(v).unwrap();
        let Part::ToolResult { content, .. } = &request.messages[0].parts[0] else {
            panic!()
        };
        assert!(!content.contains("</function>{"), "{content}");
        assert!(!content.contains("d</functions>"), "{content}");
        let line = content.lines().nth(1).unwrap();
        let json = &line["<function>".len()..line.len() - "</function>".len()];
        let parsed: Value = serde_json::from_str(json).unwrap();
        assert_eq!(parsed["name"], "a</function>");
        assert_eq!(parsed["description"], "d</functions>");
    }

    #[test]
    fn a_tool_reference_outside_a_tool_result_does_not_count() {
        let mut v = base();
        v["tools"] = json!([
            {"name": "mcp__x__grep", "input_schema": {"type": "object"}, "defer_loading": true}
        ]);
        v["messages"] = json!([{"role": "user", "content": [
            {"type": "text", "text": "hi", "content": [{"type": "tool_reference", "tool_name": "mcp__x__grep"}]}
        ]}]);
        assert!(decode_json(v).unwrap().tools.is_empty());
    }

    #[test]
    fn client_text_in_an_error_is_cut_and_escaped() {
        let long = "x".repeat(10_000);
        let mut v = base();
        v["messages"] = json!([{"role": format!("bad\n{long}"), "content": "hi"}]);
        let message = decode_json(v).unwrap_err().to_string();
        assert!(message.len() < 200, "{}", message.len());
        assert!(!message.contains('\n'));
        let mut v = base();
        v["tools"] = json!([{"name": long}]);
        assert!(decode_json(v).unwrap_err().to_string().len() < 200);
        let mut v = base();
        v["messages"] = json!([{"role": "user", "content": [{"type": long}]}]);
        assert!(decode_json(v).unwrap_err().to_string().len() < 200);
    }

    #[test]
    fn deferred_tools_are_sent_only_once_referenced() {
        let mut v = base();
        v["tools"] = json!([
            {"name": "ToolSearch", "input_schema": {"type": "object"}},
            {"name": "DeferredToolPlaceholder", "input_schema": {"type": "object"}, "defer_loading": true},
            {"name": "mcp__x__grep", "input_schema": {"type": "object"}, "defer_loading": true}
        ]);
        v["messages"] = json!([
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": [{"type": "tool_use", "id": "t", "name": "ToolSearch", "input": {}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t", "content": [
                {"type": "tool_reference", "tool_name": "mcp__x__grep"}
            ]}]}
        ]);
        let names: Vec<String> = decode_json(v)
            .unwrap()
            .tools
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert_eq!(names, ["ToolSearch", "mcp__x__grep"]);
    }
}
