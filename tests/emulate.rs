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

#[tokio::test]
async fn a_preset_names_client_headers_the_backend_does_not_receive() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(
            &upstream,
            "emulate = \"client\"\ndrop_headers = [\"x-app\", \"user-agent\"]",
            "",
        ),
        &[(
            "client",
            r#"
drop_headers = ["x-stainless-*", "@claude-code"]

[headers]
user-agent = "client/1"
x-session-id = "{header:x-claude-code-session-id}"
"#,
        )],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let res = router
        .post("/v1/messages", &chat("qwen"))
        .header("x-claude-code-session-id", "abc-123")
        .header("x-app", "cli")
        .header("x-stainless-lang", "js")
        .header("x-stainless-retry-count", "0")
        .header("x-kept", "yes")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let received = upstream.last();
    let stainless: Vec<&str> = received
        .headers
        .keys()
        .map(|name| name.as_str())
        .filter(|name| name.starts_with("x-stainless-"))
        .collect();
    assert_eq!(stainless, [] as [&str; 0]);
    assert_eq!(received.header("x-claude-code-session-id"), None);
    assert_eq!(
        received.header("x-app"),
        None,
        "the backend's list applies beside the preset's"
    );
    assert_eq!(received.header("x-kept"), Some("yes"));
    assert_eq!(
        received.header("x-session-id"),
        Some("abc-123"),
        "a placeholder is read before the header it names is dropped"
    );
    assert_eq!(
        received.header("user-agent"),
        Some("client/1"),
        "a header the preset sets is sent though the backend drops the client's"
    );
}

#[tokio::test]
async fn a_variable_only_a_body_field_names_holds_what_no_header_could() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(
            &upstream,
            "emulate = \"client\"",
            "emulate = { preset = \"client\", note = \"three\\nfour\" }",
        ),
        &[(
            "client",
            "[variables]\nnote = \"one\\ntwo\"\n\n[set_fields]\n\"extra.note\" = \"{note}\"\n",
        )],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let body = sent(&router, &upstream, &chat("qwen")).await.json();
    assert_eq!(body["extra"]["note"], "one\ntwo");
    let body = sent(&router, &upstream, &chat("other")).await.json();
    assert_eq!(body["extra"]["note"], "three\nfour");
}

#[tokio::test]
async fn the_status_lists_the_keys_the_backend_wrote_and_none_of_the_presets() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(
            &upstream,
            "emulate = \"client\"\ndrop_headers = [\"x-app\"]\nset_fields = { top_k = 20 }",
            "",
        ),
        &[(
            "client",
            "drop_headers = [\"x-stainless-*\"]\n\n[set_fields]\ntool_choice = \"auto\"\ntop_k = 40\n",
        )],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let status: Value = router
        .get("/api/status")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let shown = &status["backends"][0];
    assert_eq!(shown["set_fields"], json!(["top_k"]));
    assert_eq!(shown["drop_headers"], json!(["x-app"]));
}

#[tokio::test]
async fn a_preset_sets_body_fields_and_the_backends_value_wins() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(
            &upstream,
            "emulate = \"client\"\nset_fields = { \"extra.source\" = \"backend {version}\", top_k = 20 }",
            "",
        ),
        &[(
            "client",
            r#"
[variables]
version = "1.19.6"

[set_fields]
tool_choice = "auto"
top_k = 40
"extra.source" = "preset"
"extra.client" = "client/{version} {header:x-app}"
"extra.session" = "{header:x-absent}"
"#,
        )],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let body = sent(&router, &upstream, &chat("qwen")).await.json();
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["top_k"], 20);
    assert_eq!(
        body["extra"],
        json!({"client": "client/1.19.6 cli", "source": "backend {version}"}),
        "a field whose client header is absent is left out; a variable is a preset's"
    );
    assert_eq!(body["model"], "qwen");
}

/// A conversation whose last message is a `system` message.
fn chat_with_a_late_system_message(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 16,
        "system": "Lead.",
        "messages": [
            {"role": "user", "content": "hi"},
            {"role": "system", "content": "Later."}
        ]
    })
}

#[tokio::test]
async fn a_preset_places_a_later_system_message() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(&upstream, "emulate = \"client\"", ""),
        &[("client", "mid_conversation_system = \"merge\"\n")],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let body = sent(&router, &upstream, &chat_with_a_late_system_message("qwen"))
        .await
        .json();
    assert_eq!(
        body["messages"],
        json!([
            {"role": "system", "content": "Lead.\n\nLater."},
            {"role": "user", "content": "hi"}
        ])
    );
}

#[tokio::test]
async fn the_backends_and_the_models_placement_win_over_the_presets() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(
            &upstream,
            "emulate = \"client\"\nmid_conversation_system = \"keep\"",
            "mid_conversation_system = \"user\"",
        ),
        &[("client", "mid_conversation_system = \"merge\"\n")],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let body = sent(&router, &upstream, &chat_with_a_late_system_message("qwen"))
        .await
        .json();
    assert_eq!(
        body["messages"],
        json!([
            {"role": "system", "content": "Lead."},
            {"role": "user", "content": "hi"},
            {"role": "system", "content": "Later."}
        ]),
        "`keep` written on the backend is the backend's value"
    );

    let body = sent(
        &router,
        &upstream,
        &chat_with_a_late_system_message("other"),
    )
    .await
    .json();
    assert_eq!(
        body["messages"][2],
        json!({"role": "user", "content": "<system-reminder>\nLater.\n</system-reminder>"})
    );
}

/// A chat request the encoder writes `user` and `reasoning_effort` for.
fn chat_with_user_and_effort(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 16,
        "stream": true,
        "messages": [{"role": "user", "content": "hi"}],
        "metadata": {"user_id": "u1"},
        "output_config": {"effort": "high"}
    })
}

#[tokio::test]
async fn a_preset_omits_body_fields_and_orders_the_rest() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(&upstream, "emulate = \"client\"", ""),
        &[(
            "client",
            r#"
omit_fields = ["user", "reasoning_effort", "stream_options.include_usage"]
field_order = ["model", "max_tokens", "messages", "tools", "tool_choice", "stream"]

[set_fields]
tool_choice = "auto"
"#,
        )],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let received = sent(&router, &upstream, &chat_with_user_and_effort("qwen")).await;
    assert_eq!(
        String::from_utf8_lossy(&received.body),
        r#"{"model":"qwen","max_tokens":16,"messages":[{"role":"user","content":"hi"}],"tool_choice":"auto","stream":true,"stream_options":{}}"#
    );
}

#[tokio::test]
async fn a_field_the_backend_sets_is_sent_though_the_preset_omits_it() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(
            &upstream,
            "emulate = \"client\"\nset_fields = { user = \"gateway\" }",
            "",
        ),
        &[(
            "client",
            "omit_fields = [\"user\"]\nfield_order = [\"user\", \"model\"]\n",
        )],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let received = sent(&router, &upstream, &chat("qwen")).await;
    assert_eq!(
        String::from_utf8_lossy(&received.body),
        r#"{"user":"gateway","model":"qwen","messages":[{"role":"user","content":"hi"}],"max_tokens":16,"stream":false}"#
    );
}

#[tokio::test]
async fn a_preset_names_the_output_token_limit() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(&upstream, "emulate = \"main\"", "emulate = \"release\""),
        &[
            ("main", "max_tokens_field = \"max_completion_tokens\"\n"),
            ("release", "max_tokens_field = \"max_tokens\"\n"),
        ],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let body = sent(&router, &upstream, &chat("qwen")).await.json();
    assert_eq!(body["max_completion_tokens"], 16);
    assert_eq!(body.get("max_tokens"), None);

    let body = sent(&router, &upstream, &chat("other")).await.json();
    assert_eq!(body["max_tokens"], 16);
    assert_eq!(body.get("max_completion_tokens"), None);
}
