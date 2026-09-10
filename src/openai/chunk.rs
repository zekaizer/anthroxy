//! Streaming chunk (`data:` payload) → IR events.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::ir::{Event, StopReason, Usage};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ChunkError {
    #[error("chunk is not a JSON object: {0}")]
    NotJson(String),
}

/// Decodes one `data:` payload at a time, carrying the tool-call state a
/// stream needs: which indexes have started, and deltas that arrived before
/// the call's name did.
#[derive(Debug, Default)]
pub struct ChunkDecoder {
    started: bool,
    known: BTreeSet<u32>,
    pending: BTreeMap<u32, Pending>,
    /// Next index for a call the backend did not number.
    unnumbered: u32,
    /// The call an unnumbered delta without `id` or name continues.
    last: Option<u32>,
}

#[derive(Debug, Default)]
struct Pending {
    id: Option<String>,
    arguments: String,
}

impl ChunkDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// End of stream: calls still waiting for a name are reported, since
    /// the client would otherwise see a `tool_use` stop with no tool.
    pub fn finish(&mut self) -> Vec<Event> {
        let pending = std::mem::take(&mut self.pending);
        pending
            .into_keys()
            .map(|index| Event::Error(format!("tool call {index} never received a function name")))
            .collect()
    }

    pub fn decode(&mut self, data: &str) -> Result<Vec<Event>, ChunkError> {
        let data = data.trim();
        if data == "[DONE]" {
            return Ok(vec![Event::Done]);
        }
        let root: Value =
            serde_json::from_str(data).map_err(|e| ChunkError::NotJson(e.to_string()))?;
        let root = root
            .as_object()
            .ok_or_else(|| ChunkError::NotJson("not an object".to_owned()))?;
        if let Some(message) = error_document(root) {
            return Ok(vec![Event::Error(message)]);
        }
        let mut events = Vec::new();
        if !self.started {
            self.started = true;
            events.push(start_event(root));
        }
        if let Some(choice) = first_choice(root) {
            if let Some(delta) = choice.get("delta").and_then(Value::as_object) {
                content_events(delta, &mut events);
                if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        self.tool_call(call, &mut events);
                    }
                }
            }
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                events.push(Event::Finish(stop_reason(reason)));
            }
        }
        if let Some(usage) = usage_event(root) {
            events.push(usage);
        }
        Ok(events)
    }

    fn tool_call(&mut self, call: &Value, events: &mut Vec<Event>) {
        let function = call.get("function");
        let names_a_call = call.get("id").is_some_and(Value::is_string)
            || function
                .and_then(|f| f.get("name"))
                .is_some_and(Value::is_string);
        let index = match call.get("index").and_then(Value::as_u64) {
            Some(index) => index as u32,
            None => match self.last {
                Some(last) if !names_a_call => last,
                _ => {
                    self.unnumbered += 1;
                    self.unnumbered - 1
                }
            },
        };
        self.last = Some(index);
        let arguments = function
            .and_then(|f| f.get("arguments"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if self.known.contains(&index) {
            if !arguments.is_empty() {
                events.push(Event::ToolCallDelta {
                    index,
                    arguments: arguments.to_owned(),
                });
            }
            return;
        }
        let pending = self.pending.entry(index).or_default();
        if let Some(id) = call.get("id").and_then(Value::as_str) {
            pending.id.get_or_insert_with(|| id.to_owned());
        }
        pending.arguments.push_str(arguments);
        let Some(name) = function.and_then(|f| f.get("name")).and_then(Value::as_str) else {
            return;
        };
        let pending = self.pending.remove(&index).expect("inserted above");
        self.known.insert(index);
        events.push(Event::ToolCallStart {
            index,
            id: pending.id.unwrap_or_else(generated_id),
            name: name.to_owned(),
        });
        if !pending.arguments.is_empty() {
            events.push(Event::ToolCallDelta {
                index,
                arguments: pending.arguments,
            });
        }
    }
}

pub(super) fn start_event(root: &serde_json::Map<String, Value>) -> Event {
    let text = |field: &str| {
        root.get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    Event::Start {
        id: text("id"),
        model: text("model"),
    }
}

pub(super) fn first_choice(root: &serde_json::Map<String, Value>) -> Option<&Value> {
    root.get("choices").and_then(Value::as_array)?.first()
}

/// Reasoning first, then text; empty strings yield nothing.
pub(super) fn content_events(delta: &serde_json::Map<String, Value>, events: &mut Vec<Event>) {
    let reasoning = delta
        .get("reasoning_content")
        .or_else(|| delta.get("reasoning"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !reasoning.is_empty() {
        events.push(Event::ThinkingDelta(reasoning.to_owned()));
    }
    if let Some(refusal) = delta.get("refusal").and_then(Value::as_str)
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
pub(super) fn usage_event(root: &serde_json::Map<String, Value>) -> Option<Event> {
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

pub(super) fn generated_id() -> String {
    format!("call_{}", uuid::Uuid::new_v4().simple())
}

/// The message of an error document: OpenAI's `{"error": …}` (a null
/// `error` is not one) or vLLM's `{"object": "error", "message": …}`.
pub(super) fn error_document(root: &serde_json::Map<String, Value>) -> Option<String> {
    match root.get("error") {
        Some(error) if !error.is_null() => return Some(error_text(error)),
        _ => {}
    }
    if root.get("object").and_then(Value::as_str) == Some("error") {
        return Some(
            root.get("message")
                .and_then(Value::as_str)
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
