//! `${NAME}` expansion over the raw configuration text.

use super::ConfigError;

/// Replaces every `${NAME}` with `lookup(NAME)`. `$${NAME}` emits a literal
/// `${NAME}`. A `$` not followed by `{` is copied unchanged.
pub fn expand(text: &str, lookup: impl Fn(&str) -> Option<String>) -> Result<String, ConfigError> {
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
