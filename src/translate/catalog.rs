//! OpenAI or Anthropic `/v1/models` bytes → IR → Claude Code catalog rows.

use crate::anthropic::{self, ModelObject};
use crate::config::BackendKind;
use crate::ir::Model;
use crate::openai;

/// Decode an origin model list through the IR, then encode Anthropic/Claude Code rows.
pub fn catalog(kind: BackendKind, bytes: &[u8]) -> Vec<ModelObject> {
    anthropic::encode_models(&models(kind, bytes))
}

/// An origin model list as IR rows, by the codec the backend's kind calls
/// for. The only place a decoder is chosen.
pub fn models(kind: BackendKind, bytes: &[u8]) -> Vec<Model> {
    match kind {
        BackendKind::OpenAi => openai::decode_models(bytes),
        BackendKind::Anthropic | BackendKind::Passthrough => {
            let anthropic = anthropic::decode_models(bytes);
            if !anthropic.is_empty() {
                anthropic
            } else {
                openai::decode_models(bytes)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_list_fills_claude_code_catalog_fields() {
        let raw = br#"{
            "object": "list",
            "data": [{
                "id": "grok-4.6",
                "object": "model",
                "created": 1700000000,
                "owned_by": "xai",
                "context_length": 500000,
                "capabilities": {
                    "reasoning_effort": ["low", "high"],
                    "default_reasoning_effort": "high"
                }
            }]
        }"#;
        let rows = catalog(BackendKind::OpenAi, raw);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "grok-4.6");
        assert_eq!(rows[0].context_window, Some(500000));
        assert_eq!(rows[0].name.as_deref(), Some("grok-4.6"));
        let runtime = rows[0].runtime.as_ref().unwrap();
        assert_eq!(runtime.max_input_tokens, Some(500000));
        assert_eq!(
            runtime.effort_levels,
            Some(vec!["low".into(), "high".into()])
        );
        assert_eq!(runtime.default_effort.as_deref(), Some("high"));
        let thinking = rows[0].thinking.as_ref().unwrap();
        assert_eq!(thinking.effort_options.as_ref().unwrap()[0].id, "low");
    }

    #[test]
    fn vllm_max_model_len_becomes_context_window() {
        let raw = br#"{"data":[{"id":"local","max_model_len":32768}]}"#;
        let rows = catalog(BackendKind::OpenAi, raw);
        assert_eq!(rows[0].context_window, Some(32768));
    }

    #[test]
    fn anthropic_list_keeps_identity() {
        let raw = br#"{"data":[{"id":"claude-opus-4-5","type":"model","display_name":"Opus","created_at":"2025-01-01T00:00:00Z"}]}"#;
        let rows = catalog(BackendKind::Passthrough, raw);
        assert_eq!(rows[0].id, "claude-opus-4-5");
        assert_eq!(rows[0].display_name, "Opus");
        assert_eq!(rows[0].context_window, None);
    }

    #[test]
    fn passthrough_openai_shaped_list_uses_openai_codec() {
        let raw = br#"{"object":"list","data":[{"id":"grok-4","object":"model","created":1700000000,"context_length":200000}]}"#;
        let rows = catalog(BackendKind::Passthrough, raw);
        assert_eq!(rows[0].id, "grok-4");
        assert_eq!(rows[0].context_window, Some(200000));
    }
}
