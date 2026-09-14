//! Inspection and rewriting of a request body on its way to a backend that
//! speaks the Messages API. Only `model` and `stream` are read; the body is
//! otherwise opaque except for the rewrites in [`rewrite`].

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
/// in `drop_fields` removed. For an Anthropic backend (`for_anthropic`),
/// unsigned `thinking` blocks are removed too (ADR-0010: the router's own
/// thinking blocks, which an Anthropic backend rejects; an assistant message
/// left empty by that goes with them) and tool call ids are put in the form
/// it accepts (ADR-0013). Other fields and their order stay intact;
/// `Ok(None)` when nothing changed, so the caller forwards the original
/// bytes. A path is dot-separated object keys; one whose prefix is missing
/// or not an object removes nothing.
///
/// Reading the whole document is stricter than [`peek`], which skips values it
/// does not need: a body nested deeper than serde's recursion limit gets here
/// and is [`PeekError::NotJson`], since the caller must not forward a request
/// whose `model` it could not rewrite.
pub fn rewrite(
    body: &[u8],
    model: Option<&str>,
    drop_fields: &[String],
    for_anthropic: bool,
) -> Result<Option<Vec<u8>>, PeekError> {
    // Parsing is paid only when a rewrite can apply; the Anthropic rules need
    // the text `"thinking"` or `"tool_use"` to be in the body at all. The
    // body passed `peek`, so it is UTF-8 and `str::contains` (two-way
    // search) does the scan.
    let text = std::str::from_utf8(body).unwrap_or_default();
    let may_strip = for_anthropic && text.contains(THINKING);
    let may_rename_ids = for_anthropic && text.contains(TOOL_USE);
    if model.is_none() && drop_fields.is_empty() && !may_strip && !may_rename_ids {
        return Ok(None);
    }
    let mut value: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(body).map_err(PeekError::NotJson)?;
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
    if may_strip {
        changed |= strip_unsigned(&mut value);
    }
    if may_rename_ids {
        changed |= accepted_tool_ids(&mut value);
    }
    Ok(changed.then(|| serde_json::to_vec(&value).expect("a parsed document serializes")))
}

const THINKING: &str = "\"thinking\"";
const TOOL_USE: &str = "\"tool_use\"";

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

/// Whether any unsigned `thinking` block, or a message emptied by removing
/// one, is now gone. Signed `thinking` and `redacted_thinking` blocks stay.
fn strip_unsigned(root: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    use serde_json::Value;
    let Some(Value::Array(messages)) = root.get_mut("messages") else {
        return false;
    };
    let mut changed = false;
    messages.retain_mut(|message| {
        let Some(Value::Array(blocks)) = message.get_mut("content") else {
            return true;
        };
        let before = blocks.len();
        blocks.retain(|block| {
            !(block.get("type").and_then(Value::as_str) == Some("thinking")
                && block
                    .get("signature")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty))
        });
        if blocks.len() == before {
            return true;
        }
        changed = true;
        !blocks.is_empty()
    });
    changed
}

/// Whether any `tool_use.id` or `tool_result.tool_use_id` had a character
/// outside `[A-Za-z0-9_-]`, which is now `_` (ADR-0013).
fn accepted_tool_ids(root: &mut serde_json::Map<String, serde_json::Value>) -> bool {
    use serde_json::Value;
    let Some(Value::Array(messages)) = root.get_mut("messages") else {
        return false;
    };
    let mut changed = false;
    let blocks = messages
        .iter_mut()
        .filter_map(|message| match message.get_mut("content") {
            Some(Value::Array(blocks)) => Some(blocks),
            _ => None,
        })
        .flatten();
    for block in blocks {
        let field = match block.get("type").and_then(Value::as_str) {
            Some("tool_use") => "id",
            Some("tool_result") => "tool_use_id",
            _ => continue,
        };
        if let Some(Value::String(id)) = block.get_mut(field)
            && id.chars().any(|c| !accepted_id_char(c))
        {
            *id = id
                .chars()
                .map(|c| if accepted_id_char(c) { c } else { '_' })
                .collect();
            changed = true;
        }
    }
    changed
}

fn accepted_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}
