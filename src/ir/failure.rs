//! A failure on its way between wire formats. Names no wire format.

/// What a backend refused, or what went wrong reading its answer. `message`
/// is the origin's own words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
}

/// The kinds a client can act on differently. A codec maps its own names
/// onto these; nothing here spells a wire format's vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    InvalidRequest,
    /// The request does not fit the model's context window.
    ContextExceeded,
    Authentication,
    Permission,
    NotFound,
    TooLarge,
    RateLimit,
    Overloaded,
    /// The origin failed on its own side, or named nothing the router knows.
    Upstream,
}

impl FailureKind {
    /// The kind an HTTP status stands for; anything else is `Upstream`.
    pub fn from_status(status: u16) -> Self {
        match status {
            400 => FailureKind::InvalidRequest,
            401 => FailureKind::Authentication,
            403 => FailureKind::Permission,
            404 => FailureKind::NotFound,
            413 => FailureKind::TooLarge,
            429 => FailureKind::RateLimit,
            529 => FailureKind::Overloaded,
            _ => FailureKind::Upstream,
        }
    }

    /// Whether the status alone said nothing beyond "it failed", so what the
    /// body names is worth taking instead.
    pub fn is_unspecific(self) -> bool {
        self == FailureKind::Upstream
    }
}

/// Text that marks a request over the model's context window in an error's
/// names or its message, one entry per way a server words it. Matched with
/// case and everything but ASCII letters and digits ignored, on both sides.
const CONTEXT_EXCEEDED: &[&str] = &[
    "context_length_exceeded",
    "exceed_context_size",
    "exceeds the available context size",
    "maximum context length",
    "context size has been exceeded",
    "context window exceeded",
];

/// Whether `text`, an error's name or message, says the request exceeded the
/// model's context window.
pub fn says_context_exceeded(text: &str) -> bool {
    let text = squashed(text);
    CONTEXT_EXCEEDED
        .iter()
        .any(|needle| text.contains(&squashed(needle)))
}

/// `text` lowercased, with only its ASCII letters and digits kept.
fn squashed(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Longest message kept; a backend's error text is cut here, so no
/// exchange record or error document carries more.
pub const MAX_MESSAGE_CHARS: usize = 4096;

impl Failure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if let Some((at, _)) = message.char_indices().nth(MAX_MESSAGE_CHARS) {
            message.truncate(at);
            message.push('…');
        }
        Self { kind, message }
    }

    /// A failure the router itself found in what a backend sent.
    pub fn upstream(message: impl Into<String>) -> Self {
        Self::new(FailureKind::Upstream, message)
    }
}
