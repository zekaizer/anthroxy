//! Black-box tests of the `anthroxy` binary.

mod support;

use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use predicates::prelude::*;
use support::MockUpstream;
use support::mock_upstream::echo;
use support::router::{TOKEN, config_with_backend, messages_body};

/// For tests that keep the process running and read its output live.
fn spawnable() -> std::process::Command {
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin!("anthroxy"));
    cmd.env_remove("ANTHROXY_CONFIG")
        .env_remove("RUST_LOG")
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
        // Statistics default to the state directory; keep them out of $HOME.
        .env(
            "XDG_STATE_HOME",
            std::env::temp_dir().join("anthroxy-cli-tests-state"),
        )
        .env("NO_COLOR", "1");
    cmd
}

fn bin() -> Command {
    Command::from_std(spawnable())
}

/// The shared test configuration, pointing at a port nothing listens on.
fn write_config(dir: &std::path::Path, extra: &str) -> std::path::PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(&path, config_with_backend("http://127.0.0.1:1", extra)).unwrap();
    path
}

#[test]
fn help_lists_every_command() {
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("serve"))
        .stdout(predicate::str::contains("check"))
        .stdout(predicate::str::contains("models"))
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("env"))
        .stdout(predicate::str::contains("service"))
        .stdout(predicate::str::contains("Getting started"));
}

#[test]
fn version_shows_git_state_and_help_shows_author() {
    let out = bin().arg("--version").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let re =
        regex_lite::Regex::new(r"^anthroxy \d+\.\d+\.\d+ \(([0-9a-f]{7,}(-dirty)?|unknown)\)\n$")
            .unwrap();
    assert!(re.is_match(&text), "unexpected version line: {text:?}");

    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Luke Lee"))
        .stdout(predicate::str::is_match(r"anthroxy \d+\.\d+\.\d+ \(").unwrap());
}

#[test]
fn init_writes_a_loadable_config_and_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("config.toml");
    bin()
        .args(["--config", path.to_str().unwrap(), "init"])
        .assert()
        .success()
        .stdout(predicate::str::contains("wrote"))
        .stdout(predicate::str::contains("anthroxy check"));
    assert!(path.exists());

    bin()
        .args(["--config", path.to_str().unwrap(), "check", "--no-probe"])
        .assert()
        .success()
        .stdout(predicate::str::contains("valid"))
        .stdout(predicate::str::contains("local-default"));

    bin()
        .args(["--config", path.to_str().unwrap(), "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"))
        .stderr(predicate::str::contains("--force"));

    bin()
        .args(["--config", path.to_str().unwrap(), "init", "--force"])
        .assert()
        .success();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the file holds the client token");
    }
}

#[test]
fn init_stdout_prints_without_touching_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    bin()
        .args(["--config", path.to_str().unwrap(), "init", "--stdout"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("# anthroxy configuration"))
        .stdout(predicate::str::contains("[[models]]"));
    assert!(!path.exists());
}

#[test]
fn check_reports_every_problem_and_fails() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[server]\ntoken = \"\"\n[[models]]\nid = \"m\"\nbackend = \"ghost\"\n",
    )
    .unwrap();
    bin()
        .args(["--config", path.to_str().unwrap(), "check", "--no-probe"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("server.token"))
        .stdout(predicate::str::contains("models[0].backend"))
        .stdout(predicate::str::contains("at least one [backends"));
}

/// The file carries the router token and any `static` backend credential.
#[cfg(unix)]
#[test]
fn check_reports_a_configuration_other_users_can_read() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "");
    let chmod = |mode| {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    };

    chmod(0o644);
    bin()
        .args(["--config", path.to_str().unwrap(), "check", "--no-probe"])
        .assert()
        .success()
        .stdout(predicate::str::contains("644"))
        .stdout(predicate::str::contains("chmod 600"));

    chmod(0o600);
    bin()
        .args(["--config", path.to_str().unwrap(), "check", "--no-probe"])
        .assert()
        .success()
        .stdout(predicate::str::contains("chmod 600").not());
}

#[test]
fn check_probe_reports_unreachable_backend() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "");
    bin()
        .args([
            "--config",
            path.to_str().unwrap(),
            "check",
            "--timeout",
            "2s",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("unreachable"))
        .stdout(predicate::str::contains("1 backend problem"));
}

#[test]
fn check_reports_an_unreadable_ca_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(
        dir.path(),
        "\n[upstream]\nca_certificate = \"/nonexistent/corp-ca.pem\"\n",
    );
    bin()
        .args(["--config", path.to_str().unwrap(), "check", "--no-probe"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("/nonexistent/corp-ca.pem"));
}

#[test]
fn check_trusts_the_ca_named_by_ssl_cert_file() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let (upstream, ca_pem) = runtime.block_on(MockUpstream::start_tls(echo));
    let dir = tempfile::tempdir().unwrap();
    let ca = dir.path().join("corp-ca.pem");
    std::fs::write(&ca, ca_pem).unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, config_with_backend(&upstream.url(), "")).unwrap();
    let args = [
        "--config",
        path.to_str().unwrap(),
        "check",
        "--timeout",
        "5s",
    ];

    bin()
        .args(args)
        .assert()
        .failure()
        .stdout(predicate::str::contains("UnknownIssuer"));

    bin()
        .args(args)
        .env("SSL_CERT_FILE", &ca)
        .assert()
        .success()
        .stdout(predicate::str::contains("ready"));
}

#[test]
fn missing_config_is_a_clear_error() {
    let out = bin()
        .args(["--config", "/nonexistent/anthroxy.toml", "models"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot read"), "{stderr}");
    assert!(stderr.contains("anthroxy init"), "{stderr}");
    assert_eq!(
        stderr.matches("/nonexistent/anthroxy.toml").count(),
        1,
        "the file is named twice: {stderr}"
    );
}

/// Backends covering each credential kind, one of them broken.
fn credential_config(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        r#"
[server]
token = "t"
listen = "127.0.0.1:0"

[backends.good]
url = "http://127.0.0.1:1"
credential = { kind = "command", command = "printf '  tok-abcdefgh \n'", refresh = "90s" }

[backends.bad]
url = "http://127.0.0.1:1"
credential = { kind = "command", command = "echo boom >&2; exit 3" }

[backends.from-env]
url = "http://127.0.0.1:1"
credential = { kind = "env", name = "ANTHROXY_TEST_ABSENT" }

[[models]]
id = "m"
backend = "good"
"#,
    )
    .unwrap();
    path
}

#[test]
fn credential_reports_every_command_and_fails_on_a_broken_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = credential_config(dir.path());
    bin()
        .args(["--config", path.to_str().unwrap(), "credential"])
        .env_remove("ANTHROXY_TEST_ABSENT")
        .assert()
        .failure()
        .stdout(predicate::str::contains("credential  tok-…efgh (12 chars)"))
        .stdout(predicate::str::contains("re-run in   1m 30s"))
        .stdout(predicate::str::contains("✗ exit 3 in"))
        .stdout(predicate::str::contains("stderr      boom"))
        .stdout(predicate::str::contains(
            "from-env  env $ANTHROXY_TEST_ABSENT",
        ))
        .stdout(predicate::str::contains("✗ not set"))
        .stdout(predicate::str::contains("2 credential problem(s)"))
        .stdout(predicate::str::contains("--reveal"));
}

#[test]
fn credential_takes_backend_names_and_reveals_on_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = credential_config(dir.path());
    let path = path.to_str().unwrap();
    bin()
        .args(["--config", path, "credential", "good"])
        .assert()
        .success()
        .stdout(predicate::str::contains("tok-…efgh"))
        .stdout(predicate::str::contains("every credential acquired"))
        .stdout(predicate::str::contains("bad").not());

    bin()
        .args(["--config", path, "credential", "good", "--reveal"])
        .assert()
        .success()
        .stdout(predicate::str::contains("credential  tok-abcdefgh"));

    bin()
        .args(["--config", path, "credential", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no backend named `nope`"))
        .stderr(predicate::str::contains("good"));
}

#[test]
fn credential_as_service_needs_the_systemd_user_manager() {
    let dir = tempfile::tempdir().unwrap();
    let path = credential_config(dir.path());
    // Fails everywhere: without a user manager it cannot start, and where it
    // can, `bad` still exits 3.
    let out = bin()
        .args([
            "--config",
            path.to_str().unwrap(),
            "credential",
            "--as-service",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(text.contains("systemd"), "{text}");
}

#[test]
fn models_prints_the_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "[routing]\ndefault_model = \"smart\"\n");
    bin()
        .args(["--config", path.to_str().unwrap(), "models"])
        .assert()
        .success()
        .stdout(predicate::str::contains("fast"))
        .stdout(predicate::str::contains("mock-fast-v1"))
        .stdout(predicate::str::contains("Fast Mock"))
        .stdout(predicate::str::contains("claude-haiku-4-5"))
        .stdout(predicate::str::contains("unknown model ids → smart"));
}

#[test]
fn env_prints_shell_exports_and_json() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "");
    let path = path.to_str().unwrap();
    bin()
        .args(["--config", path, "env", "--host", "router.example"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "export ANTHROPIC_BASE_URL='http://router.example:0'",
        ))
        .stdout(predicate::str::contains(format!(
            "export ANTHROPIC_AUTH_TOKEN='{TOKEN}'"
        )))
        .stdout(predicate::str::contains(
            "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
        ))
        .stdout(predicate::str::contains("export ANTHROPIC_MODEL='fast'"));
    bin()
        .args(["--config", path, "env", "--format", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "\"ANTHROPIC_BASE_URL\": \"http://127.0.0.1:0\"",
        ));
}

#[test]
fn service_install_print_renders_a_unit_anywhere() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "");
    bin()
        .args([
            "--config",
            path.to_str().unwrap(),
            "service",
            "install",
            "--print",
        ])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("[Unit]"))
        .stdout(predicate::str::contains("serve"))
        .stdout(predicate::str::contains(path.to_str().unwrap()));
}

#[test]
fn a_failure_reports_its_cause_once() {
    // Hold the port so `serve` cannot bind it.
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = taken.local_addr().unwrap().to_string();
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "");
    let out = bin()
        .args([
            "--config",
            path.to_str().unwrap(),
            "serve",
            "--listen",
            &addr,
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("cannot listen on"), "{stderr}");
    assert_eq!(
        stderr.matches("in use").count(),
        1,
        "the cause is repeated: {stderr}"
    );
}

/// `RUST_LOG=` exported empty is not a filter; the file still decides.
#[test]
fn an_empty_rust_log_does_not_silence_the_router() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "");
    let mut child = spawnable()
        .env("RUST_LOG", "")
        .args([
            "--config",
            path.to_str().unwrap(),
            "serve",
            "--listen",
            "127.0.0.1:0",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = BufReader::new(child.stderr.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stderr.lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut logged = false;
    while !logged && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => logged = line.contains("anthroxy listening"),
            Err(_) => continue,
        }
    }
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(logged, "the router logged nothing at the file's `info`");
}

/// `http_proxy` in the environment would send a backend the configuration
/// named by URL — and the credential for it — to a host it never named.
#[test]
fn an_ambient_proxy_does_not_redirect_a_backend_request() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let upstream = runtime.block_on(MockUpstream::start(echo));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, config_with_backend(&upstream.url(), "")).unwrap();

    let mut child = spawnable()
        // Nothing listens there, so a proxied request cannot be answered.
        .env("HTTP_PROXY", "http://127.0.0.1:1")
        .env("ALL_PROXY", "http://127.0.0.1:1")
        .args([
            "--config",
            path.to_str().unwrap(),
            "serve",
            "--listen",
            "127.0.0.1:0",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let url = wait_for_banner(&mut child).url;
    let status = runtime.block_on(async {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(format!("{url}/v1/messages"))
            .header("x-api-key", TOKEN)
            .json(&messages_body("fast"))
            .send()
            .await
            .unwrap()
            .status()
    });
    child.kill().unwrap();
    assert_eq!(status, 200, "the backend is reached directly");
    assert_eq!(upstream.received().len(), 1);
}

#[test]
fn serve_starts_answers_health_and_stops_on_sigterm() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "");
    let mut child = serve(&path);
    let banner = wait_for_banner(&mut child);
    assert!(
        banner.lines.iter().any(|l| l.contains("fast")),
        "{banner:?}"
    );
    assert!(
        banner.lines.iter().any(|l| l.contains("body log")),
        "{banner:?}"
    );
    assert!(
        banner
            .lines
            .iter()
            .any(|l| l.contains("stats") && l.contains("anthroxy-cli-tests-state")),
        "statistics go to the state directory: {banner:?}"
    );

    let body = http_get(&format!("{}/healthz", banner.url), None);
    assert!(body.contains("\"status\":\"ok\""), "{body}");

    let unauthenticated = http_get(&format!("{}/v1/models", banner.url), None);
    assert!(
        unauthenticated.contains("authentication_error"),
        "{unauthenticated}"
    );

    #[cfg(unix)]
    {
        // SAFETY: plain libc call on a pid this test owns.
        unsafe { libc_signal(child.id() as i32, 15) };
        let status = child.wait().unwrap();
        assert!(status.success(), "graceful shutdown exits 0, got {status}");
    }
    #[cfg(not(unix))]
    child.kill().unwrap();
}

#[cfg(unix)]
unsafe fn libc_signal(pid: i32, sig: i32) {
    unsafe extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe {
        kill(pid, sig);
    }
}

#[cfg(unix)]
#[test]
fn sighup_reloads_the_configuration_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "");
    let mut child = serve(&path);
    let url = wait_for_banner(&mut child).url;

    let before = http_get(&format!("{url}/v1/models"), Some(TOKEN));
    assert!(
        before.contains("\"fast\"") && !before.contains("\"newcomer\""),
        "{before}"
    );

    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\n[[models]]\nid = \"newcomer\"\nbackend = \"mock\"\n");
    std::fs::write(&path, text).unwrap();
    // SAFETY: plain libc call on a pid this test owns.
    unsafe { libc_signal(child.id() as i32, 1) };

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut after = String::new();
    while Instant::now() < deadline {
        after = http_get(&format!("{url}/v1/models"), Some(TOKEN));
        if after.contains("\"newcomer\"") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        after.contains("\"newcomer\""),
        "model table not reloaded: {after}"
    );
    child.kill().unwrap();
}

#[cfg(unix)]
#[test]
fn a_logging_change_on_reload_says_it_needs_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "[logging]\nlevel = \"info\"\n");
    let mut child = serve(&path);
    let _ = wait_for_banner(&mut child);
    let stderr = BufReader::new(child.stderr.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stderr.lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });

    std::fs::write(
        &path,
        config_with_backend("http://127.0.0.1:1", "[logging]\nlevel = \"debug\"\n"),
    )
    .unwrap();
    // SAFETY: plain libc call on a pid this test owns.
    unsafe { libc_signal(child.id() as i32, 1) };

    // A margin for a loaded machine, not a timing assertion: the loop ends as
    // soon as the line arrives.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut lines = Vec::new();
    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            let done = line.contains("restart");
            lines.push(line);
            if done {
                break;
            }
        }
    }
    child.kill().unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("logging") && l.contains("restart")),
        "{lines:?}"
    );
}

/// `anthroxy serve` on an ephemeral port with its output captured.
fn serve(config: &std::path::Path) -> std::process::Child {
    spawnable()
        .args([
            "--config",
            config.to_str().unwrap(),
            "serve",
            "--listen",
            "127.0.0.1:0",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[derive(Debug)]
struct Banner {
    /// Base URL from the `listening on` line.
    url: String,
    lines: Vec<String>,
}

/// Reads the start-up banner through its last line.
fn wait_for_banner(child: &mut std::process::Child) -> Banner {
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stdout.lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut url = None;
    let mut lines = Vec::new();
    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            if let Some(rest) = line.trim().strip_prefix("listening on ") {
                url = Some(rest.split_whitespace().next().unwrap().to_owned());
            }
            let last = line.contains("Ctrl-C");
            lines.push(line);
            if last {
                break;
            }
        }
    }
    let url = url.unwrap_or_else(|| panic!("no listening line in {lines:?}"));
    Banner { url, lines }
}

/// Minimal blocking GET; the test must not depend on tokio.
fn http_get(url: &str, token: Option<&str>) -> String {
    use std::io::Read;
    let without_scheme = url.strip_prefix("http://").unwrap();
    let (host, path) = without_scheme
        .split_once('/')
        .map(|(h, p)| (h, format!("/{p}")))
        .unwrap();
    let mut stream = std::net::TcpStream::connect(host).unwrap();
    let auth = token
        .map(|t| format!("x-api-key: {t}\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\n{auth}Connection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

#[tokio::test(flavor = "multi_thread")]
async fn check_probes_an_openai_backend_without_anthropic_headers() {
    use support::mock_upstream::json_response;
    use support::openai::config_with_openai_backend;
    let upstream = support::MockUpstream::start(|_| {
        json_response(
            200,
            serde_json::json!({"object": "list", "data": [{"id": "qwen-32b", "object": "model"}]}),
        )
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, config_with_openai_backend(&upstream.url(), "")).unwrap();
    let path = path.to_str().unwrap().to_owned();
    tokio::task::spawn_blocking(move || {
        bin()
            .args(["--config", &path, "check", "--timeout", "2s"])
            .assert()
            .success()
            .stdout(predicate::str::contains("(openai)"))
            .stdout(predicate::str::contains("1 model(s)"))
            .stdout(predicate::str::contains("qwen-32b"));
    })
    .await
    .unwrap();
    let probe = upstream.last();
    assert_eq!(probe.path_and_query, "/v1/models");
    assert_eq!(probe.header("anthropic-version"), None);
    assert_eq!(
        probe.header("authorization"),
        Some("Bearer backend-secret-key")
    );
}
