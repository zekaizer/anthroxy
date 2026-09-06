use std::time::Duration;

use http::{HeaderMap, HeaderValue, StatusCode};

use super::*;
use crate::config::{BackendConfig, CredentialConfig, CredentialHeader};
use crate::credential::Credential;

fn backend(beta: &[&str], headers: &[(&str, &str)]) -> Backend {
    Backend::from_config(
        "b",
        &BackendConfig {
            url: "http://backend".into(),
            credential: CredentialConfig::None,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            anthropic_beta: beta.iter().map(|s| s.to_string()).collect(),
        },
    )
    .unwrap()
}

fn client_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    for (k, v) in [
        ("host", "router:8787"),
        ("content-length", "123"),
        ("connection", "keep-alive"),
        ("transfer-encoding", "chunked"),
        ("accept-encoding", "gzip"),
        ("x-api-key", "client-token"),
        ("authorization", "Bearer client-token"),
        ("content-type", "application/json"),
        ("anthropic-version", "2023-06-01"),
        (
            "anthropic-beta",
            "claude-code-20250219,fine-grained-tool-streaming-2025-05-14",
        ),
        ("user-agent", "claude-cli/2.0"),
        ("x-stainless-retry-count", "0"),
    ] {
        h.append(
            http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
            HeaderValue::from_static(v),
        );
    }
    h
}

#[test]
fn upstream_headers_drop_hop_by_hop_and_client_auth() {
    let out = upstream_headers(&client_headers(), &backend(&[], &[]), None);
    for dropped in [
        "host",
        "content-length",
        "connection",
        "transfer-encoding",
        "accept-encoding",
        "x-api-key",
        "authorization",
    ] {
        assert!(!out.contains_key(dropped), "{dropped} should be dropped");
    }
    for kept in [
        "content-type",
        "anthropic-version",
        "anthropic-beta",
        "user-agent",
        "x-stainless-retry-count",
    ] {
        assert!(out.contains_key(kept), "{kept} should be kept");
    }
    assert_eq!(
        out["anthropic-beta"],
        "claude-code-20250219,fine-grained-tool-streaming-2025-05-14"
    );
}

#[test]
fn upstream_headers_set_credential_and_overrides() {
    let cred = Credential::new(CredentialHeader::Bearer, "backend-token").unwrap();
    let out = upstream_headers(
        &client_headers(),
        &backend(
            &[],
            &[("anthropic-version", "2024-01-01"), ("x-extra", "1")],
        ),
        Some(&cred),
    );
    assert_eq!(out["authorization"], "Bearer backend-token");
    assert!(out["authorization"].is_sensitive());
    assert!(!out.contains_key("x-api-key"));
    assert_eq!(
        out["anthropic-version"], "2024-01-01",
        "backend header overrides client"
    );
    assert_eq!(out["x-extra"], "1");

    let cred = Credential::new(CredentialHeader::XApiKey, "k").unwrap();
    let out = upstream_headers(&client_headers(), &backend(&[], &[]), Some(&cred));
    assert_eq!(out["x-api-key"], "k");
    assert!(!out.contains_key("authorization"));
}

#[test]
fn upstream_headers_merge_beta_flags_without_duplicates() {
    let out = upstream_headers(
        &client_headers(),
        &backend(&["oauth-2025-04-20", "claude-code-20250219"], &[]),
        None,
    );
    assert_eq!(
        out["anthropic-beta"],
        "claude-code-20250219,fine-grained-tool-streaming-2025-05-14,oauth-2025-04-20"
    );

    let mut no_beta = client_headers();
    no_beta.remove("anthropic-beta");
    let out = upstream_headers(&no_beta, &backend(&["oauth-2025-04-20"], &[]), None);
    assert_eq!(out["anthropic-beta"], "oauth-2025-04-20");
}

#[test]
fn response_headers_drop_framing_only() {
    let mut up = HeaderMap::new();
    up.insert(
        "content-type",
        HeaderValue::from_static("text/event-stream"),
    );
    up.insert("content-length", HeaderValue::from_static("10"));
    up.insert("transfer-encoding", HeaderValue::from_static("chunked"));
    up.insert("connection", HeaderValue::from_static("close"));
    up.insert("request-id", HeaderValue::from_static("req_up"));
    up.insert(
        "anthropic-ratelimit-requests-remaining",
        HeaderValue::from_static("5"),
    );
    let out = response_headers(&up);
    assert_eq!(out.len(), 3);
    assert_eq!(out["content-type"], "text/event-stream");
    assert_eq!(out["request-id"], "req_up");
    assert_eq!(out["anthropic-ratelimit-requests-remaining"], "5");
}

#[test]
fn retry_policy_backs_off_exponentially_then_gives_up() {
    let policy = RetryPolicy {
        max_retries: 2,
        backoff: Duration::from_millis(100),
        retry_on_status: vec![503],
    };
    assert_eq!(
        policy.on_status(1, StatusCode::SERVICE_UNAVAILABLE),
        Decision::Retry(Duration::from_millis(100))
    );
    assert_eq!(
        policy.on_status(2, StatusCode::SERVICE_UNAVAILABLE),
        Decision::Retry(Duration::from_millis(200))
    );
    assert_eq!(
        policy.on_status(3, StatusCode::SERVICE_UNAVAILABLE),
        Decision::GiveUp
    );
    assert_eq!(
        policy.on_status(1, StatusCode::BAD_GATEWAY),
        Decision::GiveUp,
        "unlisted status"
    );
    assert_eq!(
        RetryPolicy::never().on_status(1, StatusCode::SERVICE_UNAVAILABLE),
        Decision::GiveUp
    );
}

#[tokio::test]
async fn retry_policy_retries_connection_refused_but_not_timeouts() {
    let policy = RetryPolicy {
        max_retries: 1,
        backoff: Duration::from_millis(1),
        retry_on_status: vec![],
    };
    // Port 1 on loopback is closed: connection refused.
    let refused = reqwest::Client::new()
        .get("http://127.0.0.1:1/")
        .send()
        .await
        .unwrap_err();
    assert!(is_connection_failure(&refused), "{refused:?}");
    assert_eq!(
        policy.on_transport_error(1, &refused),
        Decision::Retry(Duration::from_millis(1))
    );
    assert_eq!(policy.on_transport_error(2, &refused), Decision::GiveUp);

    // A listener that never answers produces a timeout, which is not retried.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(50))
        .build()
        .unwrap();
    let timeout = client
        .get(format!("http://{addr}/"))
        .send()
        .await
        .unwrap_err();
    assert!(timeout.is_timeout());
    assert!(!is_connection_failure(&timeout));
    assert_eq!(policy.on_transport_error(1, &timeout), Decision::GiveUp);
    drop(listener);
}
