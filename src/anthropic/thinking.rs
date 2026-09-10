//! Thinking blocks the router itself produced (ADR-0010) carry no
//! signature; an Anthropic backend rejects them. They are removed before a
//! request goes to one, so a session that came back from an `openai`
//! backend does not pay a failed round trip.

/// Returns `body` without unsigned `thinking` blocks, and without assistant
/// messages left empty by that; `None` when nothing had to change, so the
/// caller forwards the original bytes. Signed `thinking` and
/// `redacted_thinking` blocks stay. A body that is not a JSON object is
/// returned as `None` too.
use serde_json::Value;

pub fn strip_unsigned(body: &[u8]) -> Option<Vec<u8>> {
    // Cheap gate: nothing to do unless a thinking block can be present.
    if !body.windows(NEEDLE.len()).any(|w| w == NEEDLE) {
        return None;
    }
    let mut root: serde_json::Map<String, Value> = serde_json::from_slice(body).ok()?;
    let messages = root.get_mut("messages")?.as_array_mut()?;
    let mut changed = false;
    messages.retain_mut(|message| {
        let Some(Value::Array(blocks)) = message.get_mut("content") else {
            return true;
        };
        let before = blocks.len();
        blocks.retain(|block| !is_unsigned_thinking(block));
        if blocks.len() == before {
            return true;
        }
        changed = true;
        !blocks.is_empty()
    });
    changed.then(|| serde_json::to_vec(&root).expect("a parsed document serializes"))
}

const NEEDLE: &[u8] = b"\"thinking\"";

fn is_unsigned_thinking(block: &Value) -> bool {
    block.get("type").and_then(Value::as_str) == Some("thinking")
        && block
            .get("signature")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn run(body: Value) -> Option<Value> {
        strip_unsigned(&serde_json::to_vec(&body).unwrap())
            .map(|b| serde_json::from_slice(&b).unwrap())
    }

    #[test]
    fn unsigned_blocks_go_and_signed_ones_stay() {
        let body = json!({"model": "m", "messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "router made"},
                {"type": "thinking", "thinking": "anthropic made", "signature": "sig"},
                {"type": "redacted_thinking", "data": "xx"},
                {"type": "text", "text": "hello"}
            ]},
            {"role": "user", "content": [{"type": "text", "text": "more"}]}
        ]});
        assert_eq!(
            run(body).unwrap()["messages"],
            json!([
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": [
                    {"type": "thinking", "thinking": "anthropic made", "signature": "sig"},
                    {"type": "redacted_thinking", "data": "xx"},
                    {"type": "text", "text": "hello"}
                ]},
                {"role": "user", "content": [{"type": "text", "text": "more"}]}
            ])
        );
    }

    #[test]
    fn an_empty_signature_is_what_claude_code_stores_for_router_blocks() {
        let body = json!({"model": "m", "messages": [
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "router made", "signature": ""},
                {"type": "text", "text": "hello"}
            ]}
        ]});
        assert_eq!(
            run(body).unwrap()["messages"][0]["content"],
            json!([{"type": "text", "text": "hello"}])
        );
    }

    #[test]
    fn an_assistant_turn_left_empty_is_dropped() {
        let body = json!({"model": "m", "messages": [
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": [{"type": "thinking", "thinking": "only"}]},
            {"role": "user", "content": "again"}
        ]});
        assert_eq!(
            run(body).unwrap()["messages"],
            json!([{"role": "user", "content": "hi"}, {"role": "user", "content": "again"}])
        );
    }

    #[test]
    fn untouched_bodies_are_not_rewritten() {
        assert_eq!(
            run(json!({"model": "m", "messages": [{"role": "user", "content": "hi"}]})),
            None
        );
        assert_eq!(
            run(
                json!({"model": "m", "messages": [{"role": "assistant", "content": [
                {"type": "thinking", "thinking": "t", "signature": "s"}]}]})
            ),
            None
        );
        assert_eq!(
            run(
                json!({"model": "m", "messages": [{"role": "user", "content": "the word thinking"}]})
            ),
            None
        );
        assert_eq!(strip_unsigned(b"not json"), None);
    }
}
