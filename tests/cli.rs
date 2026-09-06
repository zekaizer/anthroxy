//! Black-box tests of the `claude-router` binary.

use std::io::{BufRead, BufReader, Write};
use std::process::Stdio;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use predicates::prelude::*;

fn bin() -> Command {
    let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("claude-router"));
    cmd.env_remove("CLAUDE_ROUTER_CONFIG")
        .env_remove("RUST_LOG")
        .env("NO_COLOR", "1");
    cmd
}

/// For tests that keep the process running and read its output live.
fn spawnable() -> std::process::Command {
    let mut cmd = std::process::Command::new(assert_cmd::cargo::cargo_bin!("claude-router"));
    cmd.env_remove("CLAUDE_ROUTER_CONFIG")
        .env_remove("RUST_LOG")
        .env("NO_COLOR", "1");
    cmd
}

fn write_config(dir: &std::path::Path, extra: &str) -> std::path::PathBuf {
    let path = dir.join("config.toml");
    let mut file = std::fs::File::create(&path).unwrap();
    write!(
        file,
        r#"
[server]
listen = "127.0.0.1:0"
token = "cli-test-token"

[backends.local]
url = "http://127.0.0.1:1"

[[models]]
id = "m-one"
backend = "local"
upstream_model = "upstream-one"
display_name = "Model One"
aliases = ["alias-one"]

[[models]]
id = "m-two"
backend = "local"
{extra}
"#
    )
    .unwrap();
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
    let re = regex_lite::Regex::new(
        r"^claude-router \d+\.\d+\.\d+ \(([0-9a-f]{7,}(-dirty)?|unknown)\)\n$",
    )
    .unwrap();
    assert!(re.is_match(&text), "unexpected version line: {text:?}");

    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Luke Lee"))
        .stdout(predicate::str::is_match(r"claude-router \d+\.\d+\.\d+ \(").unwrap());
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
        .stdout(predicate::str::contains("claude-router check"));
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
        .stdout(predicate::str::starts_with("# claude-router configuration"))
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
fn missing_config_is_a_clear_error() {
    bin()
        .args(["--config", "/nonexistent/claude-router.toml", "models"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read"))
        .stderr(predicate::str::contains("/nonexistent/claude-router.toml"));
}

#[test]
fn models_prints_the_table() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_config(dir.path(), "[routing]\ndefault_model = \"m-two\"\n");
    bin()
        .args(["--config", path.to_str().unwrap(), "models"])
        .assert()
        .success()
        .stdout(predicate::str::contains("m-one"))
        .stdout(predicate::str::contains("upstream-one"))
        .stdout(predicate::str::contains("Model One"))
        .stdout(predicate::str::contains("alias-one"))
        .stdout(predicate::str::contains("routed to m-two"));
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
        .stdout(predicate::str::contains(
            "export ANTHROPIC_AUTH_TOKEN=\"cli-test-token\"",
        ))
        .stdout(predicate::str::contains(
            "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",
        ))
        .stdout(predicate::str::contains("export ANTHROPIC_MODEL=\"m-one\""));
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
    let mut child = spawnable()
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

    let stdout = BufReader::new(child.stdout.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stdout.lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut url = None;
    let mut banner = Vec::new();
    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            if let Some(rest) = line.trim().strip_prefix("listening on ") {
                url = Some(rest.split_whitespace().next().unwrap().to_owned());
            }
            banner.push(line);
            if url.is_some() && banner.iter().any(|l| l.contains("Ctrl-C")) {
                break;
            }
        }
    }
    let url = url.unwrap_or_else(|| panic!("no listening line in {banner:?}"));
    assert!(banner.iter().any(|l| l.contains("m-one")), "{banner:?}");
    assert!(banner.iter().any(|l| l.contains("body log")), "{banner:?}");

    let body = ureq_get(&format!("{url}/healthz"));
    assert!(body.contains("\"status\":\"ok\""), "{body}");

    let unauthenticated = ureq_get(&format!("{url}/v1/models"));
    assert!(
        unauthenticated.contains("authentication_error"),
        "{unauthenticated}"
    );

    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // SAFETY: plain libc call on a pid this test owns.
        unsafe {
            libc_kill(pid);
        }
        let status = child.wait().unwrap();
        assert!(status.success(), "graceful shutdown exits 0, got {status}");
    }
    #[cfg(not(unix))]
    child.kill().unwrap();
}

#[cfg(unix)]
unsafe fn libc_kill(pid: i32) {
    unsafe { libc_signal(pid, 15) }
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
    let mut child = spawnable()
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
    let url = wait_for_listening(&mut child);

    let before = ureq_get_auth(&format!("{url}/v1/models"), "cli-test-token");
    assert!(
        before.contains("\"m-one\"") && !before.contains("\"m-three\""),
        "{before}"
    );

    let mut text = std::fs::read_to_string(&path).unwrap();
    text.push_str("\n[[models]]\nid = \"m-three\"\nbackend = \"local\"\n");
    std::fs::write(&path, text).unwrap();
    // SAFETY: plain libc call on a pid this test owns.
    unsafe { libc_signal(child.id() as i32, 1) };

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut after = String::new();
    while Instant::now() < deadline {
        after = ureq_get_auth(&format!("{url}/v1/models"), "cli-test-token");
        if after.contains("\"m-three\"") {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        after.contains("\"m-three\""),
        "model table not reloaded: {after}"
    );
    child.kill().unwrap();
}

/// Reads the banner until the listening line appears; returns the base URL.
fn wait_for_listening(child: &mut std::process::Child) -> String {
    let stdout = BufReader::new(child.stdout.take().unwrap());
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in stdout.lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200))
            && let Some(rest) = line.trim().strip_prefix("listening on ")
        {
            return rest.split_whitespace().next().unwrap().to_owned();
        }
    }
    panic!("serve never printed a listening line");
}

fn ureq_get_auth(url: &str, token: &str) -> String {
    use std::io::Read;
    let without_scheme = url.strip_prefix("http://").unwrap();
    let (host, path) = without_scheme
        .split_once('/')
        .map(|(h, p)| (h, format!("/{p}")))
        .unwrap();
    let mut stream = std::net::TcpStream::connect(host).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nx-api-key: {token}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

/// Minimal blocking GET; the test must not depend on tokio.
fn ureq_get(url: &str) -> String {
    use std::io::Read;
    let without_scheme = url.strip_prefix("http://").unwrap();
    let (host, path) = without_scheme
        .split_once('/')
        .map(|(h, p)| (h, format!("/{p}")))
        .unwrap();
    let mut stream = std::net::TcpStream::connect(host).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}
