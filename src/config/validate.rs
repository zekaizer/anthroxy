//! Cross-field rules. Every problem is collected so the operator fixes the
//! file in one pass.

use std::collections::HashSet;

use super::{Config, ConfigError, CredentialConfig};

pub fn validate(config: &Config) -> Result<(), ConfigError> {
    let mut problems = Vec::new();

    if config.server.token.trim().is_empty() {
        problems.push("server.token must not be empty".to_owned());
    }
    if config.backends.is_empty() {
        problems.push("at least one [backends.<name>] is required".to_owned());
    }
    if config.models.is_empty() {
        problems.push("at least one [[models]] entry is required".to_owned());
    }
    for status in &config.upstream.retry_on_status {
        if !(100..=599).contains(status) {
            problems.push(format!(
                "upstream.retry_on_status: {status} is not an HTTP status"
            ));
        }
    }

    for (name, backend) in &config.backends {
        if !header_safe(name) {
            problems.push(format!(
                "backends.{}: the name is sent in `x-anthroxy-backend` and must be header-safe",
                name.escape_debug()
            ));
        }
        check_url(&format!("backends.{name}.url"), &backend.url, &mut problems);
        match &backend.credential {
            CredentialConfig::Command { command, .. } if command.trim().is_empty() => {
                problems.push(format!(
                    "backends.{name}.credential.command must not be empty"
                ));
            }
            CredentialConfig::Env { name: var, .. } if var.trim().is_empty() => {
                problems.push(format!("backends.{name}.credential.name must not be empty"));
            }
            CredentialConfig::Static { value, .. } if value.is_empty() => {
                problems.push(format!(
                    "backends.{name}.credential.value must not be empty"
                ));
            }
            _ => {}
        }
        for (header, value) in &backend.headers {
            if http::HeaderName::from_bytes(header.as_bytes()).is_err() {
                problems.push(format!(
                    "backends.{name}.headers: `{header}` is not a valid header name"
                ));
            } else if CONNECTION_HEADERS.contains(&header.to_ascii_lowercase().as_str()) {
                problems.push(format!(
                    "backends.{name}.headers: `{header}` describes the connection the router makes and cannot be set here"
                ));
            } else if http::HeaderValue::from_str(value).is_err() {
                problems.push(format!(
                    "backends.{name}.headers: `{header}` has a value that cannot be sent in a header"
                ));
            }
        }
        for path in &backend.drop_fields {
            if path == "model" {
                problems.push(format!(
                    "backends.{name}.drop_fields: `model` is the routing key and cannot be dropped"
                ));
            } else if path.split('.').any(str::is_empty) {
                problems.push(format!(
                    "backends.{name}.drop_fields: `{path}` has an empty segment"
                ));
            }
        }
        for flag in &backend.anthropic_beta {
            if flag.contains(',') || http::HeaderValue::from_str(flag).is_err() {
                problems.push(format!(
                    "backends.{name}.anthropic_beta: `{}` is not a single header-safe flag",
                    flag.escape_debug()
                ));
            }
        }
    }

    let mut seen = HashSet::new();
    for (index, model) in config.models.iter().enumerate() {
        if model.id.trim().is_empty() {
            problems.push(format!("models[{index}].id must not be empty"));
        } else if !header_safe(&model.id) {
            problems.push(format!(
                "models[{index}].id: `{}` is sent in `x-anthroxy-model` and must be header-safe",
                model.id.escape_debug()
            ));
        }
        if let Some(upstream_model) = &model.upstream_model
            && !header_safe(upstream_model)
        {
            problems.push(format!(
                "models[{index}].upstream_model: `{}` is sent in `x-anthroxy-upstream-model` and must be header-safe",
                upstream_model.escape_debug()
            ));
        }
        if !config.backends.contains_key(&model.backend) {
            problems.push(format!(
                "models[{index}].backend: `{}` is not a configured backend",
                model.backend
            ));
        }
        for id in std::iter::once(&model.id).chain(&model.aliases) {
            if !seen.insert(id.as_str()) {
                problems.push(format!(
                    "models[{index}]: duplicate model id or alias `{id}`"
                ));
            }
        }
    }

    if let Some(default) = &config.routing.default_model
        && !seen.contains(default.as_str())
    {
        problems.push(format!(
            "routing.default_model: `{default}` is not a configured model id or alias"
        ));
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Invalid(problems))
    }
}

/// Framing and hop-by-hop headers: the HTTP client owns them, so a value set
/// here is either dropped or produces a request no backend can read. `host` is
/// not one of them; overriding it is how some gateways are addressed.
const CONNECTION_HEADERS: [&str; 8] = [
    "content-length",
    "transfer-encoding",
    "connection",
    "keep-alive",
    "proxy-connection",
    "te",
    "trailer",
    "upgrade",
];

fn header_safe(text: &str) -> bool {
    http::HeaderValue::from_str(text).is_ok()
}

/// The request path is appended to this URL unchanged, so it may carry a path
/// prefix but nothing that has to stay last.
fn check_url(field: &str, url: &str, problems: &mut Vec<String>) {
    match url.parse::<http::Uri>() {
        Ok(uri) if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.host().is_none() => {
            problems.push(format!(
                "{field}: `{url}` must be an http:// or https:// origin"
            ));
        }
        Ok(uri) if uri.query().is_some() || url.contains('#') => {
            problems.push(format!(
                "{field}: `{url}` must carry no query or fragment; the request path is appended to it"
            ));
        }
        Ok(_) => {}
        Err(e) => problems.push(format!("{field}: `{url}` is not a valid URL ({e})")),
    }
}
