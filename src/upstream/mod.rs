//! Talking to backends: header translation, retry policy and the HTTP client.

mod backend;
mod client;
mod headers;
pub mod probe;
mod retry;

#[cfg(test)]
mod tests;

pub use backend::{Backend, BackendBuildError};
pub use client::{
    UpstreamClient, UpstreamError, UpstreamRequest, UpstreamResponse, describe, http_client,
};
pub use headers::{
    X_ROUTER_BACKEND, X_ROUTER_MODEL, X_ROUTER_UPSTREAM_MODEL, response_headers, upstream_headers,
};
pub use retry::{Decision, RetryPolicy, is_connection_failure};
