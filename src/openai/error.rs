//! What an error body from an OpenAI-compatible server says.

use serde_json::Value;

use super::chunk::error_document;

/// Characters of a non-JSON body reported to the client.
pub const MAX_RAW_MESSAGE: usize = 200;

/// `error.message` of an OpenAI error document; otherwise the first
/// [`MAX_RAW_MESSAGE`] characters of the body, trimmed.
pub fn message(raw: &[u8]) -> String {
    if let Ok(Value::Object(fields)) = serde_json::from_slice::<Value>(raw)
        && let Some(message) = error_document(&fields)
    {
        return message;
    }
    String::from_utf8_lossy(raw)
        .trim()
        .chars()
        .take(MAX_RAW_MESSAGE)
        .collect()
}
