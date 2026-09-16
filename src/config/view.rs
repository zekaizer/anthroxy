//! The configuration as the console shows it: every setting in force, with
//! secrets replaced.

use std::time::Duration;

use serde_json::{Map, Value, json};

use super::{CommandOutput, Config, CredentialConfig, CredentialHeader, LogFormat};

pub const REDACTED: &str = "<redacted>";

/// `server.token`, `static` credential values, every forced header value and
/// the userinfo of a proxy URL are redacted; the rest is shown as loaded,
/// `${ENV}` expanded.
pub fn masked(config: &Config) -> Value {
    let backends: Map<String, Value> = config
        .backends
        .iter()
        .map(|(name, backend)| {
            let headers: Map<String, Value> = backend
                .headers
                .keys()
                .map(|header| (header.clone(), json!(REDACTED)))
                .collect();
            (
                name.clone(),
                json!({
                    "kind": backend.kind,
                    "url": backend.url,
                    "models_path": backend.models_path,
                    "credential": credential(&backend.credential),
                    "headers": headers,
                    "anthropic_beta": backend.anthropic_beta,
                    "drop_headers": backend.drop_headers,
                    "drop_fields": backend.drop_fields,
                    "proxy": backend.proxy.as_deref().map(redacted_url),
                }),
            )
        })
        .collect();
    let models: Vec<Value> = config
        .models
        .iter()
        .map(|model| {
            json!({
                "id": model.id,
                "backend": model.backend,
                "upstream_model": model.upstream_model,
                "display_name": model.display_name,
                "aliases": model.aliases,
            })
        })
        .collect();
    json!({
        "server": {
            "listen": config.server.listen.to_string(),
            "token": REDACTED,
            "max_body_bytes": config.server.max_body_bytes,
        },
        "logging": {
            "level": config.logging.level,
            "format": match config.logging.format {
                LogFormat::Text => "text",
                LogFormat::Json => "json",
            },
            "body_dir": config.logging.body_dir.as_ref().map(|dir| dir.display().to_string()),
            "body_retention": duration(config.logging.body_retention),
        },
        "upstream": {
            "connect_timeout": duration(config.upstream.connect_timeout),
            "read_timeout": duration(config.upstream.read_timeout),
            "retries": config.upstream.retries,
            "retry_backoff": duration(config.upstream.retry_backoff),
            "retry_on_status": config.upstream.retry_on_status,
            "ca_certificate": config.upstream.ca_certificate.as_ref().map(|file| file.display().to_string()),
        },
        "backends": backends,
        "models": models,
        "routing": {"default_model": config.routing.default_model},
        "stats": {
            "enabled": config.stats.enabled,
            "dir": config.stats.dir.display().to_string(),
            "retention": duration(config.stats.retention),
        },
    })
}

/// `url` with its userinfo, which may hold a password, replaced by
/// [`REDACTED`].
pub fn redacted_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..end].rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://{REDACTED}@{host}{}", &rest[end..]),
        None => url.to_owned(),
    }
}

fn duration(value: Duration) -> String {
    duration_text(value)
}

/// A duration in the largest whole unit it fills (`90d`, `5m`, `200ms`), the
/// way a configuration file spells it; anything finer falls back to
/// humantime's breakdown.
pub fn duration_text(value: Duration) -> String {
    let secs = value.as_secs();
    if value.subsec_nanos() == 0 {
        return match secs {
            0 => "0s".to_owned(),
            s if s % 86_400 == 0 => format!("{}d", s / 86_400),
            s if s % 3_600 == 0 => format!("{}h", s / 3_600),
            s if s % 60 == 0 => format!("{}m", s / 60),
            s => format!("{s}s"),
        };
    }
    if value.subsec_nanos().is_multiple_of(1_000_000) {
        return format!("{}ms", value.as_millis());
    }
    humantime::format_duration(value).to_string()
}

fn credential(config: &CredentialConfig) -> Value {
    match config {
        CredentialConfig::None => json!({"kind": "none"}),
        CredentialConfig::Static { header, .. } => {
            json!({"kind": "static", "value": REDACTED, "header": header_shape(header)})
        }
        CredentialConfig::Env { name, header } => {
            json!({"kind": "env", "name": name, "header": header_shape(header)})
        }
        CredentialConfig::Command {
            command,
            output,
            refresh,
            timeout,
            header,
        } => json!({
            "kind": "command",
            "command": command,
            "output": match output {
                CommandOutput::Text => "text",
                CommandOutput::Json => "json",
            },
            "refresh": duration(*refresh),
            "timeout": duration(*timeout),
            "header": header_shape(header),
        }),
    }
}

/// `authorization: Bearer …`
fn header_shape(header: &CredentialHeader) -> String {
    match &header.scheme {
        Some(scheme) => format!("{}: {scheme} …", header.name),
        None => format!("{}: …", header.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted_and_the_rest_is_shown() {
        let text = r#"
[server]
token = "router-token-value"

[upstream]
read_timeout = "2m"

[backends.a]
url = "http://a"
credential = { kind = "static", value = "static-secret-value", header = "x_api_key" }

[backends.a.headers]
"x-forced" = "forced-secret-value"

[backends.b]
url = "http://b"
credential = { kind = "command", command = "cat ~/.token", output = "json", refresh = "1m" }

[[models]]
id = "m"
backend = "a"
"#;
        let config = Config::parse(text, |_| None).unwrap();
        let view = masked(&config);
        let shown = view.to_string();
        for secret in [
            "router-token-value",
            "static-secret-value",
            "forced-secret-value",
        ] {
            assert!(!shown.contains(secret), "{secret} leaked: {shown}");
        }
        assert_eq!(view["server"]["token"], REDACTED);
        assert_eq!(view["upstream"]["read_timeout"], "2m");
        assert_eq!(
            view["backends"]["a"]["credential"]["header"],
            "x-api-key: …"
        );
        assert_eq!(view["backends"]["a"]["headers"]["x-forced"], REDACTED);
        assert_eq!(
            view["backends"]["b"]["credential"]["command"],
            "cat ~/.token"
        );
        assert_eq!(view["backends"]["b"]["credential"]["refresh"], "1m");
        assert_eq!(
            view["backends"]["b"]["credential"]["header"],
            "authorization: Bearer …"
        );
        assert_eq!(view["models"][0]["id"], "m");
        assert_eq!(view["stats"]["retention"], "90d");
    }

    #[test]
    fn the_userinfo_of_a_proxy_is_redacted() {
        let text = r#"
[server]
token = "t"

[backends.a]
url = "https://gw.corp"
proxy = "http://alice:proxy-secret@proxy.corp:3128"

[backends.b]
url = "http://b"
proxy = "http://proxy.corp:3128"

[backends.c]
url = "http://c"

[[models]]
id = "m"
backend = "a"
"#;
        let view = masked(&Config::parse(text, |_| None).unwrap());
        assert_eq!(
            view["backends"]["a"]["proxy"],
            "http://<redacted>@proxy.corp:3128"
        );
        assert_eq!(view["backends"]["b"]["proxy"], "http://proxy.corp:3128");
        assert_eq!(view["backends"]["c"]["proxy"], Value::Null);
        let shown = view.to_string();
        assert!(
            !shown.contains("alice") && !shown.contains("proxy-secret"),
            "{shown}"
        );

        assert_eq!(
            redacted_url("http://token@proxy.corp"),
            "http://<redacted>@proxy.corp"
        );
        assert_eq!(redacted_url("https://host/a@b"), "https://host/a@b");
    }

    #[test]
    fn durations_read_the_way_a_file_writes_them() {
        for (value, text) in [
            (Duration::from_secs(90 * 86_400), "90d"),
            (Duration::from_secs(7_200), "2h"),
            (Duration::from_secs(300), "5m"),
            (Duration::from_secs(90), "90s"),
            (Duration::from_millis(200), "200ms"),
            (Duration::from_millis(1_500), "1500ms"),
            (Duration::ZERO, "0s"),
        ] {
            assert_eq!(duration_text(value), text);
            assert_eq!(humantime::parse_duration(text).unwrap(), value, "{text}");
        }
    }
}
