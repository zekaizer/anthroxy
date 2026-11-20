//! `emulate`: a `kind = "openai"` backend receives requests written the way
//! a preset says (ADR-0022).

mod support;

use serde_json::{Value, json};

use support::config_dir::ConfigDir;
use support::mock_upstream::{Received, json_response};
use support::openai::completion;
use support::router::TOKEN;
use support::{MockUpstream, TestRouter};

const CLIENT: &str = r#"
[variables]
version = "1.19.6"

[headers]
user-agent = "opencode/{version}"
x-session-id = "ses_{header:x-claude-code-session-id}"
"#;

/// Answers a chat request with a completion and anything else with a model
/// list.
async fn backend() -> MockUpstream {
    MockUpstream::start(|received| match received.path_and_query.as_str() {
        "/v1/chat/completions" => completion(json!({"role": "assistant", "content": "ok"}), "stop"),
        _ => json_response(
            200,
            json!({"object": "list", "data": [{"id": "live-1", "object": "model"}]}),
        ),
    })
    .await
}

/// One `openai` backend at `upstream` with the models `qwen` and `other`.
/// `backend` is added to the backend's table and `other` to that model's.
fn config(upstream: &MockUpstream, backend: &str, other: &str) -> String {
    format!(
        r#"
[server]
listen = "127.0.0.1:0"
token = "{TOKEN}"

[[models]]
id = "qwen"
backend = "mock"

[[models]]
id = "other"
backend = "mock"
{other}

[backends.mock]
kind = "openai"
url = "{}"
credential = {{ kind = "static", value = "backend-secret-key" }}
{backend}
"#,
        upstream.url()
    )
}

fn chat(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 16,
        "messages": [{"role": "user", "content": "hi"}]
    })
}

/// What the backend received for `body`, sent by a client in session
/// `abc-123`.
async fn sent(router: &TestRouter, upstream: &MockUpstream, body: &Value) -> Received {
    let res = router
        .post("/v1/messages", body)
        .header("x-claude-code-session-id", "abc-123")
        .header("x-app", "cli")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200, "{}", res.text().await.unwrap());
    upstream.last()
}

#[tokio::test]
async fn a_preset_header_holds_text_a_client_header_and_a_variable() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(&upstream, "emulate = \"client\"", ""),
        &[("client", CLIENT)],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let received = sent(&router, &upstream, &chat("qwen")).await;
    assert_eq!(received.header("user-agent"), Some("opencode/1.19.6"));
    assert_eq!(received.header("x-session-id"), Some("ses_abc-123"));

    let res = router
        .post("/v1/messages", &chat("qwen"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let received = upstream.last();
    assert_eq!(
        received.header("x-session-id"),
        None,
        "a value whose client header is absent is not sent"
    );
    assert_eq!(received.header("user-agent"), Some("opencode/1.19.6"));
}

#[tokio::test]
async fn emulate_gives_a_variable_another_value_which_is_text() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(
            &upstream,
            "emulate = { preset = \"client\", version = \"1.20.0\" }",
            "emulate = { preset = \"client\", version = \"{header:x-app}\" }",
        ),
        &[("client", CLIENT)],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let received = sent(&router, &upstream, &chat("qwen")).await;
    assert_eq!(received.header("user-agent"), Some("opencode/1.20.0"));

    let received = sent(&router, &upstream, &chat("other")).await;
    assert_eq!(
        received.header("user-agent"),
        Some("opencode/{header:x-app}"),
        "a variable's value is not read for placeholders"
    );
}

#[tokio::test]
async fn a_model_entry_replaces_its_backends_emulate() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(&upstream, "emulate = \"client\"", "emulate = \"second\""),
        &[
            ("client", CLIENT),
            ("second", "[headers]\nx-client = \"second\"\n"),
        ],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let received = sent(&router, &upstream, &chat("qwen")).await;
    assert_eq!(received.header("user-agent"), Some("opencode/1.19.6"));
    assert_eq!(received.header("x-client"), None);

    let received = sent(&router, &upstream, &chat("other")).await;
    assert_eq!(received.header("x-client"), Some("second"));
    assert_eq!(
        received.header("x-session-id"),
        None,
        "nothing of the backend's preset is left"
    );
    assert_ne!(received.header("user-agent"), Some("opencode/1.19.6"));
}

#[tokio::test]
async fn a_model_entry_may_emulate_on_a_backend_that_does_not() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(&upstream, "", "emulate = \"client\""),
        &[("client", CLIENT)],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let received = sent(&router, &upstream, &chat("qwen")).await;
    assert_eq!(received.header("x-session-id"), None);
    let received = sent(&router, &upstream, &chat("other")).await;
    assert_eq!(received.header("x-session-id"), Some("ses_abc-123"));
}

#[tokio::test]
async fn a_header_the_backend_sets_wins_over_the_presets() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(
            &upstream,
            "emulate = \"client\"\nheaders = { User-Agent = \"mine/1.0\", x-literal = \"{version}\" }",
            "",
        ),
        &[("client", CLIENT)],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let received = sent(&router, &upstream, &chat("qwen")).await;
    assert_eq!(received.header("user-agent"), Some("mine/1.0"));
    assert_eq!(
        received.header("x-session-id"),
        Some("ses_abc-123"),
        "the preset's other headers are still sent"
    );
    assert_eq!(
        received.header("x-literal"),
        Some("{version}"),
        "a variable exists in a preset only"
    );
}

#[tokio::test]
async fn the_model_list_is_asked_for_with_the_preset_headers() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(&upstream, "emulate = \"client\"\nlive_models = true", ""),
        &[("client", CLIENT)],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let res = router.get("/v1/models").send().await.unwrap();
    assert_eq!(res.status(), 200);
    let list: Vec<Received> = upstream
        .received()
        .into_iter()
        .filter(|received| received.path_and_query == "/v1/models")
        .collect();
    assert!(!list.is_empty(), "the backend was asked for its models");
    for received in list {
        assert_eq!(received.header("user-agent"), Some("opencode/1.19.6"));
    }
}
