//! What the Chat Completions encoder leaves to its caller.

/// How the encoder writes what a backend's configuration decides. `Default`
/// is what it writes when the configuration says nothing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EncodeOptions {
    pub placement: SystemPlacement,
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
