mod support;

use claude_router::config::Config;
use serde_json::{Value, json};
use support::mock_upstream::echo;
use support::router::{TOKEN, config_with_backend};
use support::{MockUpstream, TestRouter};

fn parse(text: &str) -> Config {
    Config::parse(text, |name| std::env::var(name).ok()).unwrap()
}

async fn model_ids(router: &TestRouter, token: &str) -> (u16, Vec<String>) {
    let res = router
        .http
        .get(router.url("/v1/models"))
        .header("x-api-key", token)
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    let ids = if status == 200 {
        let body: Value = res.json().await.unwrap();
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect()
    } else {
        Vec::new()
    };
    (status, ids)
}

#[tokio::test]
async fn reload_swaps_models_backends_and_token_without_restart() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    assert_eq!(
        model_ids(&router, TOKEN).await,
        (200, vec!["fast".into(), "smart".into()])
    );

    let second = MockUpstream::start(echo).await;
    let new_config = format!(
        r#"
[server]
listen = "127.0.0.1:0"
token = "rotated-token"

[backends.mock]
url = "{}"

[backends.other]
url = "{}"
credential = {{ kind = "static", value = "other-key", header = "x_api_key" }}

[[models]]
id = "fast"
backend = "mock"

[[models]]
id = "newcomer"
backend = "other"
upstream_model = "other-v2"
"#,
        upstream.url(),
        second.url()
    );
    let report = router.reload.apply(&parse(&new_config)).unwrap();
    assert_eq!(report.models, 2);
    assert_eq!(report.backends, 2);
    assert!(!report.listen_changed);

    // Old token is gone, new one works, the model table is the new one.
    assert_eq!(model_ids(&router, TOKEN).await.0, 401);
    assert_eq!(
        model_ids(&router, "rotated-token").await,
        (200, vec!["fast".into(), "newcomer".into()])
    );

    // The new backend is reachable through the new model.
    let body = json!({"model": "newcomer", "max_tokens": 1, "messages": []});
    let res = router
        .http
        .post(router.url("/v1/messages"))
        .header("x-api-key", "rotated-token")
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["x-claude-router-backend"], "other");
    let seen = second.last();
    assert_eq!(seen.json()["model"], "other-v2");
    assert_eq!(seen.header("x-api-key"), Some("other-key"));
    assert!(
        upstream.received().is_empty()
            || upstream
                .received()
                .iter()
                .all(|r| r.path_and_query != "/v1/messages")
    );
}

#[tokio::test]
async fn failed_reload_keeps_the_previous_configuration() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;

    let broken = config_with_backend(&upstream.url(), "").replace(
        r#"credential = { kind = "static", value = "backend-secret-key" }"#,
        r#"credential = { kind = "env", name = "CLAUDE_ROUTER_TEST_DEFINITELY_UNSET" }"#,
    );
    let err = router.reload.apply(&parse(&broken)).unwrap_err();
    assert!(
        err.to_string()
            .contains("CLAUDE_ROUTER_TEST_DEFINITELY_UNSET"),
        "{err}"
    );
    assert_eq!(
        model_ids(&router, TOKEN).await,
        (200, vec!["fast".into(), "smart".into()])
    );
}

#[tokio::test]
async fn reload_reports_a_changed_listen_address() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let moved = config_with_backend(&upstream.url(), "")
        .replace("listen = \"127.0.0.1:0\"", "listen = \"127.0.0.1:9\"");
    let report = router.reload.apply(&parse(&moved)).unwrap();
    assert!(report.listen_changed);
    // Still answering on the original socket.
    assert_eq!(model_ids(&router, TOKEN).await.0, 200);
}
