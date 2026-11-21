//! What the Chat Completions encoder leaves to its caller.

/// How the encoder writes what a backend's configuration decides. `Default`
/// is what it writes when the configuration says nothing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EncodeOptions {
    pub placement: SystemPlacement,
    pub max_tokens_field: MaxTokensField,
    /// The `tool_choice` of a request that has tools and names none.
    pub default_tool_choice: Option<ToolChoiceDefault>,
    pub tool_only_content: ToolOnlyContent,
    /// Written as `strict` in every tool definition; `None` writes none.
    pub tool_strict: Option<bool>,
    /// The assistant message field a turn's reasoning is written under;
    /// `None` writes no reasoning.
    pub reasoning_field: Option<String>,
}

/// Where a `Role::System` message that is not the first message goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SystemPlacement {
    /// A `system` message where it is.
    #[default]
    Keep,
    /// Its text appended to the leading `system` message.
    Merge,
    /// A `user` message where it is, wrapped in `<system-reminder>`.
    User,
}

/// The name the output-token limit is written under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaxTokensField {
    #[default]
    MaxTokens,
    MaxCompletionTokens,
}

impl MaxTokensField {
    pub fn name(self) -> &'static str {
        match self {
            MaxTokensField::MaxTokens => "max_tokens",
            MaxTokensField::MaxCompletionTokens => "max_completion_tokens",
        }
    }
}

/// A `tool_choice` the encoder writes when the request names none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoiceDefault {
    Auto,
    Required,
    None,
}

/// The `content` of an assistant message that calls tools and has no text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolOnlyContent {
    /// No `content` key.
    #[default]
    Omitted,
    /// `""`
    Empty,
    /// `null`
    Null,
}
