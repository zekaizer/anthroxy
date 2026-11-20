//! Talking to backends: header translation, retry policy and the HTTP client.

mod backend;
mod client;
mod dialect;
mod header_command;
mod headers;
pub mod network;
mod order;
pub mod probe;
mod retry;
mod timeout;
mod transport;

#[cfg(test)]
mod tests;

pub use backend::{Backend, BackendBuildError};
pub use client::{
    Origin, UpstreamClient, UpstreamError, UpstreamRequest, UpstreamResponse, describe,
};
pub use dialect::{BodyRules, Dialect, HeaderRules, Target};
pub use header_command::{
    HeaderCommand, HeaderCommandError, HeaderCommandStatus, HeaderRun, parse_output,
};
pub use headers::{
    DropReason, DroppedHeader, ForcedHeaders, HeaderSource, SecretView, SentHeader,
    X_ROUTER_BACKEND, X_ROUTER_MODEL, X_ROUTER_UPSTREAM_MODEL, dropped_headers, header_value,
    is_event_stream, response_headers, sent_headers, upstream_headers,
};
pub use order::HeaderOrder;
pub use retry::{Decision, MAX_BACKOFF, RetryPolicy, is_connection_failure};
pub use timeout::{BodyClock, BodyError, TimedBody, TimeoutClock, Timeouts};
pub use transport::{ClientBuildError, HttpClients, Transport};
