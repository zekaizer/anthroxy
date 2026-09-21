//! Token usage and failures in a Messages response as the client receives
//! it, event stream or document. Observes bytes; never changes them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sse::{Frame, Parser};

/// Token counts in the Anthropic sense: `input` excludes what was read from
/// or written to a prompt cache.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    /// The backend said something about caching, even if it said zero. A
    /// backend that says nothing is not one that cached nothing, and the
    /// difference decides whether a hit rate can be quoted at all.
    /// Absent from lines written before the field existed, where a non-zero
    /// count is itself the answer; [`TokenUsage::cache_known`] reads both.
    #[serde(default)]
    pub cache_reported: bool,
}

impl TokenUsage {
    /// Whether anything is known about caching for this exchange.
    pub fn cache_known(&self) -> bool {
        self.cache_reported || self.cache_read > 0 || self.cache_creation > 0
    }

    /// Prompt tokens in all: what was sent fresh, read from the cache and
    /// written to it.
    pub fn prompt(&self) -> u64 {
        self.input
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_creation)
    }
}

/// What a scan found once the body ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scan {
    /// `None` when the response carried no usage.
    pub usage: Option<TokenUsage>,
    /// An `error` event or error document: `<type>: <message>`.
    pub error: Option<String>,
}

/// A document larger than this is not kept for scanning.
pub const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

pub struct UsageScanner {
    body: Body,
    usage: Option<TokenUsage>,
    error: Option<String>,
}

enum Body {
    Events(Parser),
    /// `None` once the document outgrew [`MAX_DOCUMENT_BYTES`].
    Document(Option<Vec<u8>>),
    /// The stream stopped being readable; nothing more is scanned.
    Unreadable,
}

impl UsageScanner {
    /// For a body with this `content-type`: an event stream when it says so,
    /// a document otherwise.
    pub fn for_content_type(content_type: Option<&str>) -> Self {
        let events = content_type.is_some_and(|t| {
            t.trim_start()
                .to_ascii_lowercase()
                .starts_with("text/event-stream")
        });
        Self {
            body: if events {
                Body::Events(Parser::new())
            } else {
                Body::Document(Some(Vec::new()))
            },
            usage: None,
            error: None,
        }
    }

    pub fn feed(&mut self, chunk: &[u8]) {
        match &mut self.body {
            Body::Events(parser) => match parser.feed(chunk) {
                Ok(frames) => self.frames(frames),
                Err(_) => self.body = Body::Unreadable,
            },
            Body::Document(kept) => {
                if let Some(bytes) = kept {
                    if bytes.len() + chunk.len() > MAX_DOCUMENT_BYTES {
                        *kept = None;
                    } else {
                        bytes.extend_from_slice(chunk);
                    }
                }
            }
            Body::Unreadable => {}
        }
    }

    pub fn finish(mut self) -> Scan {
        match std::mem::replace(&mut self.body, Body::Unreadable) {
            Body::Events(mut parser) => {
                if let Ok(frames) = parser.finish() {
                    self.frames(frames);
                }
            }
            Body::Document(Some(bytes)) => {
                if let Ok(document) = serde_json::from_slice::<Value>(&bytes) {
                    if document.get("type").and_then(Value::as_str) == Some("error") {
                        self.error = error_text(&document);
                    } else {
                        self.overlay(document.get("usage"));
                    }
                }
            }
            Body::Document(None) | Body::Unreadable => {}
        }
        Scan {
            usage: self.usage,
            error: self.error,
        }
    }

    /// Only frames that can carry usage or an error are parsed.
    fn frames(&mut self, frames: Vec<Frame>) {
        for frame in frames {
            if !(frame.data.contains("\"usage\"") || frame.data.contains("\"error\"")) {
                continue;
            }
            let Ok(event) = serde_json::from_str::<Value>(&frame.data) else {
                continue;
            };
            match event.get("type").and_then(Value::as_str) {
                Some("message_start") => self.overlay(event.pointer("/message/usage")),
                Some("message_delta") => self.overlay(event.get("usage")),
                Some("error") => self.error = error_text(&event),
                _ => {}
            }
        }
    }

    /// Counts are cumulative, so a later value replaces an earlier one; a
    /// field the event leaves out keeps its value.
    fn overlay(&mut self, usage: Option<&Value>) {
        let Some(fields) = usage.and_then(Value::as_object) else {
            return;
        };
        let totals = self.usage.get_or_insert_with(TokenUsage::default);
        for (key, slot) in [
            ("input_tokens", &mut totals.input),
            ("output_tokens", &mut totals.output),
            ("cache_read_input_tokens", &mut totals.cache_read),
            ("cache_creation_input_tokens", &mut totals.cache_creation),
        ] {
            if let Some(count) = fields.get(key).and_then(Value::as_u64) {
                *slot = count;
            }
        }
        // The flat total is the sum of the nested breakdown, so the nested one
        // counts only where there is no flat field to count instead.
        if !fields.contains_key("cache_creation_input_tokens")
            && let Some(nested) = fields.get("cache_creation").and_then(Value::as_object)
        {
            totals.cache_creation = nested
                .values()
                .filter_map(Value::as_u64)
                .fold(0, u64::saturating_add);
        }
        // Naming a cache field is the answer, whatever the number in it.
        if [
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
            "cache_creation",
        ]
        .iter()
        .any(|key| fields.contains_key(*key))
        {
            totals.cache_reported = true;
        }
    }
}

/// `<type>: <message>` of an Anthropic error event or document.
fn error_text(document: &Value) -> Option<String> {
    let error = document.get("error")?;
    let kind = error.get("type").and_then(Value::as_str);
    let message = error.get("message").and_then(Value::as_str);
    match (kind, message) {
        (Some(kind), Some(message)) => Some(format!("{kind}: {message}")),
        (None, Some(text)) | (Some(text), None) => Some(text.to_owned()),
        (None, None) => Some(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SSE: Option<&str> = Some("text/event-stream");
    const JSON: Option<&str> = Some("application/json");

    fn scan(content_type: Option<&str>, chunks: &[&[u8]]) -> Scan {
        let mut scanner = UsageScanner::for_content_type(content_type);
        for chunk in chunks {
            scanner.feed(chunk);
        }
        scanner.finish()
    }

    fn event(name: &str, data: &str) -> String {
        format!("event: {name}\ndata: {data}\n\n")
    }

    #[test]
    fn a_nested_cache_creation_counts_when_there_is_no_flat_total() {
        // The API sends the flat total and the per-lifetime breakdown
        // together, and the flat one is their sum. An Anthropic-compatible
        // backend that sends only the breakdown still wrote those tokens.
        let nested = br#"{"id":"m","type":"message","content":[],"usage":{"input_tokens":1000,"output_tokens":40,"cache_read_input_tokens":24000,"cache_creation":{"ephemeral_5m_input_tokens":400,"ephemeral_1h_input_tokens":100}}}"#;
        assert_eq!(
            scan(None, &[nested]).usage,
            Some(TokenUsage {
                input: 1000,
                output: 40,
                cache_read: 24000,
                cache_creation: 500,
                cache_reported: true,
            })
        );
        // Both present: the flat total is the sum, so counting both doubles it.
        let both = br#"{"id":"m","type":"message","content":[],"usage":{"input_tokens":1000,"output_tokens":40,"cache_read_input_tokens":24000,"cache_creation_input_tokens":500,"cache_creation":{"ephemeral_5m_input_tokens":400,"ephemeral_1h_input_tokens":100}}}"#;
        assert_eq!(
            scan(None, &[both]).usage,
            Some(TokenUsage {
                input: 1000,
                output: 40,
                cache_read: 24000,
                cache_creation: 500,
                cache_reported: true,
            })
        );
    }

    #[test]
    fn counts_at_the_edge_saturate_instead_of_wrapping() {
        let body = br#"{"id":"m","type":"message","content":[],"usage":{"input_tokens":18446744073709551615,"output_tokens":1,"cache_read_input_tokens":1,"cache_creation":{"a":18446744073709551615,"b":1}}}"#;
        let usage = scan(None, &[body]).usage.unwrap();
        assert_eq!(usage.cache_creation, u64::MAX);
        assert_eq!(usage.prompt(), u64::MAX);
    }

    #[test]
    fn saying_nothing_about_caching_differs_from_saying_zero() {
        let quiet = br#"{"id":"m","type":"message","content":[],"usage":{"input_tokens":1000,"output_tokens":40}}"#;
        assert_eq!(
            scan(None, &[quiet]).usage,
            Some(TokenUsage {
                input: 1000,
                output: 40,
                cache_read: 0,
                cache_creation: 0,
                cache_reported: false,
            })
        );
        let told = br#"{"id":"m","type":"message","content":[],"usage":{"input_tokens":1000,"output_tokens":40,"cache_read_input_tokens":0}}"#;
        assert_eq!(
            scan(None, &[told]).usage,
            Some(TokenUsage {
                input: 1000,
                output: 40,
                cache_read: 0,
                cache_creation: 0,
                cache_reported: true,
            })
        );
    }

    #[test]
    fn a_stream_takes_the_start_usage_and_the_final_output_count() {
        let body = [
            event(
                "message_start",
                r#"{"type":"message_start","message":{"id":"m","usage":{"input_tokens":10,"cache_creation_input_tokens":20,"cache_read_input_tokens":500,"output_tokens":1}}}"#,
            ),
            event(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"usage \"error\""}}"#,
            ),
            event(
                "message_delta",
                r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":42}}"#,
            ),
            event("message_stop", r#"{"type":"message_stop"}"#),
        ]
        .concat();
        let bytes = body.as_bytes();
        // Split inside a frame and inside a UTF-8-free JSON token.
        let scan = scan(SSE, &[&bytes[..37], &bytes[37..211], &bytes[211..]]);
        assert_eq!(
            scan.usage,
            Some(TokenUsage {
                input: 10,
                output: 42,
                cache_read: 500,
                cache_creation: 20,
                cache_reported: true,
            })
        );
        assert_eq!(scan.error, None);
    }

    #[test]
    fn a_final_delta_with_cumulative_input_overrides_the_start() {
        let body = [
            event(
                "message_start",
                r#"{"type":"message_start","message":{"usage":{"input_tokens":0,"output_tokens":0}}}"#,
            ),
            event(
                "message_delta",
                r#"{"type":"message_delta","usage":{"input_tokens":12,"output_tokens":34,"cache_read_input_tokens":7}}"#,
            ),
        ]
        .concat();
        let scan = scan(SSE, &[body.as_bytes()]);
        assert_eq!(
            scan.usage,
            Some(TokenUsage {
                input: 12,
                output: 34,
                cache_read: 7,
                cache_creation: 0,
                cache_reported: true,
            })
        );
    }

    #[test]
    fn an_error_event_is_reported() {
        let body = [
            event(
                "message_start",
                r#"{"type":"message_start","message":{"usage":{"input_tokens":3,"output_tokens":0}}}"#,
            ),
            event(
                "error",
                r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
            ),
        ]
        .concat();
        let scan = scan(SSE, &[body.as_bytes()]);
        assert_eq!(scan.error.as_deref(), Some("overloaded_error: Overloaded"));
        assert_eq!(scan.usage.map(|u| u.input), Some(3));
    }

    #[test]
    fn a_document_and_an_error_document() {
        let document = br#"{"id":"m","type":"message","content":[{"type":"text","text":"hi"}],"usage":{"input_tokens":5,"output_tokens":7,"cache_read_input_tokens":300}}"#;
        let scan = scan(JSON, &[&document[..20], &document[20..]]);
        assert_eq!(
            scan.usage,
            Some(TokenUsage {
                input: 5,
                output: 7,
                cache_read: 300,
                cache_creation: 0,
                cache_reported: true,
            })
        );

        let error = br#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#;
        let scan = super::tests::scan(None, &[error]);
        assert_eq!(scan.error.as_deref(), Some("invalid_request_error: bad"));
        assert_eq!(scan.usage, None);
    }

    #[test]
    fn a_body_without_usage_or_that_is_not_json_yields_nothing() {
        assert_eq!(scan(JSON, &[b"not json"]), Scan::default());
        assert_eq!(
            scan(SSE, &[b"data: {\"type\":\"ping\"}\n\n"]),
            Scan::default()
        );
        assert_eq!(scan(SSE, &[b"\xff\xfe\n\n"]), Scan::default());
    }

    #[test]
    fn an_oversized_document_is_not_kept() {
        let mut scanner = UsageScanner::for_content_type(JSON);
        scanner.feed(br#"{"usage":{"input_tokens":1,"output_tokens":1},"pad":""#);
        scanner.feed(&vec![b'x'; MAX_DOCUMENT_BYTES]);
        scanner.feed(br#""}"#);
        assert_eq!(scanner.finish(), Scan::default());
    }
}
