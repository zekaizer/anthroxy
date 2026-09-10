//! Codecs between the IR and the OpenAI Chat Completions API (ADR-0010).
//! Nothing here knows the Anthropic wire format.

mod chunk;
mod error;
mod request;
mod response;

#[cfg(test)]
mod tests;

pub use chunk::{ChunkDecoder, ChunkError};
pub use error::message as error_message;
pub use request::encode as encode_request;
pub use response::{ResponseError, decode as decode_response};
