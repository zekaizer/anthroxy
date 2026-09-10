//! Single-endpoint gateway for Claude Code in front of several
//! Anthropic-API-compatible backends.
//!
//! All behaviour lives here. `src/main.rs` only parses the command line and
//! calls into this crate.

pub mod anthropic;
pub mod build_info;
pub mod cli;
pub mod config;
pub mod credential;
pub mod ir;
pub mod observability;
pub mod openai;
pub(crate) mod private_fs;
pub mod routing;
pub mod server;
pub mod service;
pub mod sse;
pub(crate) mod text;
pub mod translate;
pub mod upstream;
