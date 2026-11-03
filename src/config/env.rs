//! `${NAME}` expansion over string values of a parsed TOML document. Keys and
//! comments are never touched.

use super::ConfigError;

/// Why an environment variable cannot serve as a secret.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EnvSecretError {
    #[error("environment variable `{0}` is not set")]
    Missing(String),
    #[error("environment variable `{0}` is empty")]
    Empty(String),
}

/// The secret in `raw`, the value of the variable `name`. Surrounding
/// whitespace is dropped: a line ending from a file would otherwise reach a
/// header or a token comparison.
pub fn secret(name: &str, raw: Option<String>) -> Result<String, EnvSecretError> {
    let raw = raw.ok_or_else(|| EnvSecretError::Missing(name.to_owned()))?;
    match raw.trim() {
        "" => Err(EnvSecretError::Empty(name.to_owned())),
        value => Ok(value.to_owned()),
    }
}

/// Expands every string value in `value`, recursively, except a
/// `credential.command`: that one is expanded when it runs, so a secret it
/// references never sits in the loaded configuration.
pub fn expand_value(
    value: &mut toml::Value,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<(), ConfigError> {
    walk(value, lookup, false)
}

fn walk(
    value: &mut toml::Value,
    lookup: &impl Fn(&str) -> Option<String>,
    in_credential: bool,
) -> Result<(), ConfigError> {
    match value {
        toml::Value::String(text) => {
            *text = expand(text, lookup)?;
        }
        toml::Value::Array(items) => {
            for item in items {
                walk(item, lookup, false)?;
            }
        }
        toml::Value::Table(table) => {
            for (key, item) in table.iter_mut() {
                if in_credential && key == "command" {
                    continue;
                }
                walk(item, lookup, key == "credential")?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Replaces every `${NAME}` with `lookup(NAME)`. `$${NAME}` emits a literal
/// `${NAME}`. A `$` not followed by `{` is copied unchanged.
pub fn expand(text: &str, lookup: &impl Fn(&str) -> Option<String>) -> Result<String, ConfigError> {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find('$') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        if let Some(stripped) = after.strip_prefix("${") {
            // Escaped reference: drop one `$`, keep `${...}` verbatim.
            out.push_str("${");
            rest = stripped;
        } else if let Some(body) = after.strip_prefix('{') {
            let Some(end) = body.find('}') else {
                out.push('$');
                rest = after;
                continue;
            };
            let name = &body[..end];
            let value = lookup(name).ok_or_else(|| ConfigError::MissingEnv(name.to_owned()))?;
            out.push_str(&value);
            rest = &body[end + 1..];
        } else {
            out.push('$');
            rest = after;
        }
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_loses_surrounding_whitespace() {
        assert_eq!(secret("K", Some(" sk-1\r\n".to_owned())).unwrap(), "sk-1");
    }

    #[test]
    fn a_secret_must_exist_and_hold_something() {
        assert_eq!(
            secret("K", None),
            Err(EnvSecretError::Missing("K".to_owned()))
        );
        assert_eq!(
            secret("K", Some(" \n".to_owned())),
            Err(EnvSecretError::Empty("K".to_owned()))
        );
    }
}
