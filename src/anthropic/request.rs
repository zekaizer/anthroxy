//! Minimal inspection of a request body. Only `model` and `stream` are read;
//! the body is otherwise opaque except for the rewrites in [`rewrite`].

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

/// Returns `body` with `model` replaced when `model` is given and every path
/// in `drop_fields` removed, other fields and their order intact; `None` when
/// nothing changed, so the caller forwards the original bytes. A path is
/// dot-separated object keys; one whose prefix is missing or not an object
/// removes nothing. `body` must already have passed [`peek`], which proves it
/// is a JSON object.
pub fn rewrite(body: &[u8], model: Option<&str>, drop_fields: &[String]) -> Option<Vec<u8>> {
    if model.is_none() && drop_fields.is_empty() {
        return None;
    }
    let mut value: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(body).expect("peek accepted this body");
    let mut changed = false;
    for path in drop_fields {
        changed |= remove_path(&mut value, path);
    }
    if let Some(model) = model {
        value.insert(
            "model".to_owned(),
            serde_json::Value::String(model.to_owned()),
        );
        changed = true;
    }
    changed.then(|| serde_json::to_vec(&value).expect("a parsed document serializes"))
}

/// Whether `path` named an existing field, which is now gone.
fn remove_path(object: &mut serde_json::Map<String, serde_json::Value>, path: &str) -> bool {
    match path.split_once('.') {
        None => object.shift_remove(path).is_some(),
        Some((head, rest)) => match object.get_mut(head) {
            Some(serde_json::Value::Object(inner)) => remove_path(inner, rest),
            _ => false,
        },
    }
}
