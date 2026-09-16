//! Cross-field rules. Every problem is collected so the operator fixes the
//! file in one pass.

use std::collections::{HashMap, HashSet};

use super::view::redacted_url;
use super::{BackendKind, Config, ConfigError, CredentialConfig, origin};

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
    // `0s` turns a limit off elsewhere in the file (`logging.body_retention`),
    // so it is worth saying that here it expires instead of lifting.
    for (field, value) in [
        ("upstream.connect_timeout", config.upstream.connect_timeout),
        ("upstream.read_timeout", config.upstream.read_timeout),
    ] {
        if value.is_zero() {
            problems.push(format!(
                "{field}: 0 is not `no limit` here; it expires before the backend can answer"
            ));
        }
    }
    for status in &config.upstream.retry_on_status {
        if !(100..=599).contains(status) {
            problems.push(format!(
                "upstream.retry_on_status: {status} is not an HTTP status"
            ));
        }
    }

    for (name, backend) in &config.backends {
        if name.trim().is_empty() {
            problems.push("backends: the name of a backend must not be empty".to_owned());
        }
        if !header_safe(name) {
            problems.push(format!(
                "backends.{}: the name is sent in `x-anthroxy-backend` and must be header-safe",
                name.escape_debug()
            ));
        }
        check_url(&format!("backends.{name}.url"), &backend.url, &mut problems);
        check_models_path(name, &backend.models_path, &mut problems);
        if let Some(proxy) = &backend.proxy {
            check_proxy(&format!("backends.{name}.proxy"), proxy, &mut problems);
        }
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
        if let CredentialConfig::Command { timeout, .. } = &backend.credential
            && timeout.is_zero()
        {
            problems.push(format!(
                "backends.{name}.credential.timeout: 0 is not `no limit` here; it kills the command before it can print"
            ));
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
        if backend.kind == BackendKind::OpenAi && !backend.anthropic_beta.is_empty() {
            problems.push(format!(
                "backends.{name}.anthropic_beta: not sent to a backend with kind = \"openai\""
            ));
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

    check_proxies_per_origin(config, &mut problems);

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
        if let Some(upstream_model) = &model.upstream_model {
            if upstream_model.trim().is_empty() {
                problems.push(format!(
                    "models[{index}].upstream_model must not be empty; leave it out to send the id"
                ));
            } else if !header_safe(upstream_model) {
                problems.push(format!(
                    "models[{index}].upstream_model: `{}` is sent in `x-anthroxy-upstream-model` and must be header-safe",
                    upstream_model.escape_debug()
                ));
            }
        }
        if model.aliases.iter().any(|a| a.trim().is_empty()) {
            problems.push(format!(
                "models[{index}].aliases: an alias must not be empty"
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

/// Connections to one origin share a pool and the proxy is chosen per origin,
/// so every backend on an origin must name the same proxy or none. Backends
/// whose URL or proxy is already reported are left out.
fn check_proxies_per_origin(config: &Config, problems: &mut Vec<String>) {
    let mut routes: HashMap<String, (&str, Option<reqwest::Url>)> = HashMap::new();
    for (name, backend) in &config.backends {
        let Some(origin) = reqwest::Url::parse(&backend.url)
            .ok()
            .as_ref()
            .and_then(origin)
        else {
            continue;
        };
        let proxy = match backend.proxy.as_deref().map(reqwest::Url::parse) {
            None => None,
            Some(Ok(url)) => Some(url),
            Some(Err(_)) => continue,
        };
        match routes.get(&origin) {
            None => {
                routes.insert(origin, (name, proxy));
            }
            Some((first, other)) if *other != proxy => {
                let how = match (other, &proxy) {
                    (None, _) => "directly",
                    (Some(_), None) => "through a proxy",
                    (Some(_), Some(_)) => "through another proxy",
                };
                problems.push(format!(
                    "backends.{name}.proxy: backend `{first}` reaches the same origin {origin} {how}; backends on one origin must name the same proxy or none"
                ));
            }
            Some(_) => {}
        }
    }
}

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

/// Appended to `url` as the request target, so it is a path and may carry a
/// query, but nothing before the first `/`.
fn check_models_path(name: &str, path: &str, problems: &mut Vec<String>) {
    let shown = path.escape_debug();
    if !path.starts_with('/') {
        problems.push(format!(
            "backends.{name}.models_path: `{shown}` must start with `/`; it is appended to `url`"
        ));
    } else if path.parse::<http::uri::PathAndQuery>().is_err() {
        problems.push(format!(
            "backends.{name}.models_path: `{shown}` is not a valid request path"
        ));
    }
}

/// Connections are opened to this URL, so it is an origin and nothing more.
/// Its userinfo may be a password and stays out of the message.
fn check_proxy(field: &str, proxy: &str, problems: &mut Vec<String>) {
    let shown = redacted_url(proxy);
    match reqwest::Url::parse(proxy) {
        Ok(url) if !matches!(url.scheme(), "http" | "https") || !url.has_host() => {
            problems.push(format!(
                "{field}: `{shown}` must be an http:// or https:// proxy URL"
            ));
        }
        Ok(url) if url.path() != "/" || url.query().is_some() || url.fragment().is_some() => {
            problems.push(format!(
                "{field}: `{shown}` must carry no path, query or fragment"
            ));
        }
        Ok(_) => {}
        Err(e) => problems.push(format!("{field}: `{shown}` is not a valid URL ({e})")),
    }
}
