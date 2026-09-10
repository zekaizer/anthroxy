mod support;

use std::sync::{Arc, Mutex};

use anthroxy::config::{Config, process_env};
use anthroxy::server::{Loaded, ReloadOutcome, ReloadTrigger};
use serde_json::json;
use support::mock_upstream::echo;
use support::router::{TOKEN, config_with_backend, model_ids};
use support::{MockUpstream, TestRouter};

fn parse(text: &str) -> Config {
    Config::parse(text, process_env).unwrap()
}

async fn list_models(router: &TestRouter, token: &str) -> (u16, Vec<String>) {
    let res = router
        .http
        .get(router.url("/v1/models"))
        .header("x-api-key", token)
        .send()
        .await
        .unwrap();
    let status = res.status().as_u16();
    let ids = if status == 200 {
        model_ids(&res.json().await.unwrap())
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
        list_models(&router, TOKEN).await,
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
    assert_eq!(list_models(&router, TOKEN).await.0, 401);
    assert_eq!(
        list_models(&router, "rotated-token").await,
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
    assert_eq!(res.headers()["x-anthroxy-backend"], "other");
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
        r#"credential = { kind = "env", name = "ANTHROXY_TEST_DEFINITELY_UNSET" }"#,
    );
    let err = router.reload.apply(&parse(&broken)).unwrap_err();
    assert!(
        err.to_string().contains("ANTHROXY_TEST_DEFINITELY_UNSET"),
        "{err}"
    );
    assert_eq!(
        list_models(&router, TOKEN).await,
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
    assert_eq!(list_models(&router, TOKEN).await.0, 200);
}

#[tokio::test]
async fn reload_runs_the_loader_and_keeps_a_history() {
    let upstream = MockUpstream::start(echo).await;
    let router = TestRouter::start(&config_with_backend(&upstream.url(), "")).await;

    let event = router.reload.reload(ReloadTrigger::Console);
    match &event.outcome {
        ReloadOutcome::Rejected { error } => {
            assert!(error.contains("without a configuration file"), "{error}")
        }
        other => panic!("{other:?}"),
    }

    let text = Arc::new(Mutex::new(format!(
        "{}\n[[models]]\nid = \"newcomer\"\nbackend = \"mock\"\n",
        config_with_backend(&upstream.url(), "")
    )));
    let source = text.clone();
    router.reload.set_loader(
        "test.toml".into(),
        Arc::new(move || {
            let config = Config::parse(&source.lock().unwrap(), process_env)?;
            Ok(Loaded {
                config,
                restart_needed: vec!["logging level or format".to_owned()],
            })
        }),
    );
    let event = router.reload.reload(ReloadTrigger::Signal);
    assert_eq!(
        event.outcome,
        ReloadOutcome::Applied {
            backends: 1,
            models: 3,
            restart_needed: vec!["logging level or format".to_owned()],
        }
    );
    assert!(
        list_models(&router, TOKEN)
            .await
            .1
            .contains(&"newcomer".to_owned())
    );

    *text.lock().unwrap() = "[server]\ntoken = \"\"\n".to_owned();
    let event = router.reload.reload(ReloadTrigger::Console);
    match &event.outcome {
        ReloadOutcome::Rejected { error } => assert!(error.contains("server.token"), "{error}"),
        other => panic!("{other:?}"),
    }
    assert!(
        list_models(&router, TOKEN)
            .await
            .1
            .contains(&"newcomer".to_owned()),
        "a rejected reload keeps the running configuration"
    );

    let triggers: Vec<ReloadTrigger> = router.reload.reloads().iter().map(|e| e.trigger).collect();
    assert_eq!(
        triggers,
        [
            ReloadTrigger::Startup,
            ReloadTrigger::Console,
            ReloadTrigger::Signal,
            ReloadTrigger::Console
        ]
    );
    assert!(matches!(
        router.reload.reloads()[0].outcome,
        ReloadOutcome::Applied { models: 2, .. }
    ));
}
