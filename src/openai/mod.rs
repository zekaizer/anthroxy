//! Codecs between the IR and the OpenAI Chat Completions API (ADR-0010).
//! Nothing here knows the Anthropic wire format.

mod chunk;
mod common;
mod error;
mod models;
mod request;
mod response;

#[cfg(test)]
mod tests;

pub use chunk::ChunkDecoder;
pub use common::ParseError;
pub use error::decode as decode_error;
pub use models::decode as decode_models;
pub use request::encode as encode_request;
pub use response::{ResponseError, decode as decode_response};
