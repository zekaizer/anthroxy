/// Text parts of one message are joined with a blank line when flattened.
pub const TEXT_SEPARATOR: &str = "\n\n";

/// A chat request. Only what both APIs can express; anything else is dropped
/// or rejected by the decoder that produced this.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub model: String,
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<Tool>,
    pub tool_choice: Option<ToolChoice>,
    pub max_tokens: Option<u64>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub stop: Vec<String>,
    pub stream: bool,
    /// Caller identifier for the backend's own accounting.
    pub user: Option<String>,
    /// `low`, `medium` or `high`.
    pub reasoning_effort: Option<String>,
    /// The caller forbids parallel tool calls.
    pub disable_parallel_tool_calls: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    /// Instructions placed mid-conversation; text only.
    System,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Text(String),
    /// `data` is the base64 payload as received.
    Image {
        media_type: String,
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// Text content flattened; images kept apart, since a tool message
    /// cannot carry them on every API.
    ToolResult {
        tool_use_id: String,
        content: String,
        images: Vec<Image>,
        /// The tool failed; the model must not read the content as a result.
        is_error: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub media_type: String,
    /// The base64 payload as received.
    pub data: String,
}

impl Part {
    /// The block type this part came from, for messages naming it.
    pub fn kind(&self) -> &'static str {
        match self {
            Part::Text(_) => "text",
            Part::Image { .. } => "image",
            Part::ToolUse { .. } => "tool_use",
            Part::ToolResult { .. } => "tool_result",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema of the input.
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolChoice {
    Auto,
    Required,
    None,
    Tool(String),
}
