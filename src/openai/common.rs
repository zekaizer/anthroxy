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
        // An index the IR cannot hold is no index: the call is numbered
        // after the ones before it rather than folded into one of them.
        index: call
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|i| u32::try_from(i).ok()),
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

/// Where a backend speaking this wire format puts the count of prompt
/// tokens it read from a cache. Every one of them counts those tokens inside
/// `prompt_tokens`, so the IR subtracts them to reach the Anthropic meaning.
const CACHE_READ: [&str; 4] = [
    // OpenAI, Azure, vLLM, SGLang, Groq, OpenRouter, llama.cpp, Ollama.
    "prompt_tokens_details.cached_tokens",
    // DeepSeek, whose `prompt_tokens` is documented as hit + miss.
    "prompt_cache_hit_tokens",
    // Together, on the models that report it flat.
    "cached_tokens",
    // LiteLLM proxying Anthropic keeps Anthropic's name while recomputing
    // `prompt_tokens` to the OpenAI meaning, so this is still inclusive.
    "cache_read_input_tokens",
];

/// The same for tokens written to a cache, which are likewise inside
/// `prompt_tokens` here and outside `input_tokens` in the Anthropic sense.
const CACHE_WRITE: [&str; 4] = [
    // OpenAI, Azure, OpenRouter.
    "prompt_tokens_details.cache_write_tokens",
    // vLLM's local prefix-cache write.
    "prompt_tokens_details.created_cache_tokens",
    // LiteLLM.
    "prompt_tokens_details.cache_creation_tokens",
    "cache_creation_input_tokens",
];

/// `prompt_tokens` counts cached tokens too; the IR keeps them apart, as
/// the Anthropic API does.
pub(super) fn usage_event(root: &Map<String, Value>) -> Option<Event> {
    let usage = root.get("usage")?.as_object()?;
    let count = |field: &str| usage.get(field).and_then(Value::as_u64).unwrap_or(0);
    let at = |path: &str| -> Option<u64> {
        let mut value = usage.get(path.split('.').next()?)?;
        for step in path.split('.').skip(1) {
            value = value.get(step)?;
        }
        value.as_u64()
    };
    // The largest of the names a backend might use, not their sum: LiteLLM
    // sends the same count twice, once in each dialect's name.
    let largest = |paths: &[&str]| paths.iter().filter_map(|path| at(path)).max();
    let cache_read_tokens = largest(&CACHE_READ);
    let cache_creation_tokens = largest(&CACHE_WRITE);
    let cached = cache_read_tokens
        .unwrap_or(0)
        .saturating_add(cache_creation_tokens.unwrap_or(0));
    Some(Event::Usage(Usage {
        input_tokens: count("prompt_tokens").saturating_sub(cached),
        output_tokens: count("completion_tokens"),
        cache_read_tokens: cache_read_tokens.unwrap_or(0),
        cache_creation_tokens: cache_creation_tokens.unwrap_or(0),
        // A backend that named none of these said nothing about caching,
        // which is not the same as saying it cached nothing.
        cache_reported: cache_read_tokens.is_some() || cache_creation_tokens.is_some(),
        thinking_tokens: at("completion_tokens_details.reasoning_tokens").unwrap_or(0),
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

/// The names an error document gives its failure, most specific first:
/// `error.type` and `error.code`, or the same fields on a bare error object.
pub(super) fn error_names(root: &Map<String, Value>) -> impl Iterator<Item = &str> {
    let error = match root.get("error") {
        Some(Value::Object(fields)) => fields,
        _ => root,
    };
    ["type", "code"]
        .into_iter()
        .filter_map(|field| str_field(error, field))
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
