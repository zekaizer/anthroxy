//! Completed `chat.completion` document → IR events, the same sequence a
//! stream of the same content would produce.

use serde_json::Value;

use super::common::{
    ParseError, content_events, error_document, first_choice, parse_object, push_tool_call,
    start_event, stop_reason, tool_call, usage_event,
};
use crate::ir::Event;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ResponseError {
    #[error("response is {0}")]
    Parse(#[from] ParseError),
    #[error("response has no choices")]
    NoChoices,
    /// A 2xx body that is an error document.
    #[error("backend reported: {0}")]
    Backend(String),
}

pub fn decode(body: &[u8]) -> Result<Vec<Event>, ResponseError> {
    let root = parse_object(body)?;
    if let Some(message) = error_document(&root) {
        return Ok(vec![Event::Error(message)]);
    }
    let choice = first_choice(&root).ok_or(ResponseError::NoChoices)?;
    let mut events = vec![start_event(&root)];
    if let Some(message) = choice.get("message").and_then(Value::as_object) {
        content_events(message, &mut events);
        let calls = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for (index, call) in (0u32..).zip(calls) {
            let call = tool_call(call);
            match call.name {
                Some(name) => push_tool_call(
                    index,
                    call.id.map(str::to_owned),
                    name,
                    call.arguments.to_owned(),
                    &mut events,
                ),
                None => events.push(Event::Error(format!(
                    "tool call {index} has no function name"
                ))),
            }
        }
    }
    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        events.push(Event::Finish(stop_reason(reason)));
    }
    if let Some(usage) = usage_event(&root) {
        events.push(usage);
    }
    Ok(events)
}
