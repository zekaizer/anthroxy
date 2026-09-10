//! Wire types of the Anthropic Messages API that the router itself produces
//! or inspects, and the codecs between that API and the IR (ADR-0010). For
//! `anthropic` backends everything else is relayed as bytes.

mod decode;
mod error;
mod message;
mod models;
mod request;
mod stream;

#[cfg(test)]
mod tests;

pub use decode::{DecodeError, decode};
pub use error::{ErrorResponse, ErrorType};
pub use message::encode as encode_message;
pub use models::{ModelList, ModelObject};
pub use request::{PeekError, RequestPeek, peek, rewrite};
pub use stream::StreamEncoder;
