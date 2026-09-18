use serde::{Deserialize, Serialize};

/// One entry of `GET /v1/models`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelObject {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub display_name: String,
    /// RFC 3339 timestamp.
    pub created_at: String,
}

impl ModelObject {
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        created_at: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: "model".to_owned(),
            display_name: display_name.into(),
            created_at: created_at.into(),
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

/// Anthropic model identity from an upstream `GET /v1/models` body.
/// Accepts the Anthropic list (`type`/`display_name`/`created_at`) and the
/// OpenAI list (`object`/`created`). Unknown JSON yields an empty list.
pub fn identity_list(bytes: &[u8]) -> Vec<ModelObject> {
    #[derive(serde::Deserialize)]
    struct WireList {
        #[serde(default)]
        data: Vec<WireModel>,
    }
    #[derive(serde::Deserialize)]
    struct WireModel {
        id: String,
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default)]
        created_at: Option<String>,
        #[serde(default)]
        created: Option<i64>,
    }
    let Ok(list) = serde_json::from_slice::<WireList>(bytes) else {
        return Vec::new();
    };
    list.data
        .into_iter()
        .filter(|m| !m.id.is_empty())
        .map(|m| {
            let display_name = m.display_name.filter(|s| !s.is_empty()).unwrap_or_else(|| m.id.clone());
            let created_at = m.created_at.filter(|s| !s.is_empty()).unwrap_or_else(|| {
                m.created
                    .and_then(unix_created_at)
                    .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_owned())
            });
            ModelObject::new(m.id, display_name, created_at)
        })
        .collect()
}

fn unix_created_at(created: i64) -> Option<String> {
    jiff::Timestamp::from_second(created)
        .ok()
        .map(|t| t.to_string())
}
