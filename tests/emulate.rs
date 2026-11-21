//! `emulate`: a `kind = "openai"` backend receives requests written the way
//! a preset says (ADR-0022).

mod support;

use axum::http::Version;
use axum::response::Response;
use serde_json::{Value, json};

use support::config_dir::ConfigDir;
use support::mock_upstream::{Received, json_response};
use support::openai::completion;
use support::raw_upstream::{Head, REFUSED_KEY, RawUpstream};
use support::router::TOKEN;
use support::{MockUpstream, TestRouter};

const CLIENT: &str = r#"
[variables]
version = "1.19.6"

[headers]
user-agent = "opencode/{version}"
x-session-id = "ses_{header:x-claude-code-session-id}"
"#;

/// A completion for a chat request and a model list for anything else.
fn answer(received: &Received) -> Response {
    match received.path_and_query.as_str() {
        "/v1/chat/completions" => completion(json!({"role": "assistant", "content": "ok"}), "stop"),
        _ => json_response(
            200,
            json!({"object": "list", "data": [{"id": "live-1", "object": "model"}]}),
        ),
    }
}

async fn backend() -> MockUpstream {
    MockUpstream::start(answer).await
}

/// One `openai` backend at `upstream` with the models `qwen` and `other`.
/// `backend` is added to the backend's table and `other` to that model's.
fn config(upstream: &MockUpstream, backend: &str, other: &str) -> String {
    config_at(&upstream.url(), backend, other)
}

/// [`config`] with the backend at `url`.
fn config_at(url: &str, backend: &str, other: &str) -> String {
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
url = "{url}"
credential = {{ kind = "static", value = "backend-secret-key" }}
{backend}
"#
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

/// `chat` with one tool, and `tool_choice` when given.
fn chat_with_a_tool(model: &str, tool_choice: Option<Value>) -> Value {
    let mut body = chat(model);
    body["tools"] = json!([{
        "name": "Read",
        "description": "Read a file",
        "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}
    }]);
    if let Some(choice) = tool_choice {
        body["tool_choice"] = choice;
    }
    body
}

#[tokio::test]
async fn a_preset_gives_the_tool_choice_of_a_request_with_tools_that_names_none() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(&upstream, "emulate = \"client\"", ""),
        &[("client", "default_tool_choice = \"auto\"\n")],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let body = sent(&router, &upstream, &chat_with_a_tool("qwen", None))
        .await
        .json();
    assert_eq!(body["tool_choice"], "auto");

    let body = sent(&router, &upstream, &chat("qwen")).await.json();
    assert_eq!(
        body.get("tool_choice"),
        None,
        "a request without tools gets no default"
    );

    let named = chat_with_a_tool("qwen", Some(json!({"type": "any"})));
    let body = sent(&router, &upstream, &named).await.json();
    assert_eq!(
        body["tool_choice"], "required",
        "what Claude Code named is kept"
    );
}

/// A conversation whose assistant turn called a tool and said nothing.
fn chat_after_a_tool_only_turn(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 16,
        "messages": [
            {"role": "user", "content": "read a.rs"},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "Read", "input": {"path": "a.rs"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "fn main() {}"}
            ]}
        ]
    })
}

/// The assistant message of the request the backend received, as sent.
async fn assistant_turn(router: &TestRouter, upstream: &MockUpstream, body: &Value) -> String {
    let body = sent(router, upstream, body).await.json();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("an assistant message")
        .to_string()
}

const READ_CALL: &str = r#""tool_calls":[{"id":"toolu_1","type":"function","function":{"name":"Read","arguments":"{\"path\":\"a.rs\"}"}}]"#;

#[tokio::test]
async fn a_preset_writes_the_content_of_a_turn_that_only_calls_tools() {
    let upstream = backend().await;
    let presets = [
        ("empty", "tool_only_content = \"empty\"\n"),
        ("null", "tool_only_content = \"null\"\n"),
        ("omitted", "tool_only_content = \"omitted\"\n"),
    ];
    let dir = ConfigDir::new(
        &config(&upstream, "emulate = \"empty\"", "emulate = \"null\""),
        &presets,
    );
    let router = TestRouter::start_at(&dir.config()).await;
    assert_eq!(
        assistant_turn(&router, &upstream, &chat_after_a_tool_only_turn("qwen")).await,
        format!(r#"{{"role":"assistant","content":"",{READ_CALL}}}"#)
    );
    assert_eq!(
        assistant_turn(&router, &upstream, &chat_after_a_tool_only_turn("other")).await,
        format!(r#"{{"role":"assistant","content":null,{READ_CALL}}}"#)
    );

    let dir = ConfigDir::new(&config(&upstream, "emulate = \"omitted\"", ""), &presets);
    let router = TestRouter::start_at(&dir.config()).await;
    assert_eq!(
        assistant_turn(&router, &upstream, &chat_after_a_tool_only_turn("qwen")).await,
        format!(r#"{{"role":"assistant",{READ_CALL}}}"#)
    );
}

#[tokio::test]
async fn a_preset_writes_strict_on_every_tool_definition() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(&upstream, "emulate = \"loose\"", "emulate = \"strict\""),
        &[
            ("loose", "tool_strict = false\n"),
            ("strict", "tool_strict = true\n"),
        ],
    );
    let router = TestRouter::start_at(&dir.config()).await;
    let with_two_tools = |model: &str| {
        let mut body = chat_with_a_tool(model, None);
        let tools = body["tools"].as_array_mut().unwrap();
        tools.push(json!({"name": "Bare", "input_schema": {"type": "object"}}));
        body
    };

    let received = sent(&router, &upstream, &with_two_tools("qwen")).await;
    assert_eq!(
        received.json()["tools"].to_string(),
        concat!(
            r#"[{"type":"function","function":{"name":"Read","description":"Read a file","parameters":{"type":"object","properties":{"path":{"type":"string"}}},"strict":false}},"#,
            r#"{"type":"function","function":{"name":"Bare","parameters":{"type":"object","properties":{}},"strict":false}}]"#
        )
    );

    let body = sent(&router, &upstream, &with_two_tools("other"))
        .await
        .json();
    let strict: Vec<&Value> = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| &tool["function"]["strict"])
        .collect();
    assert_eq!(strict, [&json!(true), &json!(true)]);
}

#[tokio::test]
async fn a_preset_sends_the_reasoning_of_earlier_turns_back() {
    let upstream = MockUpstream::start(|_| {
        completion(
            json!({
                "role": "assistant",
                "content": null,
                "reasoning_content": "The user wants a.rs. I read it first.",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "Read", "arguments": "{\"path\":\"a.rs\"}"}
                }]
            }),
            "tool_calls",
        )
    })
    .await;
    let dir = ConfigDir::new(
        &config(&upstream, "emulate = \"client\"", ""),
        &[(
            "client",
            "reasoning_field = \"reasoning_content\"\ntool_only_content = \"empty\"\n",
        )],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let mut request = chat_with_a_tool("qwen", None);
    let res = router.post("/v1/messages", &request).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let mut turn: Value = res.json().await.unwrap();
    assert_eq!(turn["content"][0]["type"], "thinking", "{turn}");
    // Claude Code stores a block that came without a signature with an
    // empty one, and sends the turn back followed by the tool's result.
    turn["content"][0]["signature"] = json!("");
    let messages = request["messages"].as_array_mut().unwrap();
    messages.push(json!({"role": "assistant", "content": turn["content"]}));
    messages.push(json!({"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "call_1", "content": "fn main() {}"}
    ]}));
    assert_eq!(
        assistant_turn(&router, &upstream, &request).await,
        concat!(
            r#"{"role":"assistant","content":"","reasoning_content":"The user wants a.rs. I read it first.","#,
            r#""tool_calls":[{"id":"call_1","type":"function","function":{"name":"Read","arguments":"{\"path\":\"a.rs\"}"}}]}"#
        )
    );
}

#[tokio::test]
async fn a_turn_without_reasoning_text_gets_no_reasoning_field() {
    let upstream = backend().await;
    let dir = ConfigDir::new(
        &config(&upstream, "emulate = \"client\"", ""),
        &[("client", "reasoning_field = \"reasoning_content\"\n")],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    for content in [
        json!([{"type": "text", "text": "Hello"}]),
        json!([
            {"type": "thinking", "thinking": "", "signature": ""},
            {"type": "redacted_thinking", "data": "opaque"},
            {"type": "text", "text": "Hello"}
        ]),
    ] {
        let mut request = chat("qwen");
        let messages = request["messages"].as_array_mut().unwrap();
        messages.push(json!({"role": "assistant", "content": content}));
        messages.push(json!({"role": "user", "content": "again"}));
        assert_eq!(
            assistant_turn(&router, &upstream, &request).await,
            r#"{"role":"assistant","content":"Hello"}"#
        );
    }
}

/// The HTTP versions requests arrive in at an HTTPS backend that offers
/// HTTP/2 and HTTP/1.1: a chat request for `qwen`, one for `other`, and the
/// model list. `backend` and `other` are the `emulate` lines of the backend
/// and of the model `other`.
async fn versions_with(
    backend: &str,
    other: &str,
    presets: &[(&str, &str)],
) -> (Version, Version, Version) {
    let (upstream, ca_pem) = MockUpstream::start_tls(answer).await;
    let dir = ConfigDir::new("", presets);
    let ca = dir.config().with_file_name("ca.pem");
    std::fs::write(&ca, ca_pem).unwrap();
    dir.write_config(&config(
        &upstream,
        &format!(
            "{backend}\nlive_models = true\n\n[upstream]\nca_certificate = \"{}\"",
            ca.display()
        ),
        other,
    ));
    let router = TestRouter::start_at(&dir.config()).await;

    let qwen = sent(&router, &upstream, &chat("qwen")).await.version;
    let other = sent(&router, &upstream, &chat("other")).await.version;
    let res = router.get("/v1/models").send().await.unwrap();
    assert_eq!(res.status(), 200);
    let lists: Vec<Version> = upstream
        .received()
        .into_iter()
        .filter(|received| received.path_and_query == "/v1/models")
        .map(|received| received.version)
        .collect();
    assert!(!lists.is_empty(), "the backend was asked for its models");
    assert!(
        lists.iter().all(|version| *version == lists[0]),
        "{lists:?}"
    );
    (qwen, other, lists[0])
}

const HTTP_1_1: &str = "http_version = \"1.1\"\n";

#[tokio::test]
async fn a_preset_keeps_a_route_off_http_2() {
    assert_eq!(
        versions_with("emulate = \"client\"", "", &[("client", "")]).await,
        (Version::HTTP_2, Version::HTTP_2, Version::HTTP_2),
        "without the key the server's offer of HTTP/2 is taken"
    );
    assert_eq!(
        versions_with("emulate = \"client\"", "", &[("client", HTTP_1_1)]).await,
        (Version::HTTP_11, Version::HTTP_11, Version::HTTP_11)
    );
}

#[tokio::test]
async fn the_http_version_is_that_of_the_preset_a_request_travels_under() {
    assert_eq!(
        versions_with("", "emulate = \"client\"", &[("client", HTTP_1_1)]).await,
        (Version::HTTP_2, Version::HTTP_11, Version::HTTP_2),
        "the model list is the backend's request, not a model's"
    );
    assert_eq!(
        versions_with(
            "emulate = \"client\"",
            "emulate = \"plain\"",
            &[("client", HTTP_1_1), ("plain", "")]
        )
        .await,
        (Version::HTTP_11, Version::HTTP_2, Version::HTTP_11)
    );
}

/// The head of the request a backend on a bare socket received for `body`,
/// sent by a client in session `abc-123`.
async fn head_sent(router: &TestRouter, upstream: &RawUpstream, body: &Value) -> Head {
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

/// The headers of opencode 1.19.6 in the order it sends them, without the
/// two a preset never sends (`Connection`, `Accept-Encoding`).
const OPENCODE_HEADERS: &str = r#"
http_version = "1.1"
header_order = [
    "authorization", "content-type", "user-agent",
    "x-opencode-session-id", "x-session-affinity", "x-session-id",
    "accept", "host", "content-length",
]
drop_headers = ["@claude-code"]

[headers]
user-agent = "opencode/1.19.6"
accept = "*/*"
x-opencode-session-id = "{header:x-claude-code-session-id}"
x-session-affinity = "{header:x-claude-code-session-id}"
x-session-id = "{header:x-claude-code-session-id}"
"#;

#[tokio::test]
async fn a_preset_orders_the_headers_credential_and_framing_included() {
    let upstream = RawUpstream::start().await;
    let dir = ConfigDir::new(
        &config_at(&upstream.url(), "emulate = \"client\"", ""),
        &[("client", OPENCODE_HEADERS)],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let head = head_sent(&router, &upstream, &chat("qwen")).await;
    assert_eq!(head.request_line(), "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(
        head.names(),
        [
            "authorization",
            "content-type",
            "user-agent",
            "x-opencode-session-id",
            "x-session-affinity",
            "x-session-id",
            "accept",
            "host",
            "content-length",
        ]
    );
    assert_eq!(
        head.value("authorization"),
        Some("Bearer backend-secret-key")
    );
    assert_eq!(
        head.value("host"),
        Some(upstream.url().trim_start_matches("http://"))
    );
}

#[tokio::test]
async fn a_listed_host_holds_the_value_an_unlisted_one_does() {
    let upstream = RawUpstream::start().await;
    let url = upstream.url().replace("127.0.0.1", "LocalHost");
    let plain = ConfigDir::new(&config_at(&url, "", ""), &[]);
    let ordered = ConfigDir::new(
        &config_at(&url, "emulate = \"client\"", ""),
        &[(
            "client",
            "http_version = \"1.1\"\nheader_order = [\"host\"]\n",
        )],
    );

    let router = TestRouter::start_at(&plain.config()).await;
    let head = head_sent(&router, &upstream, &chat("qwen")).await;
    let unlisted = head.value("host").expect("a host header").to_owned();
    assert_eq!(unlisted, url.trim_start_matches("http://").to_lowercase());

    let router = TestRouter::start_at(&ordered.config()).await;
    let head = head_sent(&router, &upstream, &chat("qwen")).await;
    assert_eq!(head.names().first(), Some(&"host"));
    assert_eq!(head.value("host"), Some(unlisted.as_str()));
}

/// `listed`, then the names of `without` that are not among them.
fn listed_first<'a>(listed: &[&'a str], without: &'a [String]) -> Vec<&'a str> {
    let rest = without
        .iter()
        .map(String::as_str)
        .filter(|name| !listed.contains(name));
    listed.iter().copied().chain(rest).collect()
}

/// The credential line [`config_at`] writes.
const STATIC: &str = r#"credential = { kind = "static", value = "backend-secret-key" }"#;

#[tokio::test]
async fn the_authorization_of_a_url_with_userinfo_stands_where_the_order_lists_it() {
    let listed = ["content-type", "authorization"];
    let preset = "http_version = \"1.1\"\nheader_order = [\"content-type\", \"authorization\"]\n";
    let upstream = RawUpstream::start().await;
    let url = upstream.url().replace("http://", "http://user:pw@");
    for (credential, value) in [
        (STATIC, "Bearer backend-secret-key"),
        ("credential = { kind = \"none\" }", "Basic dXNlcjpwdw=="),
    ] {
        let config = |backend: &str| config_at(&url, backend, "").replace(STATIC, credential);
        let plain = ConfigDir::new(&config(""), &[]);
        let ordered = ConfigDir::new(&config("emulate = \"client\""), &[("client", preset)]);

        let router = TestRouter::start_at(&plain.config()).await;
        let head = head_sent(&router, &upstream, &chat("qwen")).await;
        let without: Vec<String> = head.names().into_iter().map(str::to_owned).collect();
        assert_eq!(without[0], "authorization", "{credential}");
        assert_eq!(head.value("authorization"), Some(value));

        let router = TestRouter::start_at(&ordered.config()).await;
        let head = head_sent(&router, &upstream, &chat("qwen")).await;
        assert_eq!(
            head.names(),
            listed_first(&listed, &without),
            "{credential}"
        );
        assert_eq!(head.value("authorization"), Some(value));
    }
}

/// The status line of the answer to `body`, posted by a client that sends no
/// `accept` header.
async fn post_without_accept(router: &TestRouter, body: &Value) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let body = body.to_string();
    let request = format!(
        "POST /v1/messages HTTP/1.1\r\nhost: {}\r\nx-api-key: {TOKEN}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        router.addr,
        body.len()
    );
    let mut socket = tokio::net::TcpStream::connect(router.addr).await.unwrap();
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    socket.read_to_string(&mut answer).await.unwrap();
    answer.lines().next().unwrap_or_default().to_owned()
}

#[tokio::test]
async fn the_accept_the_http_client_adds_stands_where_the_order_lists_it() {
    let listed = ["accept", "content-type"];
    let preset = "http_version = \"1.1\"\nheader_order = [\"accept\", \"content-type\"]\n";
    let upstream = RawUpstream::start().await;
    let plain = ConfigDir::new(&config_at(&upstream.url(), "", ""), &[]);
    let ordered = ConfigDir::new(
        &config_at(&upstream.url(), "emulate = \"client\"", ""),
        &[("client", preset)],
    );

    let router = TestRouter::start_at(&plain.config()).await;
    let status = post_without_accept(&router, &chat("qwen")).await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    let head = upstream.last();
    let without: Vec<String> = head.names().into_iter().map(str::to_owned).collect();
    assert_eq!(
        without[without.len() - 3..],
        ["accept", "host", "content-length"]
    );
    assert_eq!(head.value("accept"), Some("*/*"));

    let router = TestRouter::start_at(&ordered.config()).await;
    let status = post_without_accept(&router, &chat("qwen")).await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    let head = upstream.last();
    assert_eq!(head.names(), listed_first(&listed, &without));
    assert_eq!(head.value("accept"), Some("*/*"));
}

#[tokio::test]
async fn a_retry_with_a_new_credential_and_computed_headers_is_sent_in_the_order() {
    let upstream = RawUpstream::start().await;
    let dir = tempfile::tempdir().unwrap();
    let used = dir.path().join("used");
    let credential = format!(
        "credential = {{ kind = \"command\", command = 'if [ -e {used} ]; then echo fresh-key; else touch {used}; echo {REFUSED_KEY}; fi' }}",
        used = used.display()
    );
    let script = dir.path().join("headers.sh");
    std::fs::write(
        &script,
        r#"printf '{"x-gw-attempt":"%s"}' "$ANTHROXY_ATTEMPT""#,
    )
    .unwrap();
    let backend = format!(
        "emulate = \"client\"\n\n[backends.mock.header_command]\ncommand = \"sh {}\"",
        script.display()
    );
    let listed = ["x-gw-attempt", "content-length", "authorization"];
    let preset = "http_version = \"1.1\"\nheader_order = [\"x-gw-attempt\", \"content-length\", \"authorization\"]\n";
    let presets = ConfigDir::new(
        &config_at(&upstream.url(), &backend, "").replace(STATIC, &credential),
        &[("client", preset)],
    );
    let router = TestRouter::start_at(&presets.config()).await;

    let head = head_sent(&router, &upstream, &chat("qwen")).await;
    assert_eq!(
        head.value("x-gw-attempt"),
        Some("2"),
        "the attempt after the refused one"
    );
    assert_eq!(head.value("authorization"), Some("Bearer fresh-key"));
    assert_eq!(head.names()[..3], listed);
}

#[tokio::test]
async fn headers_the_order_does_not_list_follow_in_the_order_they_had() {
    let listed = "http_version = \"1.1\"\nheader_order = [\"host\", \"x-absent\", \"x-app\"]\n";
    let upstream = RawUpstream::start().await;
    let plain = ConfigDir::new(&config_at(&upstream.url(), "live_models = true", ""), &[]);
    let ordered = ConfigDir::new(
        &config_at(
            &upstream.url(),
            "emulate = \"client\"\nlive_models = true",
            "",
        ),
        &[("client", listed)],
    );

    let router = TestRouter::start_at(&plain.config()).await;
    let head = head_sent(&router, &upstream, &chat("qwen")).await;
    let without: Vec<String> = head.names().into_iter().map(str::to_owned).collect();
    assert_eq!(
        without[without.len() - 3..],
        ["authorization", "host", "content-length"]
    );
    assert!(without.contains(&"x-app".to_owned()), "{without:?}");

    let router = TestRouter::start_at(&ordered.config()).await;
    let head = head_sent(&router, &upstream, &chat("qwen")).await;
    let mut expected = vec!["host", "x-app"];
    expected.extend(
        without
            .iter()
            .map(String::as_str)
            .filter(|name| !["host", "x-app"].contains(name)),
    );
    assert_eq!(head.names(), expected);

    let res = router.get("/v1/models").send().await.unwrap();
    assert_eq!(res.status(), 200);
    let head = upstream.last();
    assert_eq!(head.request_line(), "GET /v1/models HTTP/1.1");
    assert_eq!(
        head.names().first(),
        Some(&"host"),
        "the model list is sent in the preset's order too"
    );
}

#[tokio::test]
async fn a_preset_writes_header_names_in_title_case() {
    let upstream = RawUpstream::start().await;
    let titled = format!("header_case = \"title\"\n{OPENCODE_HEADERS}");
    let dir = ConfigDir::new(
        &config_at(
            &upstream.url(),
            "emulate = \"client\"\nlive_models = true",
            "emulate = \"lower\"",
        ),
        &[
            ("client", &titled),
            ("lower", "http_version = \"1.1\"\nheader_case = \"lower\"\n"),
        ],
    );
    let router = TestRouter::start_at(&dir.config()).await;

    let head = head_sent(&router, &upstream, &chat("qwen")).await;
    assert_eq!(
        head.names(),
        [
            "Authorization",
            "Content-Type",
            "User-Agent",
            "X-Opencode-Session-Id",
            "X-Session-Affinity",
            "X-Session-Id",
            "Accept",
            "Host",
            "Content-Length",
        ]
    );
    assert_eq!(head.value("Content-Type"), Some("application/json"));
    assert_eq!(
        head.value("Authorization"),
        Some("Bearer backend-secret-key")
    );

    let res = router.get("/v1/models").send().await.unwrap();
    assert_eq!(res.status(), 200);
    let names = upstream.last().names().join(" ");
    assert!(
        names.starts_with("Authorization User-Agent Accept Host"),
        "the model list is written the same way: {names}"
    );

    let head = head_sent(&router, &upstream, &chat("other")).await;
    let names = head.names();
    assert!(
        names.contains(&"authorization") && names.contains(&"content-type"),
        "{names:?}"
    );
}

/// The `meta.json` of the one exchange recorded under `dir`, once it is
/// finished.
async fn recorded_meta(dir: &std::path::Path) -> String {
    for _ in 0..100 {
        let entry = std::fs::read_dir(dir)
            .ok()
            .and_then(|mut entries| entries.next())
            .and_then(|entry| entry.ok());
        if let Some(entry) = entry
            && let Ok(meta) = std::fs::read_to_string(entry.path().join("meta.json"))
            && meta.contains("\"outcome\"")
        {
            return meta;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("no finished exchange was recorded in {}", dir.display());
}

#[tokio::test]
async fn a_preset_header_is_reported_like_one_the_backend_forces() {
    let upstream = backend().await;
    let log = tempfile::tempdir().unwrap();
    let dir = ConfigDir::new(
        &config(
            &upstream,
            &format!(
                "emulate = {{ preset = \"client\", version = \"9.9.9-variable\" }}\n\n[logging]\nbody_dir = \"{}\"",
                log.path().display()
            ),
            "",
        ),
        &[("client", CLIENT)],
    );
    let router = TestRouter::start_at(&dir.config()).await;
    let received = sent(&router, &upstream, &chat("qwen")).await;
    assert_eq!(
        received.header("user-agent"),
        Some("opencode/9.9.9-variable")
    );
    assert_eq!(received.header("x-session-id"), Some("ses_abc-123"));

    let meta = recorded_meta(log.path()).await;
    assert!(!meta.contains("9.9.9-variable"), "{meta}");
    assert!(!meta.contains("ses_abc-123"), "{meta}");
    let meta: Value = serde_json::from_str(&meta).unwrap();
    let headers = meta["request_headers"].as_array().unwrap();
    for name in ["user-agent", "x-session-id"] {
        let header = headers
            .iter()
            .find(|header| header["name"] == name)
            .expect("a header the backend received is recorded");
        assert_eq!(header["value"], "<redacted>", "{name}");
        assert_eq!(header["source"], "backend", "{name}");
    }

    let status: Value = router
        .get("/api/status")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        status["config"]["backends"]["mock"]["emulate"],
        json!({"preset": "client", "variables": {"version": "9.9.9-variable"}})
    );
    assert_eq!(status["config"]["backends"]["mock"]["headers"], json!({}));
    assert_eq!(status["config"]["models"][0]["emulate"], Value::Null);
}

/// One tool round trip the way Claude Code 2.1.328 sends its second request:
/// the turn that only called a tool comes back with its `thinking` block,
/// followed by the tool's result and a system message.
fn claude_code_round_trip(model: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": 32000,
        "stream": true,
        "system": [{"type": "text", "text": "Lead.", "cache_control": {"type": "ephemeral"}}],
        "metadata": {"user_id": "{\"device_id\":\"d1\",\"session_id\":\"abc-123\"}"},
        "output_config": {"effort": "high"},
        "tools": [{
            "name": "Read",
            "description": "Read a file",
            "input_schema": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "additionalProperties": false
            }
        }],
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "read a.rs"}]},
            {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "The user wants a.rs.", "signature": ""},
                {"type": "tool_use", "id": "call_1", "name": "Read", "input": {"path": "a.rs"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "call_1", "content": "fn main() {}"}
            ]},
            {"role": "system", "content": "Later."}
        ]
    })
}

/// `request` with the headers Claude Code adds to it.
fn from_claude_code(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    request
        .header("accept", "application/json")
        .header("user-agent", "claude-cli/2.1.328 (external, cli)")
        .header("anthropic-beta", "claude-code-20250219")
        .header("anthropic-dangerous-direct-browser-access", "true")
        // Stands for whatever Claude Code sends under this prefix.
        .header("anthropic-client-any", "1")
        .header("x-app", "cli")
        .header("x-claude-code-session-id", "abc-123")
        .header("x-stainless-lang", "js")
        .header("x-stainless-retry-count", "0")
        .header("x-stainless-timeout", "600")
}

#[tokio::test]
async fn the_built_in_opencode_preset_writes_a_request_as_opencode_1_19_6_does() {
    let upstream = RawUpstream::start().await;
    let router = TestRouter::start(&config_at(
        &upstream.url(),
        "emulate = \"opencode\"\nlive_models = true",
        "emulate = { preset = \"opencode\", version = \"1.20.0\" }",
    ))
    .await;

    let res = from_claude_code(router.post("/v1/messages", &claude_code_round_trip("qwen")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let answer = res.text().await.unwrap();
    assert!(answer.contains("event: message_stop"), "{answer}");

    let head = upstream.last();
    assert_eq!(head.request_line(), "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(
        head.names(),
        [
            "Authorization",
            "Content-Type",
            "User-Agent",
            "X-Opencode-Session-Id",
            "X-Session-Affinity",
            "X-Session-Id",
            "Accept",
            "Host",
            "Content-Length",
        ],
        "the headers of opencode in its order, and none of Claude Code's own"
    );
    assert_eq!(
        head.value("Authorization"),
        Some("Bearer backend-secret-key")
    );
    assert_eq!(head.value("Content-Type"), Some("application/json"));
    assert_eq!(head.value("User-Agent"), Some("opencode/1.19.6"));
    assert_eq!(head.value("Accept"), Some("*/*"));
    for session in [
        "X-Opencode-Session-Id",
        "X-Session-Affinity",
        "X-Session-Id",
    ] {
        assert_eq!(head.value(session), Some("abc-123"), "{session}");
    }

    let raw = upstream.last_body();
    let body: Value = serde_json::from_str(&raw).unwrap();
    let fields: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        fields,
        [
            "model",
            "max_tokens",
            "messages",
            "tools",
            "tool_choice",
            "stream",
            "stream_options"
        ],
        "no `user`, no `reasoning_effort`, and opencode's order"
    );
    assert_eq!(body["max_tokens"], 32000);
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
    assert_eq!(
        body["tools"],
        json!([{"type": "function", "function": {
            "name": "Read",
            "description": "Read a file",
            "parameters": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "additionalProperties": false
            }
        }}]),
        "a tool definition carries no `strict`"
    );

    let messages = body["messages"].as_array().unwrap();
    let roles: Vec<&str> = messages
        .iter()
        .map(|message| message["role"].as_str().unwrap())
        .collect();
    assert_eq!(
        roles,
        ["system", "user", "assistant", "tool"],
        "one leading system message"
    );
    let system = messages[0]["content"].as_str().unwrap();
    assert!(
        system.starts_with("Lead.") && system.ends_with("Later."),
        "{system}"
    );
    assert_eq!(
        messages[2].to_string(),
        concat!(
            r#"{"role":"assistant","content":"","reasoning_content":"The user wants a.rs.","#,
            r#""tool_calls":[{"id":"call_1","type":"function","function":{"name":"Read","arguments":"{\"path\":\"a.rs\"}"}}]}"#
        ),
        "a turn that only called a tool, with the reasoning before it"
    );

    let res = from_claude_code(router.post("/v1/messages", &claude_code_round_trip("other")))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(upstream.last().value("User-Agent"), Some("opencode/1.20.0"));

    let res = router.get("/v1/models").send().await.unwrap();
    assert_eq!(res.status(), 200);
    let head = upstream.last();
    assert_eq!(head.request_line(), "GET /v1/models HTTP/1.1");
    assert_eq!(
        head.names(),
        ["Authorization", "User-Agent", "Accept", "Host"],
        "the model list carries the preset's headers too"
    );
}
