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

/// Whatever the command wrote — a token server's answer, a shell trace —
/// reaches a log line and the 502 the client is given.
#[test]
fn command_stderr_cannot_forge_a_log_line_or_fill_the_message() {
    let error = CredentialError::Failed {
        status: "exit 3".to_owned(),
        stderr: format!(
            "boom\n2026-09-09T00:00:00Z  INFO forged\n{}",
            "z".repeat(3_000)
        ),
    };
    let text = error.to_string();
    assert!(!text.contains('\n'), "{text}");
    assert!(text.len() < 300, "{} chars", text.len());
    assert!(text.contains("boom\\n"), "{text}");
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
async fn json_expiry_accepts_the_epoch_shapes_a_token_store_writes() {
    // Claude Code's own credential file carries fractional milliseconds.
    let expires_at = SystemTime::now() + Duration::from_secs(3600);
    let millis = expires_at.duration_since(UNIX_EPOCH).unwrap().as_millis();
    let source = json_command(
        &format!(r#"echo '{{"token": "tok-abcdefgh", "expires_at": {millis}.98}}'"#),
        Duration::from_secs(3600),
    );
    assert_eq!(
        source.credential().await.unwrap(),
        Some(bearer("tok-abcdefgh"))
    );
}

#[tokio::test]
async fn json_expiry_out_of_range_is_an_error_not_a_panic() {
    for expires_at in ["18446744073709551615", "-1", "1e400"] {
        let source = json_command(
            &format!(r#"echo '{{"token": "tok", "expires_at": {expires_at}}}'"#),
            Duration::ZERO,
        );
        let err = source.credential().await.err().unwrap();
        assert!(
            matches!(err, CredentialError::Json(_)),
            "{expires_at}: {err}"
        );
    }
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

#[tokio::test]
async fn fixed_status_is_the_masked_value_without_history() {
    let fixed = build(&CredentialConfig::Static {
        value: "key-1234567890".into(),
        header: CredentialHeader::x_api_key(),
    })
    .unwrap();
    let status = fixed.status();
    assert_eq!(status.source, "static");
    assert_eq!(status.masked.as_deref(), Some("key-…7890"));
    assert!(status.refreshes.is_empty());
    assert_eq!(status.refresh_at, None);

    let none = build(&CredentialConfig::None).unwrap().status();
    assert_eq!(none.source, "none");
    assert_eq!(none.masked, None);
}

#[tokio::test]
async fn command_status_reports_the_cached_value_and_each_run() {
    let source = CommandCredential::new(
        r#"printf '{"token": "tok-1234567890", "expires_at": 4102444800}'"#.into(),
        CommandOutput::Json,
        CredentialHeader::bearer(),
        Duration::from_secs(300),
        Duration::from_secs(10),
    );
    let before = source.status();
    assert!(before.source.starts_with("command"), "{}", before.source);
    assert_eq!(before.masked, None);
    assert!(before.refreshes.is_empty());

    source.credential().await.unwrap();
    source.credential().await.unwrap();
    let status = source.status();
    assert_eq!(status.masked.as_deref(), Some("tok-…7890"));
    assert_eq!(
        status.expires_at,
        Some("2100-01-01T00:00:00Z".parse().unwrap())
    );
    let now = jiff::Timestamp::now();
    let refresh_at = status
        .refresh_at
        .expect("a cached value has a refresh time");
    assert!(refresh_at > now, "{refresh_at}");
    assert!(
        refresh_at <= now + jiff::SignedDuration::from_secs(300),
        "{refresh_at}"
    );
    assert!(status.fetched_at.is_some_and(|t| t <= now));
    assert_eq!(status.refreshes.len(), 1, "a cache hit is not a run");
    assert_eq!(status.refreshes[0].masked.as_deref(), Some("tok-…7890"));
    assert_eq!(status.refreshes[0].error, None);

    source.invalidate().await;
    let status = source.status();
    assert_eq!(status.masked, None, "nothing is cached after invalidate");
    assert_eq!(status.refresh_at, None);
    assert_eq!(status.refreshes.len(), 1);
}

#[tokio::test]
async fn a_failed_run_is_in_the_history_with_its_error() {
    let source = CommandCredential::new(
        "echo nope >&2; exit 3".into(),
        CommandOutput::Text,
        CredentialHeader::bearer(),
        Duration::from_secs(300),
        Duration::from_secs(10),
    );
    assert!(source.credential().await.is_err());
    let status = source.status();
    assert_eq!(status.masked, None);
    assert_eq!(status.refreshes.len(), 1);
    let error = status.refreshes[0].error.as_deref().unwrap();
    assert!(
        error.contains("exit 3") && error.contains("nope"),
        "{error}"
    );
    assert_eq!(status.refreshes[0].masked, None);
}

#[tokio::test]
async fn the_run_history_is_bounded() {
    let source = CommandCredential::new(
        "exit 1".into(),
        CommandOutput::Text,
        CredentialHeader::bearer(),
        Duration::from_secs(300),
        Duration::from_secs(10),
    );
    for _ in 0..30 {
        let _ = source.credential().await;
    }
    assert_eq!(
        source.status().refreshes.len(),
        super::command::REFRESH_HISTORY
    );
}
