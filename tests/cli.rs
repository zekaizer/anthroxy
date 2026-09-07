//! Black-box tests of the `anthroxy` binary.

mod support;

use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use predicates::prelude::*;
use support::MockUpstream;
use support::mock_upstream::echo;
use support::router::{TOKEN, config_with_backend};

/// For tests that keep the process running and read its output live.
fn spawnable() -> std::process::Command {
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin!("anthroxy"));
    cmd.env_remove("ANTHROXY_CONFIG")
        .env_remove("RUST_LOG")
        .env_remove("SSL_CERT_FILE")
        .env_remove("SSL_CERT_DIR")
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
    bin()
        .args(["--config", "/nonexistent/anthroxy.toml", "models"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read"))
        .stderr(predicate::str::contains("/nonexistent/anthroxy.toml"));
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
            "export ANTHROPIC_BASE_URL=\"http://router.example:0\"",
        ))
        .stdout(predicate::str::contains(format!(
            "export ANTHROPIC_AUTH_TOKEN=\"{TOKEN}\""
        )))
        .stdout(predicate::str::contains(
            "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
        ))
        .stdout(predicate::str::contains("export ANTHROPIC_MODEL=\"fast\""));
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
