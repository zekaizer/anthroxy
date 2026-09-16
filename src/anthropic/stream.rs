//! IR events → Messages API server-sent events, one frame at a time.

use std::collections::BTreeMap;
use std::fmt::Write;

use serde_json::{Value, json};

use super::fragments::{identity, stop_reason_name, usage_json};
use crate::ir::{Event, StopReason, Usage};

/// Turns events into SSE frames as they arrive. Block indexes follow arrival
/// order; a block closes when a different kind of content starts. `Done`
/// (or end of input) closes the message; an error closes it without a
/// `message_stop`, and nothing is emitted after that.
#[derive(Debug)]
pub struct StreamEncoder {
    fallback_model: String,
    state: State,
    next_index: u32,
    open: Option<Open>,
    /// Tool call index → content block index, for every call started.
    tools: BTreeMap<u32, u32>,
    stop: StopReason,
    usage: Usage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// No `message_start` yet.
    Idle,
    Open,
    /// `message_stop` or an error went out; nothing follows.
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    Thinking(u32),
    Text(u32),
    Tool { call: u32, block: u32 },
}

impl Open {
    fn block(self) -> u32 {
        match self {
            Open::Thinking(block) | Open::Text(block) | Open::Tool { block, .. } => block,
        }
    }
}

/// The two streaming text kinds, which differ only in their JSON names.
#[derive(Debug, Clone, Copy)]
enum TextKind {
    Thinking,
    Text,
}

impl StreamEncoder {
    /// `fallback_model` is reported when the backend names none.
    pub fn new(fallback_model: impl Into<String>) -> Self {
        Self {
            fallback_model: fallback_model.into(),
            state: State::Idle,
            next_index: 0,
            open: None,
            tools: BTreeMap::new(),
            stop: StopReason::EndTurn,
            usage: Usage::default(),
        }
    }

    /// Appends the frames for every event to `out`.
    pub fn encode_all(&mut self, events: impl IntoIterator<Item = Event>, out: &mut String) {
        for event in events {
            self.encode(event, out);
        }
    }

    /// Appends the frames for `event` to `out`.
    pub fn encode(&mut self, event: Event, out: &mut String) {
        if self.state == State::Closed {
            return;
        }
        match event {
            Event::Start { id, model } => self.start(&id, &model, out),
            Event::ThinkingDelta(text) => self.text_delta(TextKind::Thinking, &text, out),
            Event::TextDelta(text) => self.text_delta(TextKind::Text, &text, out),
            Event::ToolCallStart { index, id, name } => {
                self.start("", "", out);
                let block = self.open_block(
                    json!({"type": "tool_use", "id": id, "name": name, "input": {}}),
                    out,
                );
                self.tools.insert(index, block);
                self.open = Some(Open::Tool { call: index, block });
            }
            Event::ToolCallDelta { index, arguments } => {
                let Some(&block) = self.tools.get(&index) else {
                    tracing::warn!(index, "tool call arguments for a call that never started");
                    return;
                };
                if self.open != Some(Open::Tool { call: index, block }) {
                    tracing::warn!(index, "tool call arguments arrived after its block closed");
                }
                delta(
                    block,
                    json!({"type": "input_json_delta", "partial_json": arguments}),
                    out,
                );
            }
            Event::Finish(reason) => {
                self.close_open(out);
                self.stop = reason;
            }
            Event::Usage(usage) => self.usage = usage,
            Event::Error(message) => self.error(&message, out),
            Event::Done => self.finish(out),
        }
    }

    /// End of input without `Done`: closes what is open.
    pub fn finish(&mut self, out: &mut String) {
        match self.state {
            State::Closed => return,
            State::Idle => {
                self.error("backend closed the stream without a response", out);
                return;
            }
            State::Open => {}
        }
        self.close_open(out);
        let stop_reason = self.stop.resolve(!self.tools.is_empty());
        frame(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason_name(stop_reason), "stop_sequence": null},
                "usage": usage_json(&self.usage),
            }),
            out,
        );
        frame("message_stop", json!({"type": "message_stop"}), out);
        self.state = State::Closed;
    }

    /// One `error` frame; `message` is shown to the user as is.
    pub fn error(&mut self, message: &str, out: &mut String) {
        if self.state == State::Closed {
            return;
        }
        frame(
            "error",
            json!({"type": "error", "error": {"type": "api_error", "message": message}}),
            out,
        );
        self.state = State::Closed;
    }

    /// Opens the message unless it is open already, so a delta before any
    /// `Start` still gets a `message_start`.
    fn start(&mut self, id: &str, model: &str, out: &mut String) {
        if self.state != State::Idle {
            return;
        }
        self.state = State::Open;
        let (id, model) = identity(id, model, &self.fallback_model);
        frame(
            "message_start",
            json!({
                "type": "message_start",
                "message": {
                    "id": id,
                    "type": "message",
                    "role": "assistant",
                    "model": model,
                    "content": [],
                    "stop_reason": null,
                    "stop_sequence": null,
                    "usage": {"input_tokens": 0, "output_tokens": 0},
                },
            }),
            out,
        );
    }

    fn text_delta(&mut self, kind: TextKind, text: &str, out: &mut String) {
        if text.is_empty() {
            return;
        }
        self.start("", "", out);
        let block = match (kind, self.open) {
            (TextKind::Thinking, Some(Open::Thinking(block)))
            | (TextKind::Text, Some(Open::Text(block))) => block,
            (TextKind::Thinking, _) => {
                let block = self.open_block(json!({"type": "thinking", "thinking": ""}), out);
                self.open = Some(Open::Thinking(block));
                block
            }
            (TextKind::Text, _) => {
                let block = self.open_block(json!({"type": "text", "text": ""}), out);
                self.open = Some(Open::Text(block));
                block
            }
        };
        let payload = match kind {
            TextKind::Thinking => json!({"type": "thinking_delta", "thinking": text}),
            TextKind::Text => json!({"type": "text_delta", "text": text}),
        };
        delta(block, payload, out);
    }

    /// Closes the open block and starts a new one; returns its index.
    fn open_block(&mut self, content_block: Value, out: &mut String) -> u32 {
        self.close_open(out);
        let block = self.next_index;
        self.next_index += 1;
        frame(
            "content_block_start",
            json!({"type": "content_block_start", "index": block, "content_block": content_block}),
            out,
        );
        block
    }

    fn close_open(&mut self, out: &mut String) {
        if let Some(open) = self.open.take() {
            frame(
                "content_block_stop",
                json!({"type": "content_block_stop", "index": open.block()}),
                out,
            );
        }
    }
}

fn delta(block: u32, delta: Value, out: &mut String) {
    frame(
        "content_block_delta",
        json!({"type": "content_block_delta", "index": block, "delta": delta}),
        out,
    );
}

fn frame(event: &str, data: Value, out: &mut String) {
    write!(out, "event: {event}\ndata: {data}\n\n").expect("writing to a String");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{StopReason, Usage};

    fn run(events: impl IntoIterator<Item = Event>) -> String {
        let mut e = StreamEncoder::new("fallback");
        let mut out = String::new();
        for event in events {
            e.encode(event, &mut out);
        }
        out
    }

    fn frame(event: &str, data: &str) -> String {
        format!("event: {event}\ndata: {data}\n\n")
    }

    fn start() -> Event {
        Event::Start {
            id: "chatcmpl-1".into(),
            model: "qwen".into(),
        }
    }

    #[test]
    fn reasoning_text_and_tool_calls_in_order() {
        let out = run([
            start(),
            Event::ThinkingDelta("plan".into()),
            Event::ThinkingDelta("!".into()),
            Event::TextDelta("Reading".into()),
            Event::ToolCallStart {
                index: 0,
                id: "call_a".into(),
                name: "read".into(),
            },
            Event::ToolCallDelta {
                index: 0,
                arguments: "{\"path\":".into(),
            },
            Event::ToolCallDelta {
                index: 0,
                arguments: "\"a\"}".into(),
            },
            Event::ToolCallStart {
                index: 1,
                id: "call_b".into(),
                name: "bash".into(),
            },
            Event::Finish(StopReason::ToolUse),
            Event::Usage(Usage {
                input_tokens: 12,
                output_tokens: 34,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cache_reported: false,
                thinking_tokens: 0,
            }),
            Event::Done,
        ]);
        let expected = [
            frame("message_start", r#"{"type":"message_start","message":{"id":"chatcmpl-1","type":"message","role":"assistant","model":"qwen","content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}}"#),
            frame("content_block_start", r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#),
            frame("content_block_delta", r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}"#),
            frame("content_block_delta", r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"!"}}"#),
            frame("content_block_stop", r#"{"type":"content_block_stop","index":0}"#),
            frame("content_block_start", r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#),
            frame("content_block_delta", r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Reading"}}"#),
            frame("content_block_stop", r#"{"type":"content_block_stop","index":1}"#),
            frame("content_block_start", r#"{"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"call_a","name":"read","input":{}}}"#),
            frame("content_block_delta", r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}}"#),
            frame("content_block_delta", r#"{"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"a\"}"}}"#),
            frame("content_block_stop", r#"{"type":"content_block_stop","index":2}"#),
            frame("content_block_start", r#"{"type":"content_block_start","index":3,"content_block":{"type":"tool_use","id":"call_b","name":"bash","input":{}}}"#),
            frame("content_block_stop", r#"{"type":"content_block_stop","index":3}"#),
            frame("message_delta", r#"{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"input_tokens":12,"output_tokens":34}}"#),
            frame("message_stop", r#"{"type":"message_stop"}"#),
        ]
        .concat();
        assert_eq!(out, expected);
    }

    #[test]
    fn stop_reason_rules() {
        let tail = |events: Vec<Event>| {
            let out = run([start(), Event::TextDelta("x".into())]
                .into_iter()
                .chain(events));
            out.rsplit("event: message_delta\n")
                .next()
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .to_owned()
        };
        assert_eq!(
            tail(vec![Event::Finish(StopReason::MaxTokens), Event::Done]),
            r#"data: {"type":"message_delta","delta":{"stop_reason":"max_tokens","stop_sequence":null},"usage":{"input_tokens":0,"output_tokens":0}}"#
        );
        assert!(tail(vec![Event::Done]).contains(r#""stop_reason":"end_turn""#));
        let with_tool = tail(vec![
            Event::ToolCallStart {
                index: 0,
                id: "c".into(),
                name: "t".into(),
            },
            Event::Finish(StopReason::EndTurn),
            Event::Done,
        ]);
        assert!(
            with_tool.contains(r#""stop_reason":"tool_use""#),
            "{with_tool}"
        );
    }

    #[test]
    fn empty_ids_fall_back_and_text_after_a_tool_opens_a_new_block() {
        let out = run([
            Event::Start {
                id: String::new(),
                model: String::new(),
            },
            Event::ToolCallStart {
                index: 0,
                id: "c".into(),
                name: "t".into(),
            },
            Event::TextDelta("after".into()),
            Event::Done,
        ]);
        let first = out.lines().nth(1).unwrap();
        assert!(first.contains(r#""id":"msg_"#), "{first}");
        assert!(first.contains(r#""model":"fallback""#), "{first}");
        assert!(out.contains(
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#
        ));
        assert!(out.ends_with(&frame("message_stop", r#"{"type":"message_stop"}"#)));
    }

    #[test]
    fn deltas_for_a_closed_or_unknown_tool_index() {
        let out = run([
            start(),
            Event::ToolCallStart {
                index: 0,
                id: "c".into(),
                name: "t".into(),
            },
            Event::TextDelta("x".into()),
            Event::ToolCallDelta {
                index: 0,
                arguments: "late".into(),
            },
            Event::ToolCallDelta {
                index: 9,
                arguments: "orphan".into(),
            },
            Event::Done,
        ]);
        assert!(out.contains(r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"late"}}"#), "{out}");
        assert!(!out.contains("orphan"), "{out}");
    }

    #[test]
    fn the_cache_counters_say_zero_rather_than_going_missing() {
        let with_cache = run([
            start(),
            Event::TextDelta("x".into()),
            Event::Usage(Usage {
                input_tokens: 10,
                output_tokens: 2,
                cache_read_tokens: 500,
                cache_creation_tokens: 0,
                cache_reported: true,
                thinking_tokens: 0,
            }),
            Event::Done,
        ]);
        assert!(
            with_cache.contains(
                r#""usage":{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":500,"cache_creation_input_tokens":0}"#
            ),
            "{with_cache}"
        );
        // A backend that reported no hits still answered the question, and
        // the zero is how the answer survives to whoever reads the body.
        let no_hits = run([
            start(),
            Event::TextDelta("x".into()),
            Event::Usage(Usage {
                input_tokens: 10,
                output_tokens: 2,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cache_reported: true,
                thinking_tokens: 0,
            }),
            Event::Done,
        ]);
        assert!(
            no_hits.contains(
                r#""usage":{"input_tokens":10,"output_tokens":2,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}"#
            ),
            "{no_hits}"
        );
        // A backend that was never asked leaves the counters out entirely.
        let without = run([start(), Event::TextDelta("x".into()), Event::Done]);
        assert!(
            without.contains(r#""usage":{"input_tokens":0,"output_tokens":0}"#),
            "{without}"
        );
    }

    #[test]
    fn thinking_tokens_and_refusals_are_reported() {
        let out = run([
            start(),
            Event::TextDelta("no".into()),
            Event::Finish(StopReason::Refusal),
            Event::Usage(Usage {
                input_tokens: 1,
                output_tokens: 9,
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
                cache_reported: false,
                thinking_tokens: 7,
            }),
            Event::Done,
        ]);
        assert!(out.contains(r#""stop_reason":"refusal""#), "{out}");
        assert!(
            out.contains(r#""usage":{"input_tokens":1,"output_tokens":9,"output_tokens_details":{"thinking_tokens":7}}"#),
            "{out}"
        );
    }

    #[test]
    fn errors_close_the_stream_for_good() {
        let mut e = StreamEncoder::new("m");
        let mut out = String::new();
        e.encode(start(), &mut out);
        e.encode(Event::TextDelta("partial".into()), &mut out);
        e.encode(Event::Error("overloaded".into()), &mut out);
        assert!(
            out.ends_with(&frame(
                "error",
                r#"{"type":"error","error":{"type":"api_error","message":"overloaded"}}"#
            )),
            "{out}"
        );
        assert!(
            !out.contains("content_block_stop"),
            "no closing after an error: {out}"
        );
        let len = out.len();
        e.encode(Event::TextDelta("more".into()), &mut out);
        e.encode(Event::Done, &mut out);
        e.finish(&mut out);
        e.error("again", &mut out);
        assert_eq!(out.len(), len, "nothing after the error");

        let mut before_start = String::new();
        StreamEncoder::new("m").error("gone", &mut before_start);
        assert_eq!(
            before_start,
            frame(
                "error",
                r#"{"type":"error","error":{"type":"api_error","message":"gone"}}"#
            )
        );
    }

    #[test]
    fn finish_closes_an_open_message_or_reports_a_missing_one() {
        let mut e = StreamEncoder::new("m");
        let mut out = String::new();
        e.encode(start(), &mut out);
        e.encode(Event::TextDelta("x".into()), &mut out);
        e.finish(&mut out);
        assert!(
            out.ends_with(&frame("message_stop", r#"{"type":"message_stop"}"#)),
            "{out}"
        );
        assert!(out.contains(r#"{"type":"content_block_stop","index":0}"#));

        let mut e = StreamEncoder::new("m");
        let mut out = String::new();
        e.finish(&mut out);
        assert!(out.starts_with("event: error\n"), "{out}");
        assert!(out.contains("without a response"), "{out}");
    }
}
