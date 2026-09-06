use std::time::Duration;

use super::*;

const MINIMAL: &str = r#"
[server]
token = "secret"

[backends.local]
url = "http://127.0.0.1:1234"

[[models]]
id = "gemma"
backend = "local"
"#;

fn no_env(_: &str) -> Option<String> {
    None
}

fn parse(text: &str) -> Result<Config, ConfigError> {
    Config::parse(text, no_env)
}

fn problems(text: &str) -> Vec<String> {
    match parse(text) {
        Err(ConfigError::Invalid(p)) => p,
        other => panic!("expected validation failure, got {other:?}"),
    }
}

#[test]
fn minimal_config_applies_defaults() {
    let c = parse(MINIMAL).unwrap();
    assert_eq!(c.server.listen, "0.0.0.0:8787".parse().unwrap());
    assert_eq!(c.server.max_body_bytes, 64 * 1024 * 1024);
    assert_eq!(c.logging.level, "info");
    assert_eq!(c.logging.format, LogFormat::Text);
    assert_eq!(c.upstream.retries, 2);
    assert_eq!(c.upstream.connect_timeout, Duration::from_secs(10));
    assert!(matches!(
        c.backends["local"].credential,
        CredentialConfig::None
    ));
    assert_eq!(c.models[0].upstream_model, None);
    assert_eq!(c.routing.default_model, None);
    assert_eq!(c.logging.body_retention, Duration::from_secs(7 * 24 * 3600));
}

#[test]
fn body_retention_accepts_zero() {
    let text = MINIMAL.to_owned() + "\n[logging]\nbody_retention = \"0s\"\n";
    assert_eq!(parse(&text).unwrap().logging.body_retention, Duration::ZERO);
}

#[test]
fn full_config_round_trips_every_field() {
    let text = r#"
[server]
listen = "127.0.0.1:9000"
token = "t"
max_body_bytes = "8MiB"

[logging]
level = "debug"
format = "json"
body_dir = "/tmp/bodies"

[upstream]
connect_timeout = "3s"
read_timeout = "2m"
retries = 1
retry_backoff = "50ms"
retry_on_status = [502, 503]

[backends.claude]
url = "https://api.anthropic.com/"
credential = { kind = "command", command = "echo tok", output = "json", refresh = "1m", timeout = "2s" }
anthropic_beta = ["oauth-2025-04-20"]
headers = { "x-extra" = "1" }

[backends.vllm]
url = "http://10.0.0.2:8000"
credential = { kind = "static", value = "k", header = "x_api_key" }

[[models]]
id = "opus"
backend = "claude"
upstream_model = "claude-opus-5"
display_name = "Opus via OAuth"
aliases = ["claude-opus-5", "big"]

[[models]]
id = "qwen"
backend = "vllm"

[routing]
default_model = "qwen"
"#;
    let c = parse(text).unwrap();
    assert_eq!(c.server.max_body_bytes, 8 * 1024 * 1024);
    assert_eq!(c.logging.format, LogFormat::Json);
    assert_eq!(
        c.logging.body_dir.as_deref(),
        Some(std::path::Path::new("/tmp/bodies"))
    );
    assert_eq!(c.upstream.read_timeout, Duration::from_secs(120));
    assert_eq!(c.upstream.retry_on_status, vec![502, 503]);
    match &c.backends["claude"].credential {
        CredentialConfig::Command {
            command,
            output,
            refresh,
            timeout,
            header,
        } => {
            assert_eq!(command, "echo tok");
            assert_eq!(*output, CommandOutput::Json);
            assert_eq!(*refresh, Duration::from_secs(60));
            assert_eq!(*timeout, Duration::from_secs(2));
            assert_eq!(*header, CredentialHeader::Bearer);
        }
        other => panic!("{other:?}"),
    }
    match &c.backends["vllm"].credential {
        CredentialConfig::Static { header, .. } => assert_eq!(*header, CredentialHeader::XApiKey),
        other => panic!("{other:?}"),
    }
    // Trailing slash is dropped so path concatenation is unambiguous.
    assert_eq!(c.backends["claude"].url, "https://api.anthropic.com");
    assert_eq!(c.models[0].aliases, vec!["claude-opus-5", "big"]);
    assert_eq!(c.routing.default_model.as_deref(), Some("qwen"));
}

#[test]
fn command_output_defaults_to_text() {
    let text = MINIMAL.replace(
        "url = \"http://127.0.0.1:1234\"",
        "url = \"http://127.0.0.1:1234\"\ncredential = { kind = \"command\", command = \"echo tok\" }",
    );
    match &parse(&text).unwrap().backends["local"].credential {
        CredentialConfig::Command { output, .. } => assert_eq!(*output, CommandOutput::Text),
        other => panic!("{other:?}"),
    }
}

#[test]
fn unknown_field_is_a_syntax_error() {
    let text = MINIMAL.replace(
        "token = \"secret\"",
        "token = \"secret\"\nlisten_addr = \"x\"",
    );
    let err = parse(&text).unwrap_err();
    assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    assert!(err.to_string().contains("listen_addr"), "{err}");
}

#[test]
fn env_expansion_resolves_and_escapes() {
    let lookup = |name: &str| match name {
        "TOKEN" => Some("from-env".to_owned()),
        _ => None,
    };
    let text = MINIMAL.replace("\"secret\"", "\"${TOKEN}\"")
        + "\n[backends.local.headers]\nx-literal = \"$${HOME}\"\n";
    let c = Config::parse(&text, lookup).unwrap();
    assert_eq!(c.server.token, "from-env");
    assert_eq!(c.backends["local"].headers["x-literal"], "${HOME}");
}

#[test]
fn env_expansion_reports_missing_variable() {
    let text = MINIMAL.replace("\"secret\"", "\"${NOPE}\"");
    let err = parse(&text).unwrap_err();
    assert!(
        matches!(err, ConfigError::MissingEnv(ref n) if n == "NOPE"),
        "{err}"
    );
}

#[test]
fn env_expansion_ignores_comments_and_keys() {
    let text = r#"
# a comment mentioning ${NOT_SET}
[server]
token = "t"

[backends."b-${LITERAL_KEY}"]
url = "http://x"

[[models]]
id = "m"
backend = "b-$${LITERAL_KEY}"
"#;
    let c = parse(text).unwrap();
    assert!(c.backends.contains_key("b-${LITERAL_KEY}"));
}

#[test]
fn env_expansion_leaves_bare_dollar_alone() {
    let text = MINIMAL.to_owned() + "\n[backends.local.headers]\nx = \"cost $5 and $x\"\n";
    let c = parse(&text).unwrap();
    assert_eq!(c.backends["local"].headers["x"], "cost $5 and $x");
}

#[test]
fn validation_collects_every_problem() {
    let text = r#"
[server]
token = ""

[backends.a]
url = "ftp://nope"
credential = { kind = "command", command = "  " }
headers = { "bad header" = "v" }

[[models]]
id = "m"
backend = "missing"

[[models]]
id = "m"
backend = "a"
aliases = ["m"]

[routing]
default_model = "ghost"

[upstream]
retry_on_status = [42]
"#;
    let p = problems(text);
    let joined = p.join("\n");
    for needle in [
        "server.token",
        "backends.a.url",
        "backends.a.credential.command",
        "bad header",
        "models[0].backend",
        "duplicate",
        "routing.default_model",
        "retry_on_status",
    ] {
        assert!(joined.contains(needle), "missing `{needle}` in:\n{joined}");
    }
}

#[test]
fn validation_requires_a_backend_and_a_model() {
    let p = problems("[server]\ntoken = \"t\"\n");
    let joined = p.join("\n");
    assert!(joined.contains("backend"), "{joined}");
    assert!(joined.contains("model"), "{joined}");
}

#[test]
fn byte_size_accepts_plain_integer_and_units() {
    assert_eq!(byte_size::parse("10").unwrap(), 10);
    assert_eq!(byte_size::parse("2kb").unwrap(), 2_000);
    assert_eq!(byte_size::parse("2 KiB").unwrap(), 2_048);
    assert_eq!(byte_size::parse("1GiB").unwrap(), 1 << 30);
    assert!(byte_size::parse("MiB").is_err());
    assert!(byte_size::parse("3 parsecs").is_err());
    let text = MINIMAL.replace(
        "token = \"secret\"",
        "token = \"secret\"\nmax_body_bytes = 1024",
    );
    assert_eq!(parse(&text).unwrap().server.max_body_bytes, 1024);
}

#[test]
fn load_reports_unreadable_path() {
    let err = Config::load(std::path::Path::new("/definitely/not/here.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::Read { .. }), "{err}");
    assert!(err.to_string().contains("/definitely/not/here.toml"));
}

#[test]
fn validation_rejects_header_values_and_beta_flags_that_cannot_be_sent() {
    let text = r#"
[server]
token = "t"

[backends.a]
url = "http://a"
headers = { "x-ok" = "fine", "x-bad" = "line\nbreak" }
anthropic_beta = ["fine-2026-01-01", "has,comma", "line\nbreak"]

[[models]]
id = "m"
backend = "a"
"#;
    let p = problems(text);
    let joined = p.join("\n");
    assert!(joined.contains("backends.a.headers: `x-bad`"), "{joined}");
    assert!(
        joined.contains("anthropic_beta") && joined.contains("has,comma"),
        "{joined}"
    );
    assert!(joined.contains("line\\nbreak"), "{joined}");
    assert_eq!(p.len(), 3, "{joined}");
}

#[test]
fn validation_rejects_names_that_cannot_travel_in_a_header() {
    let text = r#"
[server]
token = "t"

[backends."line\nbreak"]
url = "http://a"

[[models]]
id = "id\nbreak"
backend = "line\nbreak"
upstream_model = "up\nbreak"
"#;
    let p = problems(text);
    let joined = p.join("\n");
    assert!(joined.contains("backends.line\\nbreak"), "{joined}");
    assert!(joined.contains("models[0].id"), "{joined}");
    assert!(joined.contains("models[0].upstream_model"), "{joined}");
    assert_eq!(p.len(), 3, "{joined}");
}

#[test]
fn overrides_are_normalized_like_the_file() {
    let overrides = Overrides {
        listen: Some("127.0.0.1:1".parse().unwrap()),
        body_dir: Some("~/override".into()),
    };
    let c = parse(MINIMAL).unwrap().with_overrides(&overrides).unwrap();
    assert_eq!(c.server.listen, "127.0.0.1:1".parse().unwrap());
    assert_eq!(
        c.logging.body_dir.unwrap(),
        dirs::home_dir().unwrap().join("override")
    );
    let untouched = parse(MINIMAL)
        .unwrap()
        .with_overrides(&Overrides::default())
        .unwrap();
    assert_eq!(untouched.logging.body_dir, None);
}

#[test]
fn body_dir_tilde_expands_to_home() {
    let text = MINIMAL.to_owned() + "\n[logging]\nbody_dir = \"~/state/bodies\"\n";
    let c = parse(&text).unwrap();
    let home = dirs::home_dir().unwrap();
    assert_eq!(c.logging.body_dir.unwrap(), home.join("state/bodies"));

    let text = MINIMAL.to_owned() + "\n[logging]\nbody_dir = \"/abs/path\"\n";
    assert_eq!(
        parse(&text).unwrap().logging.body_dir.unwrap(),
        std::path::PathBuf::from("/abs/path")
    );
}
