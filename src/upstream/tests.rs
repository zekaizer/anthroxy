use std::time::Duration;

use http::{HeaderMap, HeaderValue, StatusCode};

use super::*;
use crate::config::{BackendConfig, BackendKind, CredentialConfig, UpstreamConfig};
use crate::upstream::{DropReason, dropped_headers};

fn backend(beta: &[&str], headers: &[(&str, &str)]) -> Backend {
    backend_of_kind(BackendKind::Anthropic, beta, headers)
}

fn backend_of_kind(kind: BackendKind, beta: &[&str], headers: &[(&str, &str)]) -> Backend {
    Backend::from_config(
        "b",
        &BackendConfig {
            kind,
            url: "http://backend".into(),
            models_path: BackendConfig::default_models_path(),
            live_models: false,
            credential: CredentialConfig::None,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            anthropic_beta: beta.iter().map(|s| s.to_string()).collect(),
            drop_headers: Vec::new(),
            drop_fields: Vec::new(),
            proxy: None,
        },
    )
    .unwrap()
}

fn backend_with_drops(drops: &[&str]) -> Backend {
    Backend::from_config(
        "b",
        &BackendConfig {
            kind: BackendKind::Anthropic,
            url: "http://backend".into(),
            models_path: BackendConfig::default_models_path(),
            live_models: false,
            credential: CredentialConfig::None,
            headers: Default::default(),
            anthropic_beta: Vec::new(),
            drop_headers: drops.iter().map(|s| s.to_string()).collect(),
            drop_fields: Vec::new(),
            proxy: None,
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
        ("proxy-authorization", "Basic cHJveHk6c2VjcmV0"),
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
    let out = upstream_headers(&client_headers(), &backend(&[], &[]));
    for dropped in [
        "host",
        "content-length",
        "connection",
        "transfer-encoding",
        "proxy-authorization",
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
fn passthrough_keeps_the_client_authorization() {
    let out = upstream_headers(
        &client_headers(),
        &backend_of_kind(BackendKind::Passthrough, &[], &[]),
    );
    assert_eq!(out["authorization"], "Bearer client-token");
    assert_eq!(out["x-api-key"], "client-token");
    assert_eq!(out["anthropic-version"], "2023-06-01");
}

#[test]
fn upstream_headers_apply_backend_overrides() {
    let out = upstream_headers(
        &client_headers(),
        &backend(
            &[],
            &[("anthropic-version", "2024-01-01"), ("x-extra", "1")],
        ),
    );
    assert_eq!(
        out["anthropic-version"], "2024-01-01",
        "backend header overrides client"
    );
    assert_eq!(out["x-extra"], "1");
}

#[test]
fn upstream_headers_drop_what_the_backend_named() {
    let backend = backend_with_drops(&["x-stainless-*", "User-Agent"]);
    let out = upstream_headers(&client_headers(), &backend);
    assert!(out.get("x-stainless-retry-count").is_none(), "{out:?}");
    assert!(
        out.get("user-agent").is_none(),
        "a name is matched in any case"
    );
    assert_eq!(out["content-type"], "application/json", "nothing else goes");
}

#[test]
fn the_claude_code_preset_drops_what_claude_code_adds() {
    let mut client = client_headers();
    client.append("x-app", HeaderValue::from_static("cli"));
    client.append("x-claude-code-session-id", HeaderValue::from_static("s-1"));
    let out = upstream_headers(&client, &backend_with_drops(&["@claude-code"]));
    for gone in [
        "x-stainless-retry-count",
        "x-app",
        "x-claude-code-session-id",
    ] {
        assert!(out.get(gone).is_none(), "{gone} in {out:?}");
    }
    assert_eq!(
        out["user-agent"], "claude-cli/2.0",
        "the preset leaves user-agent alone"
    );
}

#[test]
fn every_header_the_backend_does_not_see_says_why() {
    let mut client = client_headers();
    client.append("x-app", HeaderValue::from_static("cli"));
    let backend = Backend::from_config(
        "b",
        &BackendConfig {
            kind: BackendKind::OpenAi,
            url: "http://backend".into(),
            models_path: BackendConfig::default_models_path(),
            live_models: false,
            credential: CredentialConfig::None,
            headers: [("x-app".to_owned(), "gateway".to_owned())].into(),
            anthropic_beta: Vec::new(),
            drop_headers: vec!["x-stainless-*".into()],
            drop_fields: Vec::new(),
            proxy: None,
        },
    )
    .unwrap();

    let dropped = dropped_headers(&client, &backend);
    let reason = |name: &str| {
        dropped
            .iter()
            .find(|h| h.name == name)
            .map(|h| (h.value.as_str(), h.reason))
            .unwrap_or_else(|| panic!("{name} not reported: {dropped:?}"))
    };
    assert_eq!(reason("host").1, DropReason::Framing);
    assert_eq!(reason("content-length").1, DropReason::Framing);
    assert_eq!(reason("connection").1, DropReason::Framing);
    assert_eq!(reason("proxy-authorization").1, DropReason::Framing);
    assert_eq!(reason("accept-encoding").1, DropReason::Uncompressed);
    assert_eq!(reason("anthropic-version").1, DropReason::BackendKind);
    assert_eq!(reason("anthropic-beta").1, DropReason::BackendKind);
    assert_eq!(reason("x-stainless-retry-count").1, DropReason::DropHeaders);
    assert_eq!(
        reason("x-app"),
        ("cli", DropReason::Overridden),
        "the client's value is gone, and it is the client's value that is shown"
    );
    assert_eq!(
        reason("authorization"),
        ("<redacted>", DropReason::ClientCredential)
    );
    assert_eq!(reason("x-api-key").0, "<redacted>");

    // Together the two reports are every header the client sent. A name the
    // backend forces is in the upstream map carrying the backend's value, so
    // it only counts as forwarded when the backend left it alone.
    let sent = upstream_headers(&client, &backend);
    for name in client.keys() {
        let forwarded = sent.contains_key(name) && !backend.headers.contains_key(name);
        assert!(
            forwarded || dropped.iter().any(|h| h.name == name.as_str()),
            "{name} is in neither report"
        );
    }
}

#[test]
fn upstream_headers_merge_beta_flags_without_duplicates() {
    let out = upstream_headers(
        &client_headers(),
        &backend(&["oauth-2025-04-20", "claude-code-20250219"], &[]),
    );
    assert_eq!(
        out["anthropic-beta"],
        "claude-code-20250219,fine-grained-tool-streaming-2025-05-14,oauth-2025-04-20"
    );

    let mut no_beta = client_headers();
    no_beta.remove("anthropic-beta");
    let out = upstream_headers(&no_beta, &backend(&["oauth-2025-04-20"], &[]));
    assert_eq!(out["anthropic-beta"], "oauth-2025-04-20");
}

#[test]
fn upstream_headers_for_openai_backends_carry_no_anthropic_headers() {
    let out = upstream_headers(
        &client_headers(),
        &backend_of_kind(BackendKind::OpenAi, &[], &[("x-extra", "1")]),
    );
    for dropped in ["anthropic-version", "anthropic-beta"] {
        assert!(!out.contains_key(dropped), "{dropped} should be dropped");
    }
    for kept in [
        "content-type",
        "user-agent",
        "x-stainless-retry-count",
        "x-extra",
    ] {
        assert!(out.contains_key(kept), "{kept} should be kept");
    }
}

#[test]
fn upstream_headers_merge_every_beta_header_the_client_sent() {
    let mut client = client_headers();
    client.append(
        "anthropic-beta",
        HeaderValue::from_static("context-1m-2025-08-07,claude-code-20250219"),
    );
    let out = upstream_headers(&client, &backend(&["oauth-2025-04-20"], &[]));
    assert_eq!(
        out.get_all("anthropic-beta").iter().count(),
        1,
        "the flags of every header end up in one"
    );
    assert_eq!(
        out["anthropic-beta"],
        "claude-code-20250219,fine-grained-tool-streaming-2025-05-14,context-1m-2025-08-07,oauth-2025-04-20"
    );
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
    up.insert("proxy-authenticate", HeaderValue::from_static("Basic"));
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

#[test]
fn ca_certificate_must_be_a_readable_pem_file() {
    let dir = tempfile::tempdir().unwrap();
    let with = |path: &std::path::Path| UpstreamConfig {
        ca_certificate: Some(path.to_path_buf()),
        ..UpstreamConfig::default()
    };

    let missing = dir.path().join("missing.pem");
    let error =
        http_client(&with(&missing), &Default::default()).expect_err("a missing file is an error");
    assert!(error.to_string().contains("missing.pem"), "{error}");

    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, "no certificate here\n").unwrap();
    let error = http_client(&with(&empty), &Default::default())
        .expect_err("a file without a certificate is an error");
    assert!(error.to_string().contains("empty.pem"), "{error}");

    assert!(http_client(&UpstreamConfig::default(), &Default::default()).is_ok());
}

#[test]
fn the_backoff_has_a_ceiling() {
    let policy = RetryPolicy {
        max_retries: 20,
        backoff: Duration::from_secs(1),
        retry_on_status: vec![503],
    };
    // Doubling unchecked, the tenth failure would wait over eight minutes and
    // the twentieth six days: a request nobody is still waiting for.
    for failure in 1..=policy.max_retries {
        let Decision::Retry(delay) = policy.on_status(failure, StatusCode::SERVICE_UNAVAILABLE)
        else {
            panic!("failure {failure} gave up");
        };
        assert!(delay <= MAX_BACKOFF, "failure {failure} waits {delay:?}");
    }
    assert_eq!(
        policy.on_status(4, StatusCode::SERVICE_UNAVAILABLE),
        Decision::Retry(Duration::from_secs(8)),
        "below the ceiling it still doubles"
    );
}

#[test]
fn the_logged_request_target_keeps_the_url_userinfo_out() {
    let backend = Backend::from_config(
        "gw",
        &BackendConfig {
            kind: BackendKind::Anthropic,
            url: "https://gw-user:url-secret@gw.corp".into(),
            models_path: BackendConfig::default_models_path(),
            live_models: false,
            credential: CredentialConfig::None,
            headers: Default::default(),
            anthropic_beta: Vec::new(),
            drop_headers: Vec::new(),
            drop_fields: Vec::new(),
            proxy: None,
        },
    )
    .unwrap();
    assert_eq!(
        super::client::shown_target(&backend, "/v1/messages"),
        "https://<redacted>@gw.corp/v1/messages"
    );
}

#[test]
fn a_forced_host_is_reported_as_the_backend_s_not_the_url_s() {
    let backend = backend(&[], &[("host", "gateway.example.corp")]);
    let headers = upstream_headers(&client_headers(), &backend);
    let sent = sent_headers(&backend, &headers, None, 0, SecretView::Masked);
    let host = sent.iter().find(|h| h.name == "host").unwrap();
    // What went on the wire: the forced value, which the HTTP client keeps.
    assert_eq!(
        (host.value.as_str(), host.source),
        ("gate…corp", HeaderSource::Backend)
    );
    assert_eq!(sent.iter().filter(|h| h.name == "host").count(), 1);
}
