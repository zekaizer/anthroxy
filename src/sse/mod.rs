//! Incremental server-sent events parsing, independent of what the events
//! carry.

mod parser;

#[cfg(test)]
mod tests;

pub use parser::{Frame, Parser, SseError};
