//! A model catalog entry. Names no wire format.

/// One model a backend lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub display_name: String,
    /// Instant the origin first listed the model, if it said so.
    pub created_at: Option<jiff::Timestamp>,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub effort: Option<Effort>,
}

/// Reasoning-effort knobs the origin advertised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effort {
    pub levels: Vec<String>,
    pub default: Option<String>,
}
