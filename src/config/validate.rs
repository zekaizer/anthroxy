//! Cross-field rules. Every problem is collected so the operator fixes the
//! file in one pass.

use std::collections::{HashMap, HashSet};

use super::view::redacted_url;
use super::{
    BackendKind, Config, ConfigError, CredentialConfig, EmulateConfig, PresetLibrary, Template,
    TokenConfig, V1Auth, origin,
};
use super::{header_command, header_rules, set_fields};
use crate::openai::SystemPlacement;
use crate::text::short;

pub fn validate(config: &Config) -> Result<(), ConfigError> {
    let mut problems = Vec::new();

    match &config.server.token {
        TokenConfig::Literal(value) if value.trim().is_empty() => {
            problems.push("server.token must not be empty".to_owned());
        }
        TokenConfig::Env { name } if name.trim().is_empty() => {
            problems.push("server.token.name must not be empty".to_owned());
        }
        _ => {}
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
    // With the token gate on, the client's Authorization is the router token,
    // and a passthrough backend would forward that token to its origin.
    if passthrough > 0 && config.server.v1_auth == V1Auth::Token {
        problems.push(
            "backends: kind = \"passthrough\" requires server.v1_auth = \"none\"; the client's own credential is what it forwards"
                .to_owned(),
        );
    }
    let live_models = config.backends.values().any(|b| b.live_models);
    if config.models.is_empty() && passthrough == 0 && !live_models {
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
                short(name)
            ));
        }
        // Operator strings reach the log and the console one problem per
        // line; each travels cut and escaped.
        let name = &short(name);
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
        header_rules::check_forced_headers(
            &format!("backends.{name}.headers"),
            &backend.headers,
            |value| Template::parse(value).map(|template| template.reads_client()),
            &mut problems,
        );
        let forced: Vec<&str> = backend.headers.keys().map(String::as_str).collect();
        header_rules::check_drop_headers(
            &format!("backends.{name}.drop_headers"),
            &backend.drop_headers,
            &forced,
            "this backend",
            &mut problems,
        );
        if let Some(command) = &backend.header_command {
            header_command::check(name, command, &mut problems);
            if backend.kind == BackendKind::Passthrough {
                problems.push(format!(
                    "backends.{name}: kind = \"passthrough\" relays the request unmodified; a header_command is not run"
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
                    "backends.{name}.drop_fields: `{}` has an empty segment",
                    short(path)
                ));
            }
        }
        let fields = set_fields::leaves(&backend.set_fields);
        for (path, value) in &fields {
            if let Err(problem) = set_fields::check(path, value) {
                problems.push(format!(
                    "backends.{name}.set_fields: `{}`: {problem}",
                    short(path)
                ));
            }
        }
        for (outer, inner) in set_fields::overlaps(fields.iter().map(|(path, _)| path)) {
            problems.push(format!(
                "backends.{name}.set_fields: `{}` and `{}` set the same field",
                short(outer),
                short(inner)
            ));
        }
        if backend.kind != BackendKind::OpenAi
            && backend
                .mid_conversation_system
                .is_some_and(|placement| placement != SystemPlacement::Keep)
        {
            problems.push(format!(
                "backends.{name}.mid_conversation_system: applies only to kind = \"openai\""
            ));
        }
        if let Some(emulate) = &backend.emulate {
            let field = format!("backends.{name}.emulate");
            if backend.kind != BackendKind::OpenAi {
                problems.push(format!("{field}: applies only to kind = \"openai\""));
            }
            check_emulate(&field, emulate, &config.presets, &mut problems);
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
                || !backend.set_fields.is_empty()
                || !backend.headers.is_empty()
                || !backend.anthropic_beta.is_empty()
            {
                problems.push(format!(
                    "backends.{name}: kind = \"passthrough\" relays the request unmodified; drop_fields, set_fields, headers and anthropic_beta are not applied"
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
        if model.mid_conversation_system.is_some()
            && config
                .backends
                .get(&model.backend)
                .is_some_and(|backend| backend.kind != BackendKind::OpenAi)
        {
            problems.push(format!(
                "models[{index}].mid_conversation_system: applies only to a backend with kind = \"openai\""
            ));
        }
        if let Some(emulate) = &model.emulate {
            let field = format!("models[{index}].emulate");
            if config
                .backends
                .get(&model.backend)
                .is_some_and(|backend| backend.kind != BackendKind::OpenAi)
            {
                problems.push(format!(
                    "{field}: applies only to a backend with kind = \"openai\""
                ));
            }
            check_emulate(&field, emulate, &config.presets, &mut problems);
        }
        if !config.backends.contains_key(&model.backend) {
            problems.push(format!(
                "models[{index}].backend: `{}` is not a configured backend",
                short(&model.backend)
            ));
        } else if config.backends[&model.backend].kind == BackendKind::Passthrough {
            problems.push(format!(
                "models[{index}].backend: `{0}` has kind = \"passthrough\"; its models come from the backend, not [[models]]",
                short(&model.backend)
            ));
        }
        for id in std::iter::once(&model.id).chain(&model.aliases) {
            if !seen.insert(id.as_str()) {
                problems.push(format!(
                    "models[{index}]: duplicate model id or alias `{}`",
                    short(id)
                ));
            }
        }
    }

    if let Some(default) = &config.routing.default_model
        && !seen.contains(default.as_str())
    {
        problems.push(format!(
            "routing.default_model: `{}` is not a configured model id or alias",
            short(default)
        ));
    }

    problems.extend(config.presets.problems().iter().cloned());

    if problems.is_empty() {
        Ok(())
    } else {
        Err(ConfigError::Invalid(problems))
    }
}

/// The problems of an `emulate` key written at `field`: what keeps
/// [`PresetLibrary::bind`] from binding it.
fn check_emulate(
    field: &str,
    emulate: &EmulateConfig,
    presets: &PresetLibrary,
    problems: &mut Vec<String>,
) {
    if let Err(found) = presets.bind(emulate) {
        problems.extend(found.iter().map(|problem| problem.at(field)));
    }
}

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
                    "backends.{}.proxy: backend `{}` reaches the same origin {origin} {how}; backends on one origin must name the same proxy or none",
                    short(name),
                    short(first)
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
    // Userinfo here may be a password, and a rejected URL is quoted back in
    // a log line and to the console.
    let shown = redacted_url(url);
    match url.parse::<http::Uri>() {
        Ok(uri) if !matches!(uri.scheme_str(), Some("http" | "https")) || uri.host().is_none() => {
            problems.push(format!(
                "{field}: `{shown}` must be an http:// or https:// origin"
            ));
        }
        Ok(uri) if uri.query().is_some() || url.contains('#') => {
            problems.push(format!(
                "{field}: `{shown}` must carry no query or fragment; the request path is appended to it"
            ));
        }
        Ok(_) => {}
        Err(e) => problems.push(format!("{field}: `{shown}` is not a valid URL ({e})")),
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
