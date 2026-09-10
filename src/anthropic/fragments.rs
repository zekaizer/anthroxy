//! Pieces of Messages API JSON that both response encoders write.

use serde_json::{Map, Value, json};

use crate::ir::{StopReason, Usage};

pub fn stop_reason_name(reason: StopReason) -> &'static str {
    match reason {
        StopReason::EndTurn => "end_turn",
        StopReason::MaxTokens => "max_tokens",
        StopReason::ToolUse => "tool_use",
        StopReason::Refusal => "refusal",
    }
}

/// Anthropic usage: cache and thinking counters only when there are any,
/// so a backend that reports none yields the plain two-field shape.
pub fn usage_json(usage: &Usage) -> Value {
    let mut out = Map::new();
    out.insert("input_tokens".into(), json!(usage.input_tokens));
    out.insert("output_tokens".into(), json!(usage.output_tokens));
    if usage.cache_read_tokens > 0 {
        out.insert(
            "cache_read_input_tokens".into(),
            json!(usage.cache_read_tokens),
        );
    }
    if usage.thinking_tokens > 0 {
        out.insert(
            "output_tokens_details".into(),
            json!({"thinking_tokens": usage.thinking_tokens}),
        );
    }
    Value::Object(out)
}

/// The message id and model to report: what the backend gave, a fresh
/// `msg_` id and `fallback_model` where it gave none.
pub fn identity(id: &str, model: &str, fallback_model: &str) -> (String, String) {
    let id = if id.is_empty() {
        format!("msg_{}", uuid::Uuid::new_v4().simple())
    } else {
        id.to_owned()
    };
    let model = if model.is_empty() {
        fallback_model
    } else {
        model
    };
    (id, model.to_owned())
}
