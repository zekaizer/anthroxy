//! Minimal inspection of a request body. Only `model` and `stream` are read;
//! the body is otherwise opaque.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RequestPeek {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PeekError {
    #[error("request body is not a JSON object: {0}")]
    NotJson(#[source] serde_json::Error),
    #[error("request body has no `model` field")]
    NoModel,
}

/// Extracts `model` and `stream`. Fails on non-JSON or a missing `model`.
pub fn peek(body: &[u8]) -> Result<RequestPeek, PeekError> {
    let peek: RequestPeek = serde_json::from_slice(body).map_err(PeekError::NotJson)?;
    if peek.model.as_deref().is_none_or(str::is_empty) {
        return Err(PeekError::NoModel);
    }
    Ok(peek)
}

/// Returns `body` with `model` replaced, all other fields and their order
/// intact. `body` must already have passed [`peek`], which proves it is a
/// JSON object.
pub fn rewrite_model(body: &[u8], model: &str) -> Vec<u8> {
    let mut value: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(body).expect("peek accepted this body");
    value.insert(
        "model".to_owned(),
        serde_json::Value::String(model.to_owned()),
    );
    serde_json::to_vec(&value).expect("a parsed document serializes")
}
