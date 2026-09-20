//! The operator helper `scripts/xai-oauth` against a mock token endpoint,
//! and the command-credential JSON contract on its stdout.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::Form;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::Deserialize;
use serde_json::{Value, json};

use anthroxy::config::{CommandOutput, CredentialHeader};
use anthroxy::credential::{CommandCredential, CredentialSource};

const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";

fn script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/xai-oauth")
}

fn python() -> Command {
    let mut cmd = Command::new("python3");
    cmd.arg(script()).env_remove("ANTHROXY_CONFIG");
    cmd
}

fn far_expiry() -> String {
    (jiff::Timestamp::now() + jiff::SignedDuration::from_secs(3600)).to_string()
}

fn expired() -> String {
    (jiff::Timestamp::now() - jiff::SignedDuration::from_secs(30)).to_string()
}

fn write_store(path: &Path, access: &str, refresh: &str, expires_at: &str, token_url: &str) {
    let record = json!({
        "version": 1,
        "access_token": access,
        "refresh_token": refresh,
        "expires_at": expires_at,
        "client_id": CLIENT_ID,
        "token_endpoint": token_url,
    });
    std::fs::write(path, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
}

#[derive(Clone)]
struct AuthState {
    refreshes: Arc<AtomicUsize>,
    device_polls: Arc<AtomicUsize>,
}

#[derive(Deserialize)]
struct TokenForm {
    grant_type: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    device_code: String,
    #[serde(default)]
    client_id: String,
}

#[derive(Deserialize)]
struct DeviceForm {
    client_id: String,
}

async fn token(State(state): State<AuthState>, Form(form): Form<TokenForm>) -> Response {
    assert_eq!(form.client_id, CLIENT_ID);
    match form.grant_type.as_str() {
        "refresh_token" => {
            assert_eq!(form.refresh_token, "rt-old");
            let n = state.refreshes.fetch_add(1, Ordering::SeqCst) + 1;
            axum::Json(json!({
                "access_token": format!("at-refreshed-{n}"),
                "refresh_token": "rt-new",
                "expires_in": 3600,
                "token_type": "Bearer"
            }))
            .into_response()
        }
        "urn:ietf:params:oauth:grant-type:device_code" => {
            assert_eq!(form.device_code, "dc-1");
            let n = state.device_polls.fetch_add(1, Ordering::SeqCst) + 1;
            if n == 1 {
                return (
                    StatusCode::BAD_REQUEST,
                    axum::Json(json!({"error": "authorization_pending"})),
                )
                    .into_response();
            }
            axum::Json(json!({
                "access_token": "at-login",
                "refresh_token": "rt-login",
                "expires_in": 1200,
                "token_type": "Bearer"
            }))
            .into_response()
        }
        other => panic!("unexpected grant_type {other}"),
    }
}

async fn device_code(Form(form): Form<DeviceForm>) -> axum::Json<Value> {
    assert_eq!(form.client_id, CLIENT_ID);
    axum::Json(json!({
        "device_code": "dc-1",
        "user_code": "WXYZ-0001",
        "verification_uri": "https://auth.x.ai/device",
        "expires_in": 15,
        "interval": 0
    }))
}

struct MockAuth {
    token_url: String,
    device_url: String,
    refreshes: Arc<AtomicUsize>,
    device_polls: Arc<AtomicUsize>,
    _task: tokio::task::JoinHandle<()>,
}

impl MockAuth {
    async fn start() -> Self {
        let refreshes = Arc::new(AtomicUsize::new(0));
        let device_polls = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route("/oauth2/token", post(token))
            .route("/oauth2/device/code", post(device_code))
            .with_state(AuthState {
                refreshes: refreshes.clone(),
                device_polls: device_polls.clone(),
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            token_url: format!("http://{addr}/oauth2/token"),
            device_url: format!("http://{addr}/oauth2/device/code"),
            refreshes,
            device_polls,
            _task: task,
        }
    }
}

fn run_print(store: &Path) -> (i32, String, String) {
    let out = python()
        .args(["--store", store.to_str().unwrap(), "print"])
        .output()
        .expect("python3 scripts/xai-oauth");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn helper_script_is_in_the_tree() {
    let path = script();
    assert!(path.is_file(), "missing {}", path.display());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("auth.x.ai/oauth2/token"));
    assert!(text.contains(CLIENT_ID));
}

#[tokio::test(flavor = "multi_thread")]
async fn command_credential_accepts_helper_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("xai-oauth.json");
    write_store(
        &store,
        "access-from-helper",
        "rt-old",
        "2099-01-01T00:00:00Z",
        "http://127.0.0.1:1/oauth2/token",
    );
    let source = CommandCredential::new(
        format!(
            "python3 {} --store {} print",
            script().display(),
            store.display()
        ),
        CommandOutput::Json,
        CredentialHeader::bearer(),
        Duration::from_secs(60),
        Duration::from_secs(10),
    );
    let cred = source.credential().await.unwrap().unwrap();
    assert_eq!(
        cred.header_pair().1.to_str().unwrap(),
        "Bearer access-from-helper"
    );
    assert!(source.status().expires_at.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn print_serves_fixture_store_without_refresh() {
    let mock = MockAuth::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("xai-oauth.json");
    write_store(
        &store,
        "at-fixture",
        "rt-old",
        &far_expiry(),
        &mock.token_url,
    );

    let (code, stdout, stderr) = run_print(&store);
    assert_eq!(code, 0, "stderr={stderr}");
    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed["token"], "at-fixture");
    assert_eq!(mock.refreshes.load(Ordering::SeqCst), 0);

    let source = CommandCredential::new(
        format!(
            "python3 {} --store {} print",
            script().display(),
            store.display()
        ),
        CommandOutput::Json,
        CredentialHeader::bearer(),
        Duration::from_secs(60),
        Duration::from_secs(10),
    );
    let cred = source.credential().await.unwrap().unwrap();
    assert_eq!(cred.header_pair().1.to_str().unwrap(), "Bearer at-fixture");
}

#[tokio::test(flavor = "multi_thread")]
async fn print_refreshes_expired_token_and_updates_store() {
    let mock = MockAuth::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("xai-oauth.json");
    write_store(&store, "at-old", "rt-old", &expired(), &mock.token_url);

    let (code, stdout, stderr) = run_print(&store);
    assert_eq!(code, 0, "stderr={stderr}");
    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed["token"], "at-refreshed-1");
    assert!(parsed["expires_at"].as_str().unwrap().len() > 10);
    assert_eq!(mock.refreshes.load(Ordering::SeqCst), 1);

    let stored: Value = serde_json::from_slice(&std::fs::read(&store).unwrap()).unwrap();
    assert_eq!(stored["access_token"], "at-refreshed-1");
    assert_eq!(stored["refresh_token"], "rt-new");

    let (code2, stdout2, stderr2) = run_print(&store);
    assert_eq!(code2, 0, "stderr={stderr2}");
    let parsed2: Value = serde_json::from_str(stdout2.trim()).unwrap();
    assert_eq!(parsed2["token"], "at-refreshed-1");
    assert_eq!(mock.refreshes.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_polls_until_token() {
    let mock = MockAuth::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("xai-oauth.json");
    let out = python()
        .args([
            "--store",
            store.to_str().unwrap(),
            "--token-url",
            &mock.token_url,
            "--device-url",
            &mock.device_url,
            "login",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("WXYZ-0001"), "{stderr}");
    assert!(out.stdout.is_empty(), "login must not print the token");
    assert_eq!(mock.device_polls.load(Ordering::SeqCst), 2);

    let (code, stdout, stderr) = run_print(&store);
    assert_eq!(code, 0, "stderr={stderr}");
    let parsed: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!(parsed["token"], "at-login");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&store).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}

#[test]
fn example_and_docs_point_at_the_helper() {
    let example = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/config/example.rs"),
    )
    .unwrap();
    assert!(example.contains("scripts/xai-oauth print"));
    assert!(example.contains("kind = \"openai\""));
    let configuration = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/configuration.md"),
    )
    .unwrap();
    assert!(configuration.contains("scripts/xai-oauth print"));
}
