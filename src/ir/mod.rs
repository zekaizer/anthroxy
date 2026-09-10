//! The intermediate representation between the Anthropic Messages API and
//! other chat APIs (ADR-0010). Requests are a typed document; responses are
//! a sequence of events that both a stream and a completed message fold
//! from. Nothing here names a wire format.

mod event;
mod message;
mod request;

#[cfg(test)]
mod tests;

pub use event::{Event, StopReason, Usage};
pub use message::{Block, Message};
pub use request::{Image, Message as RequestMessage, Part, Request, Role, Tool, ToolChoice};
