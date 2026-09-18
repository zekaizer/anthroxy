//! Cross-field rules. Every problem is collected so the operator fixes the
//! file in one pass.

use std::collections::{HashMap, HashSet};

use super::view::redacted_url;
use super::{
    BackendConfig, BackendKind, Config, ConfigError, CredentialConfig, HeaderPattern, V1Auth,
    origin,
};

pub fn validate(config: &Config) -> Result<(), ConfigError> {
    let mut problems = Vec::new();

    if config.server.token.trim().is_empty() {
        problems.push("server.token must not be empty".to_owned());
    }
    if config.server.v1_auth == V1Auth::None && !config.server.listen.ip().is_loopback() {
        problems.push(
            "server.v1_auth = \"none\" requires server.listen on a loopback address".to_owned(),
        );
    }
    if config.backends.is_empty() {
        problems.push("at least one [backends.<name>] is required".to_owned());
    }
    let passthrough = config
        .backends
        .iter()
        .filter(|(_, b)| b.kind == BackendKind::Passthrough)
        .count();
    if passthrough > 1 {
        problems.push("at most one backend with kind = \"passthrough\" is allowed".to_owned());
    }
    if config.models.is_empty() && passthrough == 0 {
        problems.push("at least one [[models]] entry is required".to_owned());
    }
    // `0s` turns a limit off elsewhere in the file (`logging.body_retention`),
    // so it is worth saying that here it expires instead of lifting.
    for (field, value) in [
        ("upstream.connect_timeout", config.upstream.connect_timeout),
        (
            "upstream.non_stream_timeout",
            config.upstream.non_stream_timeout,
        ),
        (
            "upstream.stream_first_byte_timeout",
            config.upstream.stream_first_byte_timeout,
        ),
        (
            "upstream.stream_idle_timeout",
            config.upstream.stream_idle_timeout,
        ),
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
        check_drop_headers(name, backend, &mut problems);
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
        if backend.kind == BackendKind::Passthrough {
            if !matches!(backend.credential, CredentialConfig::None) {
                problems.push(format!(
                    "backends.{name}.credential: kind = \"passthrough\" forwards the client's Authorization; a backend credential is not allowed"
                ));
            }
            if !backend.drop_fields.is_empty()
                || !backend.headers.is_empty()
                || !backend.anthropic_beta.is_empty()
            {
                problems.push(format!(
                    "backends.{name}: kind = \"passthrough\" relays the request unmodified; drop_fields, headers and anthropic_beta are not applied"
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
        } else if config.backends[&model.backend].kind == BackendKind::Passthrough {
            problems.push(format!(
                "models[{index}].backend: `{0}` has kind = \"passthrough\"; its models come from the backend, not [[models]]",
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

/// Headers the router itself never forwards, so naming one to drop says
/// nothing.
const ALREADY_DROPPED: [&str; 6] = [
    "host",
    "content-length",
    "authorization",
    "x-api-key",
    "accept-encoding",
    "connection",
];

/// Headers the backend needs to read the request the router sends it.
const REQUIRED: [(&str, &str); 1] = [("content-type", "says what the body is")];

/// An entry must be a pattern, and must not take back what the rest of the
/// backend says: a header it forces, one the router drops anyway, or one the
/// request cannot be read without.
fn check_drop_headers(name: &str, backend: &BackendConfig, problems: &mut Vec<String>) {
    let field = format!("backends.{name}.drop_headers");
    for entry in &backend.drop_headers {
        let patterns = match HeaderPattern::parse(entry) {
            Ok(patterns) => patterns,
            Err(problem) => {
                problems.push(format!("{field}: {problem}"));
                continue;
            }
        };
        let shown = entry.escape_debug();
        let hits = |header: &str| patterns.iter().any(|pattern| pattern.matches(header));
        // A pattern says which header it caught; a name is already the name.
        let caught = |header: &str| match entry.eq_ignore_ascii_case(header) {
            true => String::new(),
            false => format!(" (`{header}`)"),
        };
        if let Some(forced) = backend
            .headers
            .keys()
            .find(|k| hits(&k.to_ascii_lowercase()))
        {
            problems.push(format!(
                "{field}: `{shown}` is set in `headers` for this backend{}; a header is forced or dropped, not both",
                caught(&forced.to_ascii_lowercase())
            ));
        }
        if let Some(header) = ALREADY_DROPPED.iter().find(|header| hits(header)) {
            problems.push(format!(
                "{field}: `{shown}` never reaches a backend anyway{}",
                caught(header)
            ));
        }
        if let Some((header, why)) = REQUIRED.iter().find(|(header, _)| hits(header)) {
            problems.push(format!("{field}: `{shown}` {why}{}", caught(header)));
        }
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
