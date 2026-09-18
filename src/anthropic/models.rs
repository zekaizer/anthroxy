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
