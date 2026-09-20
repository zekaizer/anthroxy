//! Composition of the Anthropic and OpenAI codecs through the IR
//! (ADR-0010). This is the only module that names both wire formats.

mod catalog;
mod error;
mod request;
mod response;
mod stream;

#[cfg(test)]
mod tests;

pub use catalog::{catalog, models};
pub use error::{failure, upstream_error};
pub use request::request;
pub use response::{document_events, response};
pub use stream::Translator;

/// Where an OpenAI backend takes a translated `/v1/messages` request.
pub const CHAT_COMPLETIONS_PATH: &str = "/v1/chat/completions";
