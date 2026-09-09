use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::exec::EXPIRY_MARGIN;
use super::*;
use crate::config::{CommandOutput, CredentialConfig, CredentialHeader};

fn bearer(secret: &str) -> Credential {
    Credential::new(CredentialHeader::bearer(), secret).unwrap()
}

#[test]
fn header_pair_for_each_scheme() {
    let (name, value) = bearer("abc").header_pair();
    assert_eq!(name, http::header::AUTHORIZATION);
    assert_eq!(value, "Bearer abc");
    assert!(value.is_sensitive());

    let (name, value) = Credential::new(CredentialHeader::x_api_key(), "k")
        .unwrap()
        .header_pair();
    assert_eq!(name.as_str(), "x-api-key");
    assert_eq!(value, "k");

    let custom = CredentialHeader::custom("Api-Key", None).unwrap();
    let (name, value) = Credential::new(custom, "k").unwrap().header_pair();
    assert_eq!(name.as_str(), "api-key");
    assert_eq!(value, "k");

    let custom = CredentialHeader::custom("authorization", Some("Token".into())).unwrap();
    let (name, value) = Credential::new(custom, "k").unwrap().header_pair();
    assert_eq!(name, http::header::AUTHORIZATION);
    assert_eq!(value, "Token k");
    assert!(value.is_sensitive());
}

#[test]
fn custom_header_rejects_bad_names_and_schemes() {
    assert!(CredentialHeader::custom("bad header", None).is_err());
    for scheme in ["", "two words", "line\nbreak"] {
        assert!(
            CredentialHeader::custom("x", Some(scheme.into())).is_err(),
            "{scheme:?}"
        );
    }
}

#[test]
fn debug_and_mask_never_reveal_the_secret() {
    let c = bearer("sk-ant-oat01-verysecretvalue-9999");
    let dbg = format!("{c:?}");
    assert!(!dbg.contains("verysecret"), "{dbg}");
    assert_eq!(c.masked(), "sk-a…9999");
    assert_eq!(mask("short"), "*****");
}

#[tokio::test]
async fn fixed_none_and_static() {
    let none = build(&CredentialConfig::None).unwrap();
    assert_eq!(none.credential().await.unwrap(), None);
    assert_eq!(none.describe(), "none");

    let fixed = build(&CredentialConfig::Static {
        value: "key-1234567890".into(),
        header: CredentialHeader::x_api_key(),
    })
    .unwrap();
    assert_eq!(
        fixed.credential().await.unwrap(),
        Some(Credential::new(CredentialHeader::x_api_key(), "key-1234567890").unwrap())
    );
    assert_eq!(
        fixed.describe(),
        "static",
        "describe never carries the value"
    );
}

#[tokio::test]
async fn env_is_resolved_at_build_time() {
    let name = "ANTHROXY_TEST_CRED";
    // SAFETY: test-local variable, no other thread reads it concurrently.
    unsafe { std::env::set_var(name, "from-environment") };
    let source = build(&CredentialConfig::Env {
        name: name.into(),
        header: CredentialHeader::bearer(),
    })
    .unwrap();
    assert_eq!(
        source.credential().await.unwrap(),
        Some(bearer("from-environment"))
    );
    assert_eq!(source.describe(), "env $ANTHROXY_TEST_CRED");

    let err = build(&CredentialConfig::Env {
        name: "ANTHROXY_TEST_MISSING".into(),
        header: CredentialHeader::bearer(),
    })
    .err()
    .unwrap();
    assert!(matches!(err, CredentialError::MissingEnv(_)), "{err}");
}

#[test]
fn static_value_must_be_header_safe() {
    let err = build(&CredentialConfig::Static {
        value: "line\nbreak".into(),
        header: CredentialHeader::bearer(),
    })
    .err()
    .unwrap();
    assert!(matches!(err, CredentialError::NotHeaderSafe));
}

fn command(cmd: &str, refresh: Duration, timeout: Duration) -> CommandCredential {
    CommandCredential::new(
        cmd.to_owned(),
        CommandOutput::Text,
        CredentialHeader::bearer(),
        refresh,
        timeout,
    )
}

fn json_command(cmd: &str, refresh: Duration) -> CommandCredential {
    CommandCredential::new(
        cmd.to_owned(),
        CommandOutput::Json,
        CredentialHeader::bearer(),
        refresh,
        Duration::from_secs(5),
    )
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap().as_secs()
}

/// A command that appends a line to `counter` and prints `json`.
fn counting_json(counter: &std::path::Path, json: &str) -> String {
    format!("echo run >> {} && echo '{json}'", counter.display())
}

fn runs(counter: &std::path::Path) -> usize {
    std::fs::read_to_string(counter)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

#[tokio::test]
async fn command_output_is_trimmed_and_cached() {
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("runs");
    let cmd = format!(
        "echo run >> {} && printf '  tok-abcdefgh  \\n'",
        counter.display()
    );
    let source = command(&cmd, Duration::from_secs(60), Duration::from_secs(5));

    assert_eq!(
        source.credential().await.unwrap(),
        Some(bearer("tok-abcdefgh"))
    );
    assert_eq!(
        source.credential().await.unwrap(),
        Some(bearer("tok-abcdefgh"))
    );
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().lines().count(),
        1,
        "second call served from cache"
    );

    source.invalidate().await;
    source.credential().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().lines().count(),
        2,
        "invalidate forces a re-run"
    );
}

#[tokio::test]
async fn command_reruns_after_refresh_interval() {
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("runs");
    let cmd = format!("echo run >> {} && echo tok", counter.display());
    let source = command(&cmd, Duration::ZERO, Duration::from_secs(5));
    source.credential().await.unwrap();
    source.credential().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(&counter).unwrap().lines().count(),
        2
    );
}

#[tokio::test]
async fn command_failure_reports_status_and_stderr() {
    let source = command(
        "echo boom >&2; exit 3",
        Duration::ZERO,
        Duration::from_secs(5),
    );
    let err = source.credential().await.err().unwrap();
    let text = err.to_string();
    assert!(matches!(err, CredentialError::Failed { .. }), "{text}");
    assert!(text.contains('3') && text.contains("boom"), "{text}");
}

#[tokio::test]
async fn command_empty_output_is_an_error() {
    let source = command("true", Duration::ZERO, Duration::from_secs(5));
    assert!(matches!(
        source.credential().await,
        Err(CredentialError::Empty)
    ));
}

#[tokio::test]
async fn command_timeout_is_enforced() {
    let source = command(
        "sleep 5; echo late",
        Duration::ZERO,
        Duration::from_millis(100),
    );
    let started = std::time::Instant::now();
    let err = source.credential().await.err().unwrap();
    assert!(matches!(err, CredentialError::Timeout(_)), "{err}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn command_failure_is_not_cached() {
    let dir = tempfile::tempdir().unwrap();
    let flag = dir.path().join("ok");
    let cmd = format!("test -f {f} && echo tok || exit 1", f = flag.display());
    let source = command(&cmd, Duration::from_secs(60), Duration::from_secs(5));
    assert!(source.credential().await.is_err());
    std::fs::write(&flag, "").unwrap();
    assert_eq!(source.credential().await.unwrap(), Some(bearer("tok")));
}

#[tokio::test]
async fn json_output_yields_the_token() {
    let source = json_command(r#"echo '{"token": "tok-json"}'"#, Duration::from_secs(60));
    assert_eq!(source.credential().await.unwrap(), Some(bearer("tok-json")));
    assert!(source.describe().contains("json"), "{}", source.describe());
}

#[tokio::test]
async fn json_output_with_a_distant_expiry_is_cached() {
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("runs");
    let expires_at =
        humantime::format_rfc3339_seconds(SystemTime::now() + Duration::from_secs(3600));
    let cmd = counting_json(
        &counter,
        &format!(r#"{{"token": "tok", "expires_at": "{expires_at}"}}"#),
    );
    let source = json_command(&cmd, Duration::from_secs(3600));
    assert_eq!(source.credential().await.unwrap(), Some(bearer("tok")));
    assert_eq!(source.credential().await.unwrap(), Some(bearer("tok")));
    assert_eq!(runs(&counter), 1, "second call served from cache");
}

#[tokio::test]
async fn json_output_reruns_once_the_expiry_nears() {
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("runs");
    let expires_at = unix_secs(SystemTime::now() + EXPIRY_MARGIN - Duration::from_secs(30));
    let cmd = counting_json(
        &counter,
        &format!(r#"{{"token": "tok", "expires_at": {expires_at}}}"#),
    );
    let source = json_command(&cmd, Duration::from_secs(3600));
    assert_eq!(source.credential().await.unwrap(), Some(bearer("tok")));
    assert_eq!(source.credential().await.unwrap(), Some(bearer("tok")));
    assert_eq!(
        runs(&counter),
        2,
        "a credential inside the expiry margin is not served from cache"
    );
}

#[tokio::test]
async fn json_output_that_already_expired_is_an_error() {
    let expired_ms = unix_secs(SystemTime::now() - Duration::from_secs(60)) * 1000;
    let source = json_command(
        &format!(r#"echo '{{"token": "tok", "expires_at": {expired_ms}}}'"#),
        Duration::from_secs(60),
    );
    let err = source.credential().await.err().unwrap();
    assert!(matches!(err, CredentialError::Expired(_)), "{err}");
    assert!(err.to_string().contains("expired at 20"), "{err}");
}

#[tokio::test]
async fn json_output_must_be_a_token_object() {
    for cmd in [
        "echo not-json",
        "echo '{}'",
        r#"echo '{"token": "t", "expires_at": "soon"}'"#,
        r#"echo '{"token": "t", "expires_at": true}'"#,
    ] {
        let err = json_command(cmd, Duration::ZERO)
            .credential()
            .await
            .err()
            .unwrap();
        assert!(matches!(err, CredentialError::Json(_)), "{cmd}: {err}");
    }
    let err = json_command(r#"echo '{"token": ""}'"#, Duration::ZERO)
        .credential()
        .await
        .err()
        .unwrap();
    assert!(matches!(err, CredentialError::Empty), "{err}");
}

#[tokio::test]
async fn exec_run_captures_both_streams_and_the_exit_status() {
    let run = exec::run("printf out; printf err >&2; exit 3", Duration::from_secs(5))
        .await
        .unwrap();
    assert!(!run.success);
    assert_eq!(run.status, "exit 3");
    assert_eq!(run.stdout, "out");
    assert_eq!(run.stderr, "err");

    let err = exec::interpret(&run, CommandOutput::Text).err().unwrap();
    assert_eq!(
        err.to_string(),
        "credential command failed with exit 3: err"
    );
    assert!(
        exec::interpret(
            &exec::run("echo tok", Duration::from_secs(5)).await.unwrap(),
            CommandOutput::Text
        )
        .is_ok()
    );
}

#[test]
fn valid_for_is_the_refresh_interval_cut_short_by_the_expiry() {
    let refresh = Duration::from_secs(300);
    assert_eq!(exec::valid_for(refresh, None).unwrap(), refresh);
    assert_eq!(
        exec::valid_for(refresh, Some(SystemTime::now() + Duration::from_secs(3600))).unwrap(),
        refresh,
        "a distant expiry leaves the refresh interval alone"
    );

    let near = SystemTime::now() + EXPIRY_MARGIN + Duration::from_secs(30);
    let valid = exec::valid_for(refresh, Some(near)).unwrap();
    assert!(
        valid <= Duration::from_secs(30) && valid > Duration::from_secs(25),
        "{valid:?}"
    );

    let past = SystemTime::now() - Duration::from_secs(1);
    assert!(matches!(
        exec::valid_for(refresh, Some(past)),
        Err(CredentialError::Expired(_))
    ));
}
