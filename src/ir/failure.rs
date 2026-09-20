//! A failure on its way between wire formats. Names no wire format.

/// What a backend refused, or what went wrong reading its answer. `message`
/// is the origin's own words; the router prefixes it where it is encoded.
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

impl Failure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    /// A failure the router itself found in what a backend sent.
    pub fn upstream(message: impl Into<String>) -> Self {
        Self::new(FailureKind::Upstream, message)
    }
}
