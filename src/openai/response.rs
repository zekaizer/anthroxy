//! Completed `chat.completion` document → IR events, the same sequence a
//! stream of the same content would produce.

use serde_json::Value;

use super::chunk::{
    content_events, error_text, first_choice, generated_id, start_event, stop_reason, usage_event,
};
use crate::ir::Event;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ResponseError {
    #[error("response is not a JSON object: {0}")]
    NotJson(String),
    #[error("response has no choices")]
    NoChoices,
}

pub fn decode(body: &[u8]) -> Result<Vec<Event>, ResponseError> {
    let root: Value =
        serde_json::from_slice(body).map_err(|e| ResponseError::NotJson(e.to_string()))?;
    let root = root
        .as_object()
        .ok_or_else(|| ResponseError::NotJson("not an object".to_owned()))?;
    if let Some(error) = root.get("error") {
        return Ok(vec![Event::Error(error_text(error))]);
    }
    let choice = first_choice(root).ok_or(ResponseError::NoChoices)?;
    let mut events = vec![start_event(root)];
    if let Some(message) = choice.get("message").and_then(Value::as_object) {
        content_events(message, &mut events);
        let calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for (index, call) in (0u32..).zip(calls) {
            let function = call.get("function");
            let Some(name) = function.and_then(|f| f.get("name")).and_then(Value::as_str) else {
                continue;
            };
            events.push(Event::ToolCallStart {
                index,
                id: call
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(generated_id),
                name: name.to_owned(),
            });
            let arguments = function
                .and_then(|f| f.get("arguments"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !arguments.is_empty() {
                events.push(Event::ToolCallDelta {
                    index,
                    arguments: arguments.to_owned(),
                });
            }
        }
    }
    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        events.push(Event::Finish(stop_reason(reason)));
    }
    if let Some(usage) = usage_event(root) {
        events.push(usage);
    }
    Ok(events)
}
