/// One step of a response. A decoder emits these in order; an encoder turns
/// them into its wire format without buffering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Empty `id` or `model` means the backend gave none.
    Start {
        id: String,
        model: String,
    },
    ThinkingDelta(String),
    TextDelta(String),
    /// `index` identifies the call for later deltas; calls are numbered in
    /// the order the backend starts them.
    ToolCallStart {
        index: u32,
        id: String,
        name: String,
    },
    /// A fragment of the JSON arguments of call `index`.
    ToolCallDelta {
        index: u32,
        arguments: String,
    },
    Finish(StopReason),
    Usage(Usage),
    /// The backend reported a failure; nothing meaningful follows.
    Error(String),
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    /// The backend declined to continue (content filter).
    Refusal,
}

/// Token counts in the Anthropic sense: `input_tokens` excludes what was
/// read from a prompt cache, which is counted in `cache_read_tokens`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    /// Part of `output_tokens` spent on reasoning.
    pub thinking_tokens: u64,
}
