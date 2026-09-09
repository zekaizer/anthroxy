//! Minimal inspection of a request body. Only `model` and `stream` are read;
//! the body is otherwise opaque except for the rewrites in [`rewrite`].

use std::fmt;

use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, MapAccess, Visitor};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestPeek {
    pub model: Option<String>,
    pub stream: bool,
}

/// Only an object: `rewrite` edits the body as one, and a derived
/// `Deserialize` would also accept a JSON array, whose fields it reads
/// positionally.
impl<'de> Deserialize<'de> for RequestPeek {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(PeekVisitor)
    }
}

struct PeekVisitor;

impl<'de> Visitor<'de> for PeekVisitor {
    type Value = RequestPeek;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON object")
    }

    /// A repeated key takes its last value, as every JSON parser downstream
    /// does, so the router routes on what the backend will read.
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<RequestPeek, A::Error> {
        let mut peek = RequestPeek::default();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "model" => peek.model = map.next_value()?,
                "stream" => peek.stream = map.next_value()?,
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(peek)
    }
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
