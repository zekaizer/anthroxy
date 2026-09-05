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
            } else if http::HeaderValue::from_str(value).is_err() {
                problems.push(format!(
                    "backends.{name}.headers: `{header}` has a value that cannot be sent in a header"
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

fn check_url(field: &str, url: &str, problems: &mut Vec<String>) {
    match url.parse::<http::Uri>() {
        Ok(uri) if matches!(uri.scheme_str(), Some("http" | "https")) && uri.host().is_some() => {}
        Ok(_) => problems.push(format!(
            "{field}: `{url}` must be an http:// or https:// origin"
        )),
        Err(e) => problems.push(format!("{field}: `{url}` is not a valid URL ({e})")),
    }
}
