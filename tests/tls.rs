//! Trust decisions for HTTPS backends: which CAs the router accepts.

mod support;

use anthroxy::config::{Config, process_env};
use anthroxy::server::Server;
use serde_json::Value;
use support::mock_upstream::echo;
use support::router::{config_with_backend, messages_body};
use support::{MockUpstream, TestRouter};

#[tokio::test]
async fn rejects_a_backend_signed_by_an_unknown_ca() {
    let (upstream, _ca_pem) = MockUpstream::start_tls(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;

    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 502);
    let body: Value = res.json().await.unwrap();
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.contains("UnknownIssuer"), "{message}");
    assert!(upstream.received().is_empty());
}

#[tokio::test]
async fn trusts_the_ca_named_by_ca_certificate() {
    let (upstream, ca_pem) = MockUpstream::start_tls(echo).await;
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("corp-ca.pem");
    std::fs::write(&ca, ca_pem).unwrap();
    let extra = format!("\n[upstream]\nca_certificate = \"{}\"\n", ca.display());
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;

    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(upstream.last().json()["model"], "mock-fast-v1");
}

#[tokio::test]
async fn an_unreadable_ca_certificate_fails_startup() {
    let text = config_with_backend(
        "https://127.0.0.1:1",
        "\n[upstream]\nca_certificate = \"/nonexistent/corp-ca.pem\"\n",
    );
    let config = Config::parse(&text, process_env).unwrap();
    let Err(error) = Server::bind(&config).await else {
        panic!("an unreadable CA file must fail startup");
    };
    assert!(
        error.to_string().contains("/nonexistent/corp-ca.pem"),
        "{error}"
    );
}
