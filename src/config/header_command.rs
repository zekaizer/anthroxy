//! Rules for `backends.<name>.header_command` (ADR-0021).

use super::{HeaderCommandConfig, Template, TemplateError};
use crate::text::short;

/// Variables the router sets for every run.
const RESERVED_PREFIX: &str = "ANTHROXY_";

pub fn check(backend: &str, config: &HeaderCommandConfig, problems: &mut Vec<String>) {
    let field = format!("backends.{backend}.header_command");
    if config.command.trim().is_empty() {
        problems.push(format!("{field}.command must not be empty"));
    }
    if config.timeout.is_zero() {
        problems.push(format!(
            "{field}.timeout: 0 is not `no limit` here; it kills the command before it can print"
        ));
    }
    for (name, value) in &config.env {
        if !is_variable_name(name) {
            problems.push(format!(
                "{field}.env: `{}` is not a variable name",
                short(name)
            ));
        } else if name.to_ascii_uppercase().starts_with(RESERVED_PREFIX) {
            problems.push(format!(
                "{field}.env: `{name}`: names beginning `{RESERVED_PREFIX}` are the router's own"
            ));
        }
        match Template::parse(value) {
            Ok(_) => {}
            // The value may be a secret, and what follows `{header:` is part
            // of it: the problem quotes none of it.
            Err(TemplateError::NotAHeaderName(_)) => problems.push(format!(
                "{field}.env: `{}`: a `{{header:…}}` placeholder does not name a header",
                short(name)
            )),
            Err(problem) => problems.push(format!("{field}.env: `{}`: {problem}", short(name))),
        }
    }
}

/// What `sh` accepts: a letter or `_`, then letters, digits and `_`.
fn is_variable_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
