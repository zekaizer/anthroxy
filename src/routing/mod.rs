//! Maps the `model` a client names to a backend and the model name that
//! backend understands (ADR-0003).

mod registry;

#[cfg(test)]
mod tests;

pub use registry::{Match, Registry, Resolution, Route};
