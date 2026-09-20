use serde::{Deserialize, Serialize};

use crate::ir::{Effort, Model};

/// One entry of `GET /v1/models` as Claude Code consumes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelObject {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub display_name: String,
    /// RFC 3339 timestamp.
    pub created_at: String,
    /// Claude Code picker label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<ModelRuntime>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ModelThinking>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelRuntime {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort_levels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_effort: Option<String>,
}

impl ModelRuntime {
    fn is_empty(&self) -> bool {
        self.max_input_tokens.is_none()
            && self.max_output_tokens.is_none()
            && self.effort_levels.is_none()
            && self.default_effort.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelThinking {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort_options: Option<Vec<EffortOption>>,
}

impl ModelThinking {
    fn is_empty(&self) -> bool {
        self.effort_options.as_ref().is_none_or(|v| v.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffortOption {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ModelObject {
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        created_at: impl Into<String>,
    ) -> Self {
        let display_name = display_name.into();
        Self {
            id: id.into(),
            kind: "model".to_owned(),
            name: Some(display_name.clone()),
            display_name,
            created_at: created_at.into(),
            context_window: None,
            runtime: None,
            thinking: None,
        }
    }
}

/// Response body of `GET /v1/models`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelList {
    pub data: Vec<ModelObject>,
    pub first_id: Option<String>,
    pub last_id: Option<String>,
    #[serde(default)]
    pub has_more: bool,
}

impl ModelList {
    /// A complete, single-page list.
    pub fn all(data: Vec<ModelObject>) -> Self {
        Self {
            first_id: data.first().map(|m| m.id.clone()),
            last_id: data.last().map(|m| m.id.clone()),
            has_more: false,
            data,
        }
    }
}

/// Anthropic `/v1/models` list → IR. OpenAI-shaped JSON is empty (that codec
/// is `openai::decode_models`).
pub fn decode_models(bytes: &[u8]) -> Vec<Model> {
    #[derive(Deserialize)]
    struct List {
        #[serde(default)]
        data: Vec<serde_json::Value>,
    }
    let Ok(list) = serde_json::from_slice::<List>(bytes) else {
        return Vec::new();
    };
    list.data.into_iter().filter_map(anthropic_row).collect()
}

fn anthropic_row(value: serde_json::Value) -> Option<Model> {
    let obj = value.as_object()?;
    let id = obj.get("id")?.as_str()?.to_owned();
    if id.is_empty() {
        return None;
    }
    let display_name = obj
        .get("display_name")
        .or_else(|| obj.get("name"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())?;
    let created_at = obj
        .get("created_at")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok());
    let context_window = obj
        .get("context_window")
        .and_then(|v| v.as_u64())
        .filter(|n| *n > 0);
    Some(Model {
        id,
        display_name: display_name.to_owned(),
        created_at,
        context_window,
        max_output_tokens: obj
            .get("runtime")
            .and_then(|v| v.get("max_output_tokens"))
            .and_then(|v| v.as_u64()),
        effort: None,
    })
}

/// IR → Claude Code / Anthropic `GET /v1/models` row.
pub fn encode_model(model: &Model) -> ModelObject {
    let created_at = model
        .created_at
        .map(|t| t.to_string())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_owned());
    let mut runtime = ModelRuntime {
        max_input_tokens: model.context_window,
        max_output_tokens: model.max_output_tokens,
        effort_levels: None,
        default_effort: None,
    };
    let mut thinking = ModelThinking::default();
    if let Some(Effort { levels, default }) = &model.effort {
        if !levels.is_empty() {
            runtime.effort_levels = Some(levels.clone());
            thinking.effort_options = Some(
                levels
                    .iter()
                    .map(|id| EffortOption {
                        id: id.clone(),
                        name: Some(id.clone()),
                    })
                    .collect(),
            );
        }
        runtime.default_effort = default.clone();
    }
    ModelObject {
        id: model.id.clone(),
        kind: "model".to_owned(),
        name: Some(model.display_name.clone()),
        display_name: model.display_name.clone(),
        created_at,
        context_window: model.context_window,
        runtime: (!runtime.is_empty()).then_some(runtime),
        thinking: (!thinking.is_empty()).then_some(thinking),
    }
}

pub fn encode_models(models: &[Model]) -> Vec<ModelObject> {
    models.iter().map(encode_model).collect()
}
