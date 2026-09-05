//! Single-endpoint gateway for Claude Code in front of several
//! Anthropic-API-compatible backends.
//!
//! All behaviour lives here. `src/main.rs` only parses the command line and
//! calls into this crate.

pub mod config;
pub mod credential;
pub mod routing;
