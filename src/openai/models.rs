//! OpenAI `/v1/models` list → IR.

use serde::Deserialize;

use crate::ir::{Effort, Model};

/// Window fields OpenAI-compatible lists use, most specific first.
const CONTEXT_FIELDS: &[&str] = &[
    "loaded_context_length",
    "max_model_len",
    "context_length",
    "max_context_length",
    "context_window",
    "max_input_tokens",
];

#[derive(Deserialize)]
struct List {
    #[serde(default)]
    data: Vec<serde_json::Value>,
}

/// Decode an OpenAI-shaped model list. Unknown JSON is empty.
pub fn decode(bytes: &[u8]) -> Vec<Model> {
    let Ok(list) = serde_json::from_slice::<List>(bytes) else {
        return Vec::new();
    };
    list.data.into_iter().filter_map(row).collect()
}

fn row(value: serde_json::Value) -> Option<Model> {
    let obj = value.as_object()?;
    let id = obj.get("id")?.as_str()?.to_owned();
    if id.is_empty() {
        return None;
    }
    let display_name = obj
        .get("name")
        .or_else(|| obj.get("display_name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&id)
        .to_owned();
    let created_at = obj
        .get("created")
        .and_then(|v| v.as_i64())
        .and_then(|n| jiff::Timestamp::from_second(n).ok());
    let context_window = CONTEXT_FIELDS
        .iter()
        .find_map(|field| obj.get(*field).and_then(|v| v.as_u64()).filter(|n| *n > 0));
    let max_output_tokens = obj
        .get("max_output_tokens")
        .or_else(|| obj.get("max_tokens"))
        .and_then(|v| v.as_u64())
        .filter(|n| *n > 0);
    let effort = effort(obj.get("capabilities"));
    Some(Model {
        id,
        display_name,
        created_at,
        context_window,
        max_output_tokens,
        effort,
    })
}

fn effort(caps: Option<&serde_json::Value>) -> Option<Effort> {
    let caps = caps.and_then(|v| v.as_object())?;
    let levels: Vec<String> = caps
        .get("reasoning_effort")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let default = caps
        .get("default_reasoning_effort")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if levels.is_empty() && default.is_none() {
        return None;
    }
    Some(Effort { levels, default })
}
