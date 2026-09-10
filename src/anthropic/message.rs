//! IR message → Messages API response document.

use serde_json::{Value, json};

use crate::ir::{Block, Message};

pub fn encode(message: &Message) -> Vec<u8> {
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
    let document = json!({
        "id": message.id,
        "type": "message",
        "role": "assistant",
        "model": message.model,
        "content": content,
        "stop_reason": super::stream::stop_reason_name(message.stop_reason),
        "stop_sequence": null,
        "usage": super::stream::usage_json(&message.usage),
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
                thinking_tokens: 0,
            },
        };
        let document: serde_json::Value = serde_json::from_slice(&encode(&message)).unwrap();
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
                "usage": {"input_tokens": 5, "output_tokens": 7, "cache_read_input_tokens": 300}
            })
        );
    }
}
