use std::time::Duration;

use super::*;
use crate::openai::SystemPlacement;

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
    assert_eq!(c.upstream.non_stream_timeout, Duration::from_secs(900));
    assert_eq!(
        c.upstream.stream_first_byte_timeout,
        Duration::from_secs(300)
    );
    assert_eq!(c.upstream.stream_idle_timeout, Duration::from_secs(60));
    assert!(matches!(
        c.backends["local"].credential,
        CredentialConfig::None
    ));
    assert_eq!(c.models[0].upstream_model, None);
    assert_eq!(c.routing.default_model, None);
    assert_eq!(c.logging.body_retention, Duration::from_secs(7 * 24 * 3600));
    assert!(c.stats.enabled);
    assert_eq!(c.stats.dir, StatsConfig::default_dir());
    assert!(c.stats.dir.ends_with("anthroxy/stats"));
    assert_eq!(c.stats.retention, Duration::from_secs(90 * 24 * 3600));
}

#[test]
fn stats_can_be_moved_turned_off_and_kept_forever() {
    let text = MINIMAL.to_owned()
        + "\n[stats]\nenabled = false\ndir = \"~/stats-here\"\nretention = \"0s\"\n";
    let c = parse(&text).unwrap();
    assert!(!c.stats.enabled);
    assert_eq!(c.stats.retention, Duration::ZERO);
    if let Some(home) = dirs::home_dir() {
        assert_eq!(
            c.stats.dir,
            home.join("stats-here"),
            "`~` is the home directory"
        );
    }
    let unknown = MINIMAL.to_owned() + "\n[stats]\nkeep = true\n";
    assert!(matches!(parse(&unknown), Err(ConfigError::Parse(_))));
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
non_stream_timeout = "10m"
stream_first_byte_timeout = "2m"
stream_idle_timeout = "30s"
retries = 1
retry_backoff = "50ms"
retry_on_status = [502, 503]
ca_certificate = "/etc/ssl/corp-root.pem"

[backends.claude]
url = "https://api.anthropic.com/"
credential = { kind = "command", command = "echo tok", output = "json", refresh = "1m", timeout = "2s" }
anthropic_beta = ["oauth-2025-04-20"]
headers = { "x-extra" = "1" }

[backends.vllm]
url = "http://10.0.0.2:8000"
credential = { kind = "static", value = "k", header = "x_api_key" }
drop_fields = ["context_management", "metadata.user_id"]

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
    assert_eq!(c.upstream.non_stream_timeout, Duration::from_secs(600));
    assert_eq!(
        c.upstream.stream_first_byte_timeout,
        Duration::from_secs(120)
    );
    assert_eq!(c.upstream.stream_idle_timeout, Duration::from_secs(30));
    assert_eq!(c.upstream.retry_on_status, vec![502, 503]);
    assert_eq!(
        c.upstream.ca_certificate.as_deref(),
        Some(std::path::Path::new("/etc/ssl/corp-root.pem"))
    );
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
            assert_eq!(*header, CredentialHeader::bearer());
        }
        other => panic!("{other:?}"),
    }
    match &c.backends["vllm"].credential {
        CredentialConfig::Static { header, .. } => {
            assert_eq!(*header, CredentialHeader::x_api_key())
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        c.backends["vllm"].drop_fields,
        ["context_management", "metadata.user_id"]
    );
    assert!(c.backends["claude"].drop_fields.is_empty());
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

fn with_local_credential(credential: &str) -> String {
    MINIMAL.replace(
        "url = \"http://127.0.0.1:1234\"",
        &format!("url = \"http://127.0.0.1:1234\"\ncredential = {credential}"),
    )
}

#[test]
fn credential_header_accepts_a_custom_name_and_scheme() {
    let text =
        with_local_credential(r#"{ kind = "static", value = "k", header = { name = "Api-Key" } }"#);
    match &parse(&text).unwrap().backends["local"].credential {
        CredentialConfig::Static { header, .. } => {
            assert_eq!(header.name.as_str(), "api-key");
            assert_eq!(header.scheme, None);
        }
        other => panic!("{other:?}"),
    }

    let text = with_local_credential(
        r#"{ kind = "env", name = "K", header = { name = "authorization", scheme = "Token" } }"#,
    );
    match &parse(&text).unwrap().backends["local"].credential {
        CredentialConfig::Env { header, .. } => assert_eq!(
            *header,
            CredentialHeader::custom("authorization", Some("Token".into())).unwrap()
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn credential_header_rejects_other_forms() {
    for (header, needle) in [
        (r#""basic""#, "bearer"),
        (r#"{ name = "bad header" }"#, "header name"),
        (r#"{ name = "x", scheme = "two words" }"#, "single word"),
        (r#"{ name = "x", schema = "Bearer" }"#, "unknown field"),
        (r#"{ scheme = "Bearer" }"#, "missing field"),
    ] {
        let text = with_local_credential(&format!(
            r#"{{ kind = "static", value = "k", header = {header} }}"#
        ));
        let err = parse(&text).err().unwrap().to_string();
        assert!(err.contains(needle), "{header}: {err}");
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
fn backend_url_carries_no_query_or_fragment() {
    // The request path is appended verbatim, so anything after it silently
    // lands in the wrong place.
    for url in ["http://h/v1?beta=1", "http://h/v1#frag", "http://h?x=1"] {
        let text = MINIMAL.replace("http://127.0.0.1:1234", url);
        let p = problems(&text);
        assert!(
            p.iter().any(|m| m.contains("backends.local.url")),
            "{url} accepted: {p:?}"
        );
    }

    let text = MINIMAL.replace("http://127.0.0.1:1234", "http://h/openai/v1");
    let c = parse(&text).expect("a path prefix is what the request path extends");
    assert_eq!(c.backends["local"].url, "http://h/openai/v1");
}

#[test]
fn backend_headers_cannot_take_over_the_connection() {
    for header in [
        "content-length",
        "Transfer-Encoding",
        "connection",
        "upgrade",
    ] {
        let text = format!("{MINIMAL}\n[backends.local.headers]\n\"{header}\" = \"x\"\n");
        let p = problems(&text);
        assert!(
            p.iter().any(|m| m.contains("backends.local.headers")),
            "{header} accepted: {p:?}"
        );
    }
    // A backend that wants a different Host still may have one.
    let text = format!("{MINIMAL}\n[backends.local.headers]\nhost = \"api.internal\"\n");
    assert!(parse(&text).is_ok());
}

#[test]
fn names_that_are_empty_are_rejected_wherever_they_appear() {
    let cases = [
        ("\n[backends.\"\"]\nurl = \"http://a\"\n", "backends:"),
        (
            "\n[[models]]\nid = \"m2\"\nbackend = \"local\"\nupstream_model = \"\"\n",
            "upstream_model",
        ),
        (
            "\n[[models]]\nid = \"m3\"\nbackend = \"local\"\naliases = [\"\"]\n",
            "aliases",
        ),
    ];
    for (extra, needle) in cases {
        let p = problems(&(MINIMAL.to_owned() + extra));
        assert!(
            p.iter().any(|m| m.contains(needle)),
            "{extra} accepted: {p:?}"
        );
    }
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
fn validation_rejects_unusable_drop_fields() {
    let text = r#"
[server]
token = "t"

[backends.a]
url = "http://a"
drop_fields = ["context_management", "metadata.user_id", "", "a..b", "model"]

[[models]]
id = "m"
backend = "a"
"#;
    let p = problems(text);
    let joined = p.join("\n");
    assert!(joined.contains("backends.a.drop_fields: `` "), "{joined}");
    assert!(
        joined.contains("backends.a.drop_fields: `a..b` "),
        "{joined}"
    );
    assert!(
        joined.contains("backends.a.drop_fields: `model` is the routing key"),
        "{joined}"
    );
    assert_eq!(p.len(), 3, "{joined}");
}

#[test]
fn models_path_defaults_to_the_anthropic_one_and_takes_an_override() {
    assert_eq!(
        parse(MINIMAL).unwrap().backends["local"].models_path,
        "/v1/models"
    );
    let text = MINIMAL.replace(
        "url = \"http://127.0.0.1:1234\"",
        "url = \"http://127.0.0.1:1234\"\nmodels_path = \"/llm/api/models\"",
    );
    assert_eq!(
        parse(&text).unwrap().backends["local"].models_path,
        "/llm/api/models"
    );
}

#[test]
fn live_models_defaults_off_and_allows_no_model_table() {
    assert!(!parse(MINIMAL).unwrap().backends["local"].live_models);
    let with_flag = MINIMAL.replace(
        "url = \"http://127.0.0.1:1234\"",
        "url = \"http://127.0.0.1:1234\"\nlive_models = true",
    );
    assert!(parse(&with_flag).unwrap().backends["local"].live_models);
    parse(
        r#"
[server]
token = "t"

[backends.grok]
kind = "openai"
url = "https://api.x.ai"
live_models = true
"#,
    )
    .unwrap();
}

#[test]
fn validation_rejects_a_models_path_that_is_not_one() {
    let text = r#"
[server]
token = "t"

[backends.a]
url = "http://a"
models_path = "v1/models"

[backends.b]
url = "http://b"
models_path = "/v1/mo dels"

[[models]]
id = "m"
backend = "a"
"#;
    let p = problems(text);
    let joined = p.join("\n");
    assert!(
        joined.contains("backends.a.models_path: `v1/models` must start with `/`"),
        "{joined}"
    );
    assert!(
        joined.contains("backends.b.models_path: `/v1/mo dels`"),
        "{joined}"
    );
    assert_eq!(p.len(), 2, "{joined}");
}

#[test]
fn validation_rejects_drop_headers_that_cannot_mean_what_they_say() {
    let text = r#"
[server]
token = "t"

[backends.a]
url = "http://a"
drop_headers = ["x-forced", "authorization", "content-type", "*", "@nope", "x header"]
headers = { "x-forced" = "1" }

[[models]]
id = "m"
backend = "a"
"#;
    let p = problems(text);
    let joined = p.join("\n");
    for needle in [
        "backends.a.drop_headers: `x-forced` is set in `headers` for this backend;",
        "backends.a.drop_headers: `authorization` never reaches a backend anyway",
        "backends.a.drop_headers: `content-type` says what the body is",
        "backends.a.drop_headers: `*` matches every header",
        "backends.a.drop_headers: `@nope` is not a preset",
        "backends.a.drop_headers: `x header` is not a header name",
    ] {
        assert!(joined.contains(needle), "{needle} missing from {joined}");
    }
    assert_eq!(p.len(), 6, "{joined}");
}

#[test]
fn a_pattern_that_covers_a_forced_or_required_header_is_rejected_too() {
    let text = r#"
[server]
token = "t"

[backends.a]
url = "http://a"
drop_headers = ["x-*", "content-*"]
headers = { "x-forced" = "1" }

[[models]]
id = "m"
backend = "a"
"#;
    let p = problems(text);
    let joined = p.join("\n");
    assert!(
        joined.contains("backends.a.drop_headers: `x-*` is set in `headers` for this backend"),
        "{joined}"
    );
    assert!(
        joined.contains("backends.a.drop_headers: `content-*` says what the body is"),
        "{joined}"
    );
    // A pattern wide enough to hit several rules is reported against each.
    assert!(
        joined.contains(
            "backends.a.drop_headers: `x-*` never reaches a backend anyway (`x-api-key`)"
        ),
        "{joined}"
    );
    assert!(
        joined.contains(
            "backends.a.drop_headers: `content-*` never reaches a backend anyway (`content-length`)"
        ),
        "{joined}"
    );
    assert_eq!(p.len(), 4, "{joined}");
}

#[test]
fn drop_headers_takes_names_patterns_and_the_preset() {
    let text = MINIMAL.replace(
        "url = \"http://127.0.0.1:1234\"",
        "url = \"http://127.0.0.1:1234\"\ndrop_headers = [\"x-stainless-*\", \"@claude-code\"]",
    );
    assert_eq!(
        parse(&text).unwrap().backends["local"].drop_headers,
        ["x-stainless-*", "@claude-code"]
    );
    assert!(
        parse(MINIMAL).unwrap().backends["local"]
            .drop_headers
            .is_empty()
    );
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
fn ca_certificate_tilde_expands_to_home() {
    let text = MINIMAL.to_owned() + "\n[upstream]\nca_certificate = \"~/corp-root.pem\"\n";
    let c = parse(&text).unwrap();
    assert_eq!(
        c.upstream.ca_certificate.unwrap(),
        dirs::home_dir().unwrap().join("corp-root.pem")
    );
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

#[cfg(unix)]
#[test]
fn a_configuration_open_to_other_accounts_is_reported_with_its_mode() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "").unwrap();
    let chmod = |mode| {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    };

    chmod(0o600);
    assert_eq!(open_to_other_accounts(&path), None);
    chmod(0o640);
    assert_eq!(open_to_other_accounts(&path), Some(0o640));
    chmod(0o644);
    assert_eq!(open_to_other_accounts(&path), Some(0o644));
    assert_eq!(
        open_to_other_accounts(&dir.path().join("absent.toml")),
        None,
        "a file that is not there is a problem the loader reports"
    );
}

#[test]
fn a_zero_timeout_is_rejected_where_zero_is_not_no_limit() {
    let text = r#"
[server]
token = "secret"

[upstream]
connect_timeout = "0s"
non_stream_timeout = "0s"
stream_first_byte_timeout = "0s"
stream_idle_timeout = "0s"

[backends.local]
url = "http://127.0.0.1:1234"
credential = { kind = "command", command = "cat token", timeout = "0s" }

[[models]]
id = "gemma"
backend = "local"
"#;
    let problems = problems(text);
    assert_eq!(problems.len(), 5, "{problems:?}");
    for field in [
        "upstream.connect_timeout",
        "upstream.non_stream_timeout",
        "upstream.stream_first_byte_timeout",
        "upstream.stream_idle_timeout",
        "backends.local.credential.timeout",
    ] {
        assert!(problems.iter().any(|p| p.contains(field)), "{problems:?}");
    }

    // `refresh` of zero has a meaning: run the command for every request.
    let text = MINIMAL.to_owned()
        + "\n[backends.local.credential]\nkind = \"command\"\ncommand = \"cat token\"\nrefresh = \"0s\"\n";
    assert!(matches!(
        parse(&text).unwrap().backends["local"].credential,
        CredentialConfig::Command { refresh, .. } if refresh.is_zero()
    ));
}

#[test]
fn backend_kind_defaults_to_anthropic_and_accepts_openai() {
    assert_eq!(
        parse(MINIMAL).unwrap().backends["local"].kind,
        BackendKind::Anthropic
    );
    let text = MINIMAL.replace("[backends.local]", "[backends.local]\nkind = \"openai\"");
    assert_eq!(
        parse(&text).unwrap().backends["local"].kind,
        BackendKind::OpenAi
    );
}

#[test]
fn v1_auth_none_requires_loopback_listen() {
    let p = problems(
        r#"
[server]
listen = "0.0.0.0:8787"
token = "t"
v1_auth = "none"

[backends.local]
url = "http://127.0.0.1:1"

[[models]]
id = "m"
backend = "local"
"#,
    );
    assert!(
        p.iter()
            .any(|s| s.contains("v1_auth") && s.contains("loopback")),
        "{p:?}"
    );
}

#[test]
fn passthrough_backend_allows_no_models_table() {
    let c = parse(
        r#"
[server]
listen = "127.0.0.1:8787"
token = "t"
v1_auth = "none"

[backends.account]
kind = "passthrough"
url = "https://api.anthropic.com"
"#,
    )
    .unwrap();
    assert_eq!(c.server.v1_auth, V1Auth::None);
    assert_eq!(c.backends["account"].kind, BackendKind::Passthrough);
    assert!(c.models.is_empty());
}

#[test]
fn passthrough_rejects_a_models_entry_and_a_second_passthrough() {
    let p = problems(
        r#"
[server]
listen = "127.0.0.1:8787"
token = "t"

[backends.account]
kind = "passthrough"
url = "https://api.anthropic.com"

[backends.other]
kind = "passthrough"
url = "https://example.com"

[[models]]
id = "m"
backend = "account"
"#,
    );
    let joined = p.join("\n");
    assert!(joined.contains("at most one"), "{joined}");
    assert!(joined.contains("passthrough"), "{joined}");
}

#[test]
fn passthrough_rejects_a_backend_credential() {
    let p = problems(
        r#"
[server]
listen = "127.0.0.1:8787"
token = "t"

[backends.account]
kind = "passthrough"
url = "https://api.anthropic.com"
credential = { kind = "static", value = "sk" }
"#,
    );
    assert!(
        p.iter()
            .any(|s| s.contains("credential") && s.contains("passthrough")),
        "{p:?}"
    );
}

#[test]
fn backend_kind_rejects_unknown_values() {
    let text = MINIMAL.replace("[backends.local]", "[backends.local]\nkind = \"responses\"");
    let error = parse(&text).unwrap_err();
    assert!(matches!(error, ConfigError::Parse(_)), "{error:?}");
    assert!(error.to_string().contains("responses"), "{error}");
}

#[test]
fn a_credential_command_keeps_its_env_references_for_run_time() {
    let text = r#"
[server]
token = "${TOKEN}"

[backends.a]
url = "http://a"
credential = { kind = "command", command = "vault read -token=${SECRET} x" }

[[models]]
id = "m"
backend = "a"
"#;
    let lookup = |name: &str| match name {
        "TOKEN" => Some("t".to_owned()),
        "SECRET" => Some("s".to_owned()),
        _ => None,
    };
    let c = Config::parse(text, lookup).unwrap();
    assert_eq!(c.server.token, "t");
    match &c.backends["a"].credential {
        CredentialConfig::Command { command, .. } => {
            assert_eq!(command, "vault read -token=${SECRET} x")
        }
        other => panic!("{other:?}"),
    }
    let missing = Config::parse(text, |name: &str| (name == "TOKEN").then(|| "t".to_owned()))
        .err()
        .unwrap();
    assert!(
        matches!(missing, ConfigError::MissingEnv(ref n) if n == "SECRET"),
        "{missing}"
    );
}

#[test]
fn mid_conversation_system_parses_and_defaults_to_keep() {
    let text = r#"
[server]
token = "t"

[backends.a]
kind = "openai"
url = "http://a"
mid_conversation_system = "merge"

[backends.b]
kind = "openai"
url = "http://b"

[[models]]
id = "m"
backend = "a"
"#;
    let c: Config = toml::from_str(text).unwrap();
    assert_eq!(
        c.backends["a"].mid_conversation_system,
        SystemPlacement::Merge
    );
    assert_eq!(
        c.backends["b"].mid_conversation_system,
        SystemPlacement::Keep
    );
}

#[test]
fn a_model_overrides_its_backend_mid_conversation_system() {
    let text = r#"
[server]
token = "t"

[backends.a]
kind = "openai"
url = "http://a"

[[models]]
id = "m"
backend = "a"
mid_conversation_system = "user"

[[models]]
id = "n"
backend = "a"
"#;
    let c: Config = toml::from_str(text).unwrap();
    assert_eq!(
        c.models[0].mid_conversation_system,
        Some(SystemPlacement::User)
    );
    assert_eq!(c.models[1].mid_conversation_system, None);
}

#[test]
fn validation_rejects_a_model_mid_conversation_system_off_openai_backend() {
    let text = r#"
[server]
token = "t"

[backends.a]
url = "http://a"

[[models]]
id = "m"
backend = "a"
mid_conversation_system = "merge"
"#;
    let joined = problems(text).join("\n");
    assert!(
        joined.contains(
            "models[0].mid_conversation_system: applies only to a backend with kind = \"openai\""
        ),
        "{joined}"
    );
}

#[test]
fn validation_rejects_mid_conversation_system_off_openai_backend() {
    let text = r#"
[server]
token = "t"

[backends.a]
url = "http://a"
mid_conversation_system = "merge"

[[models]]
id = "m"
backend = "a"
"#;
    let joined = problems(text).join("\n");
    assert!(
        joined.contains("backends.a.mid_conversation_system: applies only to kind = \"openai\""),
        "{joined}"
    );
}

#[test]
fn validation_rejects_anthropic_beta_on_openai_backend() {
    let text = MINIMAL.replace(
        "[backends.local]",
        "[backends.local]\nkind = \"openai\"\nanthropic_beta = [\"oauth-2025-04-20\"]",
    );
    let p = problems(&text);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(
        p[0].contains("backends.local.anthropic_beta") && p[0].contains("openai"),
        "{p:?}"
    );
}

fn with_proxy(proxy: &str) -> String {
    MINIMAL.replace(
        "url = \"http://127.0.0.1:1234\"\n",
        &format!("url = \"http://127.0.0.1:1234\"\nproxy = \"{proxy}\"\n"),
    )
}

#[test]
fn a_proxy_is_an_http_or_https_origin() {
    for proxy in [
        "http://proxy.corp:3128",
        "http://proxy.corp:3128/",
        "https://user:p%40ss@proxy.corp",
    ] {
        let c = parse(&with_proxy(proxy)).unwrap_or_else(|e| panic!("{proxy}: {e}"));
        assert_eq!(c.backends["local"].proxy.as_deref(), Some(proxy));
    }
    for proxy in [
        "",
        "proxy.corp:3128",
        "socks5://proxy.corp:1080",
        "http://",
        "http://proxy.corp:3128/path",
        "http://proxy.corp:3128/?x=1",
        "http://proxy.corp:3128/#top",
    ] {
        let p = problems(&with_proxy(proxy));
        assert!(
            p.iter().any(|m| m.contains("backends.local.proxy")),
            "{proxy:?} accepted: {p:?}"
        );
    }
}

#[test]
fn backends_on_one_origin_must_agree_on_the_proxy() {
    let mismatched = with_proxy("http://proxy.corp:3128")
        + "\n[backends.other]\nurl = \"http://127.0.0.1:1234/prefix\"\n";
    let p = problems(&mismatched);
    assert!(
        p.iter()
            .any(|m| m.contains("backends.other.proxy") && m.contains("`local`")),
        "{p:?}"
    );

    let agreed = with_proxy("http://proxy.corp:3128")
        + "\n[backends.other]\nurl = \"http://127.0.0.1:1234/prefix\"\nproxy = \"http://proxy.corp:3128/\"\n";
    assert!(parse(&agreed).is_ok());

    // The default port written out is still the same origin.
    let text = MINIMAL.to_owned()
        + "\n[backends.a]\nurl = \"https://gw.corp\"\nproxy = \"http://proxy.corp:3128\"\n"
        + "\n[backends.b]\nurl = \"https://gw.corp:443\"\n";
    let p = problems(&text);
    assert!(p.iter().any(|m| m.contains("backends.b.proxy")), "{p:?}");

    let text = MINIMAL.to_owned()
        + "\n[backends.a]\nurl = \"https://gw.corp\"\nproxy = \"http://proxy.corp:3128\"\n"
        + "\n[backends.b]\nurl = \"https://gw.corp:8443\"\n";
    assert!(parse(&text).is_ok(), "another port is another origin");
}
