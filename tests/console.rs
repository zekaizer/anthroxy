//! The web console: its page and the `/api/` routes behind the router token
//! (ADR-0011).

mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anthroxy::activity::Source;
use anthroxy::config::{Config, process_env};
use anthroxy::server::Loaded;
use axum::response::Response;
use serde_json::{Value, json};
use support::mock_upstream::{echo, json_response};
use support::openai::sse_response;
use support::router::{TOKEN, config_with_backend, messages_body};
use support::{MockUpstream, TestRouter};

const SSE: &str = concat!(
    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[],\"model\":\"mock-fast-v1\",\"usage\":{\"input_tokens\":12,\"output_tokens\":1}}}\n\n",
    "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
    "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" there\"}}\n\n",
    "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
    "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
);

/// A backend answering `/v1/models` with context lengths and `/v1/messages`
/// with [`SSE`], or with a document when the request is not streamed.
fn backend(received: &support::mock_upstream::Received) -> Response {
    match received.path_and_query.as_str() {
        "/v1/models" => json_response(
            200,
            json!({"data": [
                {"id": "mock-fast-v1", "max_model_len": 32768},
                {"id": "smart", "max_model_len": 65536},
                {"id": "unconfigured-model"}
            ]}),
        ),
        // Paced, so the answer takes time after its first byte.
        _ if received.json()["stream"] == json!(true) => sse_response(
            SSE.split_inclusive("\n\n").map(str::to_owned).collect(),
            Duration::from_millis(20),
        ),
        _ => json_response(
            200,
            json!({"id": "msg_2", "type": "message", "role": "assistant", "model": "mock-fast-v1",
                   "content": [{"type": "text", "text": "Hi from a document"}],
                   "stop_reason": "end_turn", "usage": {"input_tokens": 3, "output_tokens": 4}}),
        ),
    }
}

async fn api(router: &TestRouter, path: &str) -> Value {
    let res = router.get(path).send().await.unwrap();
    assert_eq!(res.status(), 200, "{path}");
    res.json().await.unwrap()
}

async fn post_api(router: &TestRouter, path: &str, body: Value) -> (u16, Value) {
    let res = router
        .http
        .post(router.url(path))
        .header("authorization", format!("Bearer {TOKEN}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    (
        res.status().as_u16(),
        res.json().await.unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn the_page_is_served_without_a_token_under_a_strict_policy() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    let res = http.get(router.url("/")).send().await.unwrap();
    assert!(res.status().is_redirection(), "{}", res.status());
    assert_eq!(res.headers()["location"], "/ui/");

    for (path, content_type) in [
        ("/ui/", "text/html"),
        ("/ui/app.js", "text/javascript"),
        ("/ui/recordings.js", "text/javascript"),
        ("/ui/app.css", "text/css"),
    ] {
        let res = http.get(router.url(path)).send().await.unwrap();
        assert_eq!(res.status(), 200, "{path}");
        let headers = res.headers();
        assert!(
            headers["content-type"]
                .to_str()
                .unwrap()
                .starts_with(content_type),
            "{path}: {:?}",
            headers["content-type"]
        );
        let policy = headers["content-security-policy"].to_str().unwrap();
        assert!(policy.contains("script-src 'self'"), "{policy}");
        assert!(policy.contains("frame-ancestors 'none'"), "{policy}");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        let text = res.text().await.unwrap();
        assert!(!text.contains(TOKEN) && !text.contains("backend-secret-key"));
    }
    let page = http
        .get(router.url("/ui/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        page.contains("app.js") && page.contains("recordings.js") && page.contains("app.css"),
        "{page}"
    );
}

#[tokio::test]
async fn the_api_needs_the_router_token() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    for path in [
        "/api/status",
        "/api/requests",
        "/api/stats",
        "/api/env",
        "/api/recordings",
    ] {
        let res = router.http.get(router.url(path)).send().await.unwrap();
        assert_eq!(res.status(), 401, "{path}");
    }
    for path in ["/api/reload", "/api/probe", "/api/smoke"] {
        let res = router.http.post(router.url(path)).send().await.unwrap();
        assert_eq!(res.status(), 401, "{path}");
    }
    let res = router.get("/api/status").send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["cache-control"], "no-store");
}

#[tokio::test]
async fn status_reports_the_running_configuration_without_its_secrets() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(
        &upstream.url(),
        "\n[backends.mock.headers]\n\"x-secret-header\" = \"header-secret-value\"\n",
    ))
    .await;
    let res = router.get("/api/status").send().await.unwrap();
    let text = res.text().await.unwrap();
    for secret in [TOKEN, "backend-secret-key", "header-secret-value"] {
        assert!(!text.contains(secret), "status leaks `{secret}`: {text}");
    }
    let status: Value = serde_json::from_str(&text).unwrap();
    assert!(
        status["version"]
            .as_str()
            .unwrap()
            .starts_with(env!("CARGO_PKG_VERSION"))
    );
    assert!(status["uptime_s"].is_u64());
    assert_eq!(status["reloads"][0]["trigger"], "startup");
    assert_eq!(status["reloads"][0]["result"], "applied");

    let backend = &status["backends"][0];
    assert_eq!(backend["name"], "mock");
    assert_eq!(backend["kind"], "anthropic");
    assert_eq!(backend["credential"]["source"], "static");
    assert_eq!(backend["credential"]["masked"], "back…-key");

    let models: Vec<&str> = status["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(models, ["fast", "smart"]);
    assert_eq!(status["models"][0]["aliases"], json!(["claude-haiku-4-5"]));
    assert!(status["stats"]["dir"].is_string());
    assert_eq!(status["body_log"], Value::Null);

    let config = &status["config"];
    assert_eq!(config["server"]["token"], "<redacted>");
    assert_eq!(config["backends"]["mock"]["credential"]["kind"], "static");
    assert_eq!(
        config["backends"]["mock"]["credential"]["value"],
        "<redacted>"
    );
    assert_eq!(
        config["backends"]["mock"]["headers"]["x-secret-header"],
        "<redacted>"
    );
    assert_eq!(config["backends"]["mock"]["url"], upstream.url());
    assert_eq!(config["upstream"]["non_stream_timeout"], "15m");
    assert_eq!(config["upstream"]["stream_first_byte_timeout"], "5m");
    assert_eq!(config["upstream"]["stream_idle_timeout"], "1m");
}

#[tokio::test]
async fn requests_show_what_is_in_flight_and_what_finished() {
    let upstream = MockUpstream::start(|received| {
        if received.json()["model"] == "smart" {
            return json_response(
                400,
                json!({"type": "error", "error": {"type": "invalid_request_error", "message": "context_management: Extra inputs are not permitted"}}),
            );
        }
        sse_response(
            SSE.split_inclusive("\n\n").map(str::to_owned).collect(),
            Duration::from_millis(150),
        )
    })
    .await;
    let router = Arc::new(TestRouter::start(&config_with_backend(&upstream.url(), "")).await);
    let mut body = messages_body("fast");
    body["stream"] = json!(true);
    let res = router.post("/v1/messages", &body).send().await.unwrap();
    let id = res.headers()["x-request-id"].to_str().unwrap().to_owned();

    let requests = api(&router, "/api/requests").await;
    let in_flight = requests["in_flight"].as_array().unwrap();
    assert_eq!(in_flight.len(), 1, "{requests}");
    assert_eq!(in_flight[0]["id"], id.as_str());
    assert_eq!(in_flight[0]["session"], Value::Null);
    assert_eq!(in_flight[0]["outcome"], Value::Null);
    assert!(in_flight[0]["elapsed_ms"].is_u64());
    let _ = res.text().await.unwrap();

    let res = router
        .post("/v1/messages", &messages_body("smart"))
        .header("x-claude-code-session-id", "sess-console-1")
        .send()
        .await
        .unwrap();
    let failed = res.headers()["x-request-id"].to_str().unwrap().to_owned();
    let _ = res.text().await;

    let mut requests = Value::Null;
    for _ in 0..100 {
        requests = api(&router, "/api/requests").await;
        if requests["in_flight"].as_array().unwrap().is_empty()
            && requests["recent"].as_array().unwrap().len() == 2
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let recent = requests["recent"].as_array().unwrap();
    assert_eq!(recent.len(), 2, "{requests}");
    assert_eq!(recent[0]["id"], failed.as_str(), "newest first");
    assert_eq!(recent[0]["session"], "sess-console-1");
    assert_eq!(recent[1]["session"], Value::Null);
    assert_eq!(recent[1]["outcome"], "complete");
    assert_eq!(recent[1]["usage"]["output"], 2);
    let speed = recent[1]["output_tokens_per_second"]
        .as_f64()
        .unwrap_or_default();
    assert!(
        speed > 0.0 && speed < 100.0,
        "2 tokens over 5 paced frames: {requests}"
    );
    assert_eq!(
        recent[0]["output_tokens_per_second"],
        Value::Null,
        "a failure has no speed"
    );
    assert_eq!(
        recent[0]["error_body"],
        Value::Null,
        "the list leaves bodies out"
    );
    assert_eq!(recent[0]["hint_count"], 1);

    let streamed = api(&router, &format!("/api/requests/{id}")).await;
    assert!(
        streamed["output_tokens_per_second"].as_f64().is_some(),
        "{streamed}"
    );
    let detail = api(&router, &format!("/api/requests/{failed}")).await;
    assert!(
        detail["error_body"]
            .as_str()
            .unwrap()
            .contains("context_management")
    );
    assert_eq!(
        detail["hints"][0]["snippet"],
        "[backends.mock]\ndrop_fields = [\"context_management\"]"
    );
    let res = router
        .get("/api/requests/rtr_nothing")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);

    let res = router
        .post("/v1/messages", &messages_body("ghost"))
        .send()
        .await
        .unwrap();
    let _ = res.text().await;
    let status = api(&router, "/api/status").await;
    assert_eq!(status["names"][0]["name"], "ghost");
    assert_eq!(status["names"][0]["unknown"], 1);
}

#[tokio::test]
async fn statistics_survive_a_restart_and_are_reported_by_range() {
    let upstream = MockUpstream::start(backend).await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!("\n[stats]\ndir = \"{}\"\n", dir.path().display());
    let config = config_with_backend(&upstream.url(), &extra);
    {
        let router = TestRouter::start(&config).await;
        let mut body = messages_body("fast");
        body["stream"] = json!(true);
        let res = router.post("/v1/messages", &body).send().await.unwrap();
        let _ = res.text().await;
        for _ in 0..200 {
            if std::fs::read_dir(dir.path()).unwrap().count() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    let router = TestRouter::start(&config).await;
    let mut report = Value::Null;
    for _ in 0..200 {
        report = api(&router, "/api/stats?range=all").await;
        if report["report"]["total"]["requests"] == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(report["enabled"], true);
    assert_eq!(report["report"]["range"], "all");
    assert_eq!(report["report"]["total"]["requests"], 1, "{report}");
    assert_eq!(report["report"]["models"][0]["key"], "fast");
    assert_eq!(report["report"]["models"][0]["input_tokens"], 12);
    assert_eq!(report["report"]["days"].as_array().unwrap().len(), 1);

    let series = report["report"]["series"].as_array().unwrap();
    assert_eq!(report["report"]["bucket"], "1d");
    assert_eq!(series.len(), 1, "{report}");
    assert_eq!(series[0]["requests"], 1);

    let week = api(&router, "/api/stats").await;
    assert_eq!(week["report"]["range"], "7d", "the default range");
    assert_eq!(week["report"]["bucket"], "6h");
    assert_eq!(week["report"]["series"].as_array().unwrap().len(), 29);
    let res = router.get("/api/stats?range=forever").send().await.unwrap();
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn statistics_off_says_so() {
    let upstream = MockUpstream::start(echo).await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!(
        "\n[stats]\nenabled = false\ndir = \"{}\"\n",
        dir.path().join("off").display()
    );
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;
    let report = api(&router, "/api/stats").await;
    assert_eq!(report, json!({"enabled": false}));
}

#[tokio::test]
async fn env_uses_the_address_the_browser_reached() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let res = router
        .get("/api/env")
        .header("host", "192.168.5.5:8787")
        .send()
        .await
        .unwrap();
    let env: Value = res.json().await.unwrap();
    assert_eq!(env["base_url"], "http://192.168.5.5:8787");
    assert!(env["viewer"].as_str().unwrap().starts_with("127.0.0.1:"));
    let sh = env["sh"].as_str().unwrap();
    assert!(
        sh.contains("export ANTHROPIC_BASE_URL='http://192.168.5.5:8787'"),
        "{sh}"
    );
    assert!(
        sh.contains(&format!("export ANTHROPIC_AUTH_TOKEN='{TOKEN}'")),
        "{sh}"
    );
    assert!(
        !sh.contains("CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
        "unknown until a probe"
    );
    assert!(
        env["powershell"]
            .as_str()
            .unwrap()
            .contains("$env:ANTHROPIC_BASE_URL")
    );
    let json: Value = serde_json::from_str(env["json"].as_str().unwrap()).unwrap();
    assert_eq!(json["env"]["ANTHROPIC_MODEL"], "fast");
}

#[tokio::test]
async fn reload_runs_through_the_same_path_as_sighup() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let (status, event) = post_api(&router, "/api/reload", Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(event["result"], "rejected");

    let text = Arc::new(Mutex::new(format!(
        "{}\n[[models]]\nid = \"newcomer\"\nbackend = \"mock\"\n",
        config_with_backend(&upstream.url(), "")
    )));
    let source = text.clone();
    router.reload.set_loader(
        "console-test.toml".into(),
        Arc::new(move || {
            Ok(Loaded {
                config: Config::parse(&source.lock().unwrap(), process_env)?,
                restart_needed: Vec::new(),
            })
        }),
    );
    let (status, event) = post_api(&router, "/api/reload", Value::Null).await;
    assert_eq!(status, 200);
    assert_eq!(event["trigger"], "console");
    assert_eq!(event["result"], "applied");
    assert_eq!(event["models"], 3);
    let status = api(&router, "/api/status").await;
    assert_eq!(status["config_path"], "console-test.toml");
    assert_eq!(status["reloads"].as_array().unwrap().len(), 3);
    assert_eq!(status["models"][2]["id"], "newcomer");
}

#[tokio::test]
async fn probe_lists_each_backends_models_and_feeds_the_context_limit() {
    let upstream = MockUpstream::start(backend).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let (status, probe) = post_api(&router, "/api/probe", Value::Null).await;
    assert_eq!(status, 200, "{probe}");
    let mock = &probe["backends"][0];
    assert_eq!(mock["name"], "mock");
    assert_eq!(mock["credential"]["ok"], true);
    assert_eq!(mock["models"]["status"], 200);
    let listed = mock["models"]["listed"].as_array().unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0]["id"], "mock-fast-v1");
    assert_eq!(listed[0]["context_length"], 32768);
    assert_eq!(listed[0]["configured_as"], json!(["fast"]));
    assert_eq!(listed[2]["configured_as"], json!([]));
    let snippet = listed[2]["snippet"].as_str().unwrap();
    let parsed: toml::Value = toml::from_str(snippet).unwrap();
    let model = &parsed["models"].as_array().unwrap()[0];
    assert_eq!(model["backend"].as_str(), Some("mock"));
    assert_eq!(model["upstream_model"].as_str(), Some("unconfigured-model"));
    assert_eq!(probe["max_context_tokens"], 32768);

    let env = api(&router, "/api/env").await;
    assert!(
        env["sh"]
            .as_str()
            .unwrap()
            .contains("export CLAUDE_CODE_MAX_CONTEXT_TOKENS='32768'"),
        "{env}"
    );
}

#[tokio::test]
async fn probe_shows_the_request_it_sent_and_the_route_it_took() {
    let upstream = MockUpstream::start(backend).await;
    // `far` answers only through the proxy, which the mock plays as well.
    let extra = format!(
        r#"
[backends.far]
url = "http://127.0.0.1:1"
proxy = "http://alice:proxy-secret@{}"

[backends.far.headers]
"user-agent" = "claude-cli/2.0.0 (external, cli)"
"#,
        upstream.addr
    );
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;
    let (status, probe) = post_api(&router, "/api/probe", Value::Null).await;
    assert_eq!(status, 200, "{probe}");
    let named = |name: &str| {
        probe["backends"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == name)
            .unwrap()
            .clone()
    };
    let (far, mock) = (named("far"), named("mock"));

    assert_eq!(far["proxy"], format!("http://<redacted>@{}", upstream.addr));
    assert_eq!(mock["proxy"], Value::Null);
    assert_eq!(far["models"]["status"], 200, "{far}");
    assert_eq!(mock["request"]["method"], "GET");
    assert_eq!(mock["request"]["path"], "/v1/models");
    let has = |backend: &Value, list: &str, entry: Value| {
        backend
            .pointer(list)
            .and_then(Value::as_array)
            .is_some_and(|headers| headers.contains(&entry))
    };
    assert!(
        has(
            &mock,
            "/request/headers",
            json!({"name": "authorization", "value": "Bearer back…-key", "source": "credential"})
        ),
        "{mock}"
    );
    assert!(
        has(
            &far,
            "/request/headers",
            json!({"name": "user-agent", "value": "clau…cli)", "source": "backend"})
        ),
        "{far}"
    );
    assert!(
        has(
            &mock,
            "/models/headers",
            json!({"name": "content-type", "value": "application/json"})
        ),
        "{mock}"
    );
    let shown = probe.to_string();
    for secret in ["backend-secret-key", "proxy-secret", "claude-cli/2.0.0"] {
        assert!(!shown.contains(secret), "{secret} leaked: {shown}");
    }
}

#[tokio::test]
async fn probe_reads_lm_studio_context_from_its_native_list() {
    let upstream = MockUpstream::start(|received| match received.path_and_query.as_str() {
        "/v1/models" => json_response(200, json!({"data": [{"id": "gemma", "object": "model"}]})),
        "/api/v0/models" => json_response(
            200,
            json!({"data": [{"id": "gemma", "max_context_length": 131072, "loaded_context_length": 32768}]}),
        ),
        _ => json_response(404, json!({})),
    })
    .await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;
    let (_, probe) = post_api(&router, "/api/probe", Value::Null).await;
    assert_eq!(
        probe["backends"][0]["models"]["listed"][0]["context_length"],
        32768
    );
}

#[tokio::test]
async fn a_test_request_goes_through_the_route_and_is_marked_as_the_consoles() {
    let upstream = MockUpstream::start(backend).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;

    let (status, smoke) = post_api(
        &router,
        "/api/smoke",
        json!({"model": "fast", "stream": true}),
    )
    .await;
    assert_eq!(status, 200, "{smoke}");
    assert_eq!(smoke["status"], 200);
    assert_eq!(smoke["text"], "Hello there");
    assert_eq!(smoke["usage"]["output"], 2);
    assert_eq!(smoke["backend"], "mock");
    assert_eq!(smoke["upstream_model"], "mock-fast-v1");
    assert!(smoke["ttfb_ms"].is_u64() && smoke["duration_ms"].is_u64());
    assert!(
        smoke["output_tokens_per_second"]
            .as_f64()
            .is_some_and(|s| s > 0.0),
        "{smoke}"
    );
    let sent = upstream.last();
    assert_eq!(sent.json()["model"], "mock-fast-v1");
    assert_eq!(sent.json()["stream"], true);
    assert_eq!(
        sent.header("authorization"),
        Some("Bearer backend-secret-key")
    );
    let id = smoke["request_id"].as_str().unwrap();
    let view = router.reload.activity.find(id).unwrap();
    assert_eq!(view.source, Source::Console);

    let (_, smoke) = post_api(
        &router,
        "/api/smoke",
        json!({"model": "fast", "stream": false}),
    )
    .await;
    assert_eq!(smoke["text"], "Hi from a document");
    assert_eq!(smoke["usage"]["input"], 3);
    assert_eq!(
        smoke["output_tokens_per_second"],
        Value::Null,
        "not streamed"
    );

    let (_, smoke) = post_api(
        &router,
        "/api/smoke",
        json!({"model": "ghost", "stream": false}),
    )
    .await;
    assert_eq!(smoke["status"], 404);
    assert!(
        smoke["error"].as_str().unwrap().contains("ghost"),
        "{smoke}"
    );

    let (status, _) = post_api(&router, "/api/smoke", json!({"stream": false})).await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn recordings_can_be_listed_read_and_deleted() {
    let upstream = MockUpstream::start(echo).await;
    let dir = tempfile::tempdir().unwrap();
    let extra = format!("\n[logging]\nbody_dir = \"{}\"\n", dir.path().display());
    let router = TestRouter::start(&config_with_backend(&upstream.url(), &extra)).await;
    let mut ids = Vec::new();
    for model in ["fast", "smart"] {
        let mut body = messages_body(model);
        if model == "smart" {
            // The next request of a turn: it returns a tool result.
            body["messages"] = json!([
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "Read", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": "ok"}]}
            ]);
        }
        let res = router
            .post("/v1/messages", &body)
            .header("x-claude-code-session-id", format!("session-{model}"))
            .send()
            .await
            .unwrap();
        ids.push(res.headers()["x-request-id"].to_str().unwrap().to_owned());
        let _ = res.text().await;
    }
    let mut listing = Value::Null;
    for _ in 0..200 {
        listing = api(&router, "/api/recordings").await;
        let entries = listing["entries"].as_array().unwrap();
        if entries.len() == 2 && entries.iter().all(|e| e["outcome"].is_string()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let entries = listing["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2, "{listing}");
    assert_eq!(listing["total"], 2, "{listing}");
    assert_eq!(entries[0]["request_id"], ids[1].as_str(), "newest first");
    assert_eq!(entries[0]["model"], "smart");
    assert_eq!(entries[0]["status"], 200);
    assert_eq!(entries[0]["prompt"], "hi");
    assert_eq!(entries[0]["step"], "← Read");
    assert_eq!(entries[0]["stream"], false, "{listing}");
    assert_eq!(entries[0]["messages"], 3);
    assert_eq!(entries[1]["step"], Value::Null, "{listing}");
    assert_eq!(entries[0]["session"], "session-smart");
    assert!(entries[0]["bytes"].as_u64().unwrap() > 0);
    assert!(
        entries[0]["files"]
            .as_array()
            .unwrap()
            .contains(&json!("request.json"))
    );

    let name = entries[1]["name"].as_str().unwrap().to_owned();
    let res = router
        .get(&format!("/api/recordings/{name}/meta.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert!(
        res.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    assert!(res.text().await.unwrap().contains(&ids[0]));
    for bad in [
        format!("/api/recordings/{name}/secret.txt"),
        "/api/recordings/..%2F..%2Fetc/meta.json".to_owned(),
        "/api/recordings/not-an-entry/meta.json".to_owned(),
    ] {
        let res = router.get(&bad).send().await.unwrap();
        assert_eq!(res.status(), 404, "{bad}");
    }

    let res = router
        .http
        .delete(router.url(&format!("/api/recordings/{name}")))
        .header("x-api-key", TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 204);
    assert!(!dir.path().join(&name).exists());
    let res = router
        .http
        .delete(router.url("/api/recordings"))
        .header("x-api-key", TOKEN)
        .send()
        .await
        .unwrap();
    let removed: Value = res.json().await.unwrap();
    assert_eq!(removed["removed"], 1);
    assert_eq!(api(&router, "/api/recordings").await["entries"], json!([]));
}

/// The header's one job is to say whether anything is wrong, so `/api/status`
/// has to answer that without the reader opening a table.
#[tokio::test]
async fn health_names_a_backend_whose_credential_is_not_in_hand() {
    let upstream = MockUpstream::start(backend).await;
    let router = TestRouter::start(&format!(
        r#"
[server]
listen = "127.0.0.1:0"
token = "{TOKEN}"

[backends.mock]
url = "{url}"
credential = {{ kind = "command", command = "sh -c 'echo no credential today >&2; exit 3'" }}

[[models]]
id = "fast"
backend = "mock"
upstream_model = "mock-fast-v1"
"#,
        url = upstream.url()
    ))
    .await;

    // Nothing has asked for the credential yet: that is unknown, not broken.
    let health = &api(&router, "/api/status").await["health"];
    assert_eq!(health["level"], "attention");
    assert_eq!(health["problems"][0]["kind"], "credential");
    assert_eq!(health["problems"][0]["backend"], "mock");

    // One request runs the command, which fails; now it is broken.
    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert!(res.status().is_server_error() || res.status().is_client_error());

    let health = &api(&router, "/api/status").await["health"];
    assert_eq!(health["level"], "trouble");
    let problem = &health["problems"][0];
    assert_eq!(problem["kind"], "credential");
    assert_eq!(problem["backend"], "mock");
    assert!(
        problem["detail"].as_str().unwrap().contains("exit"),
        "the reason the command gave is missing: {problem}"
    );
}

/// A backend that works and a router that has served nothing is healthy, and
/// a request that failed a moment ago is worth raising.
#[tokio::test]
async fn health_raises_a_request_that_failed_a_moment_ago() {
    let upstream =
        MockUpstream::start(|_| json_response(500, json!({"error": "upstream is out"}))).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;

    let health = &api(&router, "/api/status").await["health"];
    assert_eq!(health["level"], "ok");
    assert_eq!(health["problems"], json!([]));

    let res = router
        .post("/v1/messages", &messages_body("fast"))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 500);

    let health = &api(&router, "/api/status").await["health"];
    assert_eq!(health["level"], "attention");
    assert_eq!(health["problems"][0]["kind"], "errors");
    assert_eq!(health["problems"][0]["count"], 1);
}
