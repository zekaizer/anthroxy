//! Diagnostics: log subscriber setup and on-disk capture of proxied bodies.

pub mod body_log;
mod subscriber;

pub use body_log::{BodyLog, MAX_RECORDED_RESPONSE_BYTES, Recorder, RequestRecord};
pub use subscriber::{init, resolve_directives};
