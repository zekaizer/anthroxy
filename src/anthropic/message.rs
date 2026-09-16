//! IR message → Messages API response document.

use serde_json::{Value, json};

use super::fragments::{identity, stop_reason_name, usage_json};
use crate::ir::{Block, Message};

/// `fallback_model` is reported when the backend named none.
pub fn encode(message: &Message, fallback_model: &str) -> Vec<u8> {
    let content: Vec<Value> = message
        .blocks
        .iter()
        .map(|block| match block {
            Block::Thinking(text) => json!({"type": "thinking", "thinking": text}),
            Block::Text(text) => json!({"type": "text", "text": text}),
            Block::ToolUse {
                id,
                name,
                arguments,
            } => {
                json!({"type": "tool_use", "id": id, "name": name, "input": tool_input(arguments)})
            }
        })
        .collect();
    let (id, model) = identity(&message.id, &message.model, fallback_model);
    let document = json!({
        "id": id,
        "type": "message",
        "role": "assistant",
        "model": model,
        "content": content,
        "stop_reason": stop_reason_name(message.stop_reason),
        "stop_sequence": null,
        "usage": usage_json(&message.usage),
    });
    serde_json::to_vec(&document).expect("a JSON tree serializes")
}

/// The arguments as an object; anything the backend produced that is not
/// one becomes `{}`, since `input` must be an object.
fn tool_input(arguments: &str) -> Value {
    match serde_json::from_str::<Value>(arguments) {
        Ok(value @ Value::Object(_)) => value,
        _ => json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Block, StopReason, Usage};
    use serde_json::json;

    #[test]
    fn document_with_every_block_kind() {
        let message = Message {
            id: "chatcmpl-1".into(),
            model: "qwen".into(),
            blocks: vec![
                Block::Thinking("plan".into()),
                Block::Text("Reading".into()),
                Block::ToolUse {
                    id: "call_a".into(),
                    name: "read".into(),
                    arguments: "{\"path\":\"a\"}".into(),
                },
                Block::ToolUse {
                    id: "call_b".into(),
                    name: "bash".into(),
                    arguments: "not json".into(),
                },
            ],
            stop_reason: StopReason::ToolUse,
            usage: Usage {
                input_tokens: 5,
                output_tokens: 7,
                cache_read_tokens: 300,
                cache_creation_tokens: 0,
                cache_reported: true,
                thinking_tokens: 0,
            },
        };
        let document: serde_json::Value =
            serde_json::from_slice(&encode(&message, "fallback")).unwrap();
        assert_eq!(
            document,
            json!({
                "id": "chatcmpl-1",
                "type": "message",
                "role": "assistant",
                "model": "qwen",
                "content": [
                    {"type": "thinking", "thinking": "plan"},
                    {"type": "text", "text": "Reading"},
                    {"type": "tool_use", "id": "call_a", "name": "read", "input": {"path": "a"}},
                    {"type": "tool_use", "id": "call_b", "name": "bash", "input": {}}
                ],
                "stop_reason": "tool_use",
                "stop_sequence": null,
                "usage": {"input_tokens": 5, "output_tokens": 7, "cache_read_input_tokens": 300, "cache_creation_input_tokens": 0}
            })
        );
    }
}
