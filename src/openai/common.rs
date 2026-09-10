//! What a streamed chunk and a completed document have in common: the
//! object envelope, the choice, its content, tool calls, usage and error
//! documents.

use serde_json::{Map, Value};

use crate::ir::{Event, StopReason, Usage};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("not a JSON object: {0}")]
    NotJson(String),
}

pub(super) fn parse_object(text: &[u8]) -> Result<Map<String, Value>, ParseError> {
    match serde_json::from_slice::<Value>(text) {
        Ok(Value::Object(fields)) => Ok(fields),
        Ok(_) => Err(ParseError::NotJson("not an object".to_owned())),
        Err(error) => Err(ParseError::NotJson(error.to_string())),
    }
}

pub(super) fn start_event(root: &Map<String, Value>) -> Event {
    Event::Start {
        id: str_field(root, "id").unwrap_or_default().to_owned(),
        model: str_field(root, "model").unwrap_or_default().to_owned(),
    }
}

pub(super) fn first_choice(root: &Map<String, Value>) -> Option<&Value> {
    root.get("choices").and_then(Value::as_array)?.first()
}

/// Reasoning first, then a refusal, then text; empty strings yield nothing.
pub(super) fn content_events(delta: &Map<String, Value>, events: &mut Vec<Event>) {
    let reasoning = delta
        .get("reasoning_content")
        .or_else(|| delta.get("reasoning"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !reasoning.is_empty() {
        events.push(Event::ThinkingDelta(reasoning.to_owned()));
    }
    if let Some(refusal) = str_field(delta, "refusal")
        && !refusal.is_empty()
    {
        events.push(Event::TextDelta(refusal.to_owned()));
    }
    match delta.get("content") {
        Some(Value::String(text)) if !text.is_empty() => {
            events.push(Event::TextDelta(text.clone()));
        }
        Some(Value::Array(parts)) => {
            for part in parts {
                if part.get("type").and_then(Value::as_str) == Some("text")
                    && let Some(text) = part.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    events.push(Event::TextDelta(text.to_owned()));
                }
            }
        }
        _ => {}
    }
}

/// The fields of one `tool_calls` entry; every one of them is optional on
/// some server.
pub(super) struct ToolCall<'a> {
    pub index: Option<u32>,
    pub id: Option<&'a str>,
    pub name: Option<&'a str>,
    pub arguments: &'a str,
}

pub(super) fn tool_call(call: &Value) -> ToolCall<'_> {
    let function = call.get("function");
    ToolCall {
        index: call.get("index").and_then(Value::as_u64).map(|i| i as u32),
        id: call.get("id").and_then(Value::as_str),
        name: function.and_then(|f| f.get("name")).and_then(Value::as_str),
        arguments: function
            .and_then(|f| f.get("arguments"))
            .and_then(Value::as_str)
            .unwrap_or(""),
    }
}

/// A started call: its `ToolCallStart`, then its arguments when it has any.
pub(super) fn push_tool_call(
    index: u32,
    id: Option<String>,
    name: &str,
    arguments: String,
    events: &mut Vec<Event>,
) {
    events.push(Event::ToolCallStart {
        index,
        id: id.unwrap_or_else(|| format!("call_{}", uuid::Uuid::new_v4().simple())),
        name: name.to_owned(),
    });
    if !arguments.is_empty() {
        events.push(Event::ToolCallDelta { index, arguments });
    }
}

pub(super) fn stop_reason(finish_reason: &str) -> StopReason {
    match finish_reason {
        "length" => StopReason::MaxTokens,
        "tool_calls" | "function_call" => StopReason::ToolUse,
        "content_filter" => StopReason::Refusal,
        _ => StopReason::EndTurn,
    }
}

/// `prompt_tokens` counts cached tokens too; the IR keeps them apart, as
/// the Anthropic API does.
pub(super) fn usage_event(root: &Map<String, Value>) -> Option<Event> {
    let usage = root.get("usage")?.as_object()?;
    let count = |field: &str| usage.get(field).and_then(Value::as_u64).unwrap_or(0);
    let detail = |group: &str, field: &str| {
        usage
            .get(group)
            .and_then(|g| g.get(field))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let cache_read_tokens = detail("prompt_tokens_details", "cached_tokens");
    Some(Event::Usage(Usage {
        input_tokens: count("prompt_tokens").saturating_sub(cache_read_tokens),
        output_tokens: count("completion_tokens"),
        cache_read_tokens,
        thinking_tokens: detail("completion_tokens_details", "reasoning_tokens"),
    }))
}

/// The message of an error document: OpenAI's `{"error": …}` (a null
/// `error` is not one) or vLLM's `{"object": "error", "message": …}`.
pub(super) fn error_document(root: &Map<String, Value>) -> Option<String> {
    match root.get("error") {
        Some(error) if !error.is_null() => return Some(error_text(error)),
        _ => {}
    }
    if str_field(root, "object") == Some("error") {
        return Some(
            str_field(root, "message")
                .map(str::to_owned)
                .unwrap_or_else(|| Value::Object(root.clone()).to_string()),
        );
    }
    None
}

/// `error.message`, a bare error string, or the error value itself.
fn error_text(error: &Value) -> String {
    match error {
        Value::String(text) => text.clone(),
        Value::Object(fields) => match fields.get("message") {
            Some(Value::String(text)) => text.clone(),
            _ => error.to_string(),
        },
        other => other.to_string(),
    }
}

fn str_field<'a>(object: &'a Map<String, Value>, field: &str) -> Option<&'a str> {
    object.get(field).and_then(Value::as_str)
}
