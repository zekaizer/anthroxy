//! Codecs between the IR and the OpenAI Chat Completions API (ADR-0010).
//! Nothing here knows the Anthropic wire format.

mod request;

#[cfg(test)]
mod tests;

pub use request::encode as encode_request;
