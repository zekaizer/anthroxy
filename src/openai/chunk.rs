//! Streaming chunk (`data:` payload) → IR events.

use std::collections::BTreeMap;

use serde_json::Value;

use super::common::{
    ParseError, content_events, error_document, first_choice, parse_object, push_tool_call,
    stop_reason, tool_call, usage_event,
};
use crate::ir::{Event, Failure};

/// Arguments held for tool calls still waiting for a function name. The SSE
/// parser caps one frame; nothing else caps what a stream accumulates across
/// them. Well above any call a backend names in its first delta, as this
/// wire format has one do.
pub const MAX_PENDING_ARGUMENTS: usize = 8 * 1024 * 1024;
/// Tool calls one stream may open.
pub const MAX_CALLS: usize = 256;

/// Decodes one `data:` payload at a time, carrying the tool-call state a
/// stream needs: which calls have started, and the deltas of a call whose
/// name has not arrived yet.
#[derive(Debug, Default)]
pub struct ChunkDecoder {
    started: bool,
    calls: BTreeMap<u32, Call>,
    /// The call an unnumbered delta without `id` or name continues.
    last: Option<u32>,
    /// Arguments held across every pending call.
    pending: usize,
    /// A limit was passed; the stream's tool calls are no longer followed.
    stopped: bool,
}

#[derive(Debug)]
enum Call {
    /// Seen, but without a name yet; what arrived so far.
    Pending {
        id: Option<String>,
        arguments: String,
    },
    Started,
}

impl ChunkDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// End of stream: calls still waiting for a name are reported, since
    /// the client would otherwise see a `tool_use` stop with no tool.
    pub fn finish(&mut self) -> Vec<Event> {
        let calls = std::mem::take(&mut self.calls);
        calls
            .into_iter()
            .filter(|(_, call)| matches!(call, Call::Pending { .. }))
            .map(|(index, _)| {
                Event::Error(Failure::upstream(format!(
                    "tool call {index} never received a function name"
                )))
            })
            .collect()
    }

    /// The events of one payload. `[DONE]` yields what [`Self::finish`]
    /// would, then `Done`.
    pub fn decode(&mut self, data: &str) -> Result<Vec<Event>, ParseError> {
        let data = data.trim();
        if data == "[DONE]" {
            let mut events = self.finish();
            events.push(Event::Done);
            return Ok(events);
        }
        let root = parse_object(data.as_bytes())?;
        if error_document(&root).is_some() {
            return Ok(vec![Event::Error(super::decode_error(
                None,
                data.as_bytes(),
            ))]);
        }
        let mut events = Vec::new();
        if !self.started {
            self.started = true;
            events.push(super::common::start_event(&root));
        }
        if let Some(choice) = first_choice(&root) {
            if let Some(delta) = choice.get("delta").and_then(Value::as_object) {
                content_events(delta, &mut events);
                if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        self.tool_call_delta(call, &mut events);
                    }
                }
            }
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                events.push(Event::Finish(stop_reason(reason)));
            }
        }
        if let Some(usage) = usage_event(&root) {
            events.push(usage);
        }
        Ok(events)
    }

    fn tool_call_delta(&mut self, call: &Value, events: &mut Vec<Event>) {
        if self.stopped {
            return;
        }
        let call = tool_call(call);
        let index = match call.index {
            Some(index) => index,
            // Unnumbered: a delta with neither id nor name continues the
            // latest call; anything else is a new one after it.
            None => match self.last {
                Some(last) if call.id.is_none() && call.name.is_none() => last,
                _ => self.calls.keys().next_back().map_or(0, |last| last + 1),
            },
        };
        self.last = Some(index);
        if !self.calls.contains_key(&index) && self.calls.len() == MAX_CALLS {
            self.stop_following(
                format!("the stream opened more than {MAX_CALLS} tool calls"),
                events,
            );
            return;
        }
        let entry = self.calls.entry(index).or_insert(Call::Pending {
            id: None,
            arguments: String::new(),
        });
        let Call::Pending { id, arguments } = entry else {
            if !call.arguments.is_empty() {
                events.push(Event::ToolCallDelta {
                    index,
                    arguments: call.arguments.to_owned(),
                });
            }
            return;
        };
        if let Some(new_id) = call.id {
            id.get_or_insert_with(|| new_id.to_owned());
        }
        arguments.push_str(call.arguments);
        self.pending += call.arguments.len();
        let Some(name) = call.name else {
            if self.pending > MAX_PENDING_ARGUMENTS {
                self.stop_following(
                    format!(
                        "tool call {index} held more than {MAX_PENDING_ARGUMENTS} bytes of arguments without a function name"
                    ),
                    events,
                );
            }
            return;
        };
        let (id, arguments) = (id.take(), std::mem::take(arguments));
        self.pending -= arguments.len();
        *entry = Call::Started;
        push_tool_call(index, id, name, arguments, events);
    }

    /// Stops following tool calls and reports why: what is held is released,
    /// and the error closes the stream for the client.
    fn stop_following(&mut self, why: String, events: &mut Vec<Event>) {
        self.stopped = true;
        self.calls.clear();
        self.pending = 0;
        events.push(Event::Error(Failure::upstream(why)));
    }
}
