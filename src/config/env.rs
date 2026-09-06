//! `${NAME}` expansion over string values of a parsed TOML document. Keys and
//! comments are never touched.

use super::ConfigError;

/// Expands every string value in `value`, recursively.
pub fn expand_value(
    value: &mut toml::Value,
    lookup: &impl Fn(&str) -> Option<String>,
) -> Result<(), ConfigError> {
    match value {
        toml::Value::String(text) => {
            *text = expand(text, lookup)?;
        }
        toml::Value::Array(items) => {
            for item in items {
                expand_value(item, lookup)?;
            }
        }
        toml::Value::Table(table) => {
            for (_, item) in table.iter_mut() {
                expand_value(item, lookup)?;
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
