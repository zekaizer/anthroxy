//! Configuration hints read out of an upstream error body: the fixes an
//! operator would otherwise find by reading the message and the README.

use serde::Serialize;

use crate::config::{BackendKind, snippet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hint {
    pub summary: String,
    /// Text to paste: a configuration fragment or an environment variable.
    pub snippet: Option<String>,
}

/// An upstream error response and the route that produced it.
#[derive(Debug, Clone, Copy)]
pub struct UpstreamFailure<'a> {
    pub backend: &'a str,
    pub kind: BackendKind,
    pub upstream_model: &'a str,
    /// The backend's configured `drop_fields`.
    pub drop_fields: &'a [String],
    pub status: u16,
    pub body: &'a str,
}

pub fn hints(failure: &UpstreamFailure<'_>) -> Vec<Hint> {
    let mut hints = Vec::new();
    // An `openai` backend receives the router's translation, not the client's
    // fields, so dropping a field of the client request would not reach it.
    if failure.kind == BackendKind::Anthropic {
        hints.extend(drop_fields_hint(failure));
    }
    if let Some(window) = context_window(failure.body) {
        hints.push(Hint {
            summary: "the conversation outgrew the model's context window; Claude Code assumes 200k tokens for a model it does not know, so set CLAUDE_CODE_MAX_CONTEXT_TOKENS in its environment to the real window".to_owned(),
            snippet: window.map(|tokens| format!("CLAUDE_CODE_MAX_CONTEXT_TOKENS={tokens}")),
        });
    }
    let lower = failure.body.to_ascii_lowercase();
    if failure.status == 404
        && lower.contains("model")
        && (lower.contains("does not exist") || lower.contains("not found"))
    {
        hints.push(Hint {
            summary: format!(
                "backend `{}` does not know the model `{}`; compare upstream_model with the backend's model list (Probe, or `anthroxy check`)",
                failure.backend, failure.upstream_model
            ),
            snippet: None,
        });
    }
    if matches!(failure.status, 401 | 403) {
        hints.push(Hint {
            summary: format!(
                "backend `{}` rejected the credential; `anthroxy credential {}` shows what its source produces",
                failure.backend, failure.backend
            ),
            snippet: None,
        });
    }
    hints
}

fn drop_fields_hint(failure: &UpstreamFailure<'_>) -> Option<Hint> {
    let mut fields: Vec<String> = Vec::new();
    for field in rejected_fields(failure.body) {
        if field != "model" && !failure.drop_fields.contains(&field) && !fields.contains(&field) {
            fields.push(field);
        }
    }
    if fields.is_empty() {
        return None;
    }
    let named: Vec<String> = fields.iter().map(|f| format!("`{f}`")).collect();
    let mut all = failure.drop_fields.to_vec();
    all.extend(fields);
    Some(Hint {
        summary: format!(
            "backend `{}` rejects the request field(s) {}; drop them before forwarding",
            failure.backend,
            named.join(", ")
        ),
        snippet: Some(format!(
            "[backends.{}]\ndrop_fields = {}",
            snippet::key(failure.backend),
            snippet::string_array(&all)
        )),
    })
}

/// Dot-separated request fields an error body says the server does not
/// accept, in the order they appear. A path through an array is left out:
/// `drop_fields` cannot reach it.
fn rejected_fields(body: &str) -> Vec<String> {
    let text = body.replace("\\\"", "\"");
    let mut fields = Vec::new();
    // pydantic (vLLM and others): `extra_forbidden` with `loc` ("body", ...).
    for entry in text.split("extra_forbidden").skip(1) {
        if let Some(path) = body_loc(entry) {
            fields.push(path);
        }
    }
    // Anthropic: `<path>: Extra inputs are not permitted`.
    let marker = ": Extra inputs are not permitted";
    let mut rest = text.as_str();
    while let Some(at) = rest.find(marker) {
        let before = &rest[..at];
        let start = before
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
            .map_or(0, |i| i + 1);
        fields.extend(field_path(&before[start..]));
        rest = &rest[at + marker.len()..];
    }
    // OpenAI: `Unrecognized request argument supplied: a, b`.
    if let Some(at) = text.find("Unrecognized request argument supplied: ") {
        let list = &text[at + "Unrecognized request argument supplied: ".len()..];
        let end = list
            .find(|c: char| {
                !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == ',' || c == ' ')
            })
            .unwrap_or(list.len());
        fields.extend(list[..end].split(',').filter_map(|f| field_path(f.trim())));
    }
    // OpenAI: `Unknown parameter: 'name'`; serde: ``unknown field `name` ``.
    for (open, close) in [("Unknown parameter: '", '\''), ("unknown field `", '`')] {
        let mut rest = text.as_str();
        while let Some(at) = rest.find(open) {
            let tail = &rest[at + open.len()..];
            if let Some(end) = tail.find(close) {
                fields.extend(field_path(&tail[..end]));
            }
            rest = tail;
        }
    }
    fields
}

/// The path after `'body'` in the first `loc` of `entry`, quoted segments
/// only.
fn body_loc(entry: &str) -> Option<String> {
    let (quote, at) = ['\'', '"']
        .into_iter()
        .filter_map(|q| entry.find(&format!("{q}body{q}")).map(|at| (q, at)))
        .min_by_key(|(_, at)| *at)?;
    let mut rest = &entry[at + "body".len() + 2..];
    let mut segments = Vec::new();
    loop {
        rest = rest.trim_start_matches([' ', ',']);
        match rest.chars().next()? {
            ')' | ']' => break,
            c if c == quote => {
                let end = rest[1..].find(quote)?;
                segments.push(&rest[1..=end]);
                rest = &rest[end + 2..];
            }
            _ => return None,
        }
    }
    field_path(&segments.join("."))
}

/// `path` when every segment is a plain key, not an array index.
fn field_path(path: &str) -> Option<String> {
    let valid = !path.is_empty()
        && path.split('.').all(|segment| {
            !segment.is_empty()
                && !segment.chars().all(|c| c.is_ascii_digit())
                && segment
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        });
    valid.then(|| path.to_owned())
}

/// `Some(Some(tokens))` when the body names the window, `Some(None)` when it
/// only says the window was exceeded.
fn context_window(body: &str) -> Option<Option<u64>> {
    let marker = "maximum context length is ";
    if let Some(at) = body.find(marker) {
        let digits: String = body[at + marker.len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        return Some(digits.parse().ok());
    }
    let lower = body.to_ascii_lowercase();
    ["context size has been exceeded", "context_length_exceeded"]
        .iter()
        .any(|m| lower.contains(m))
        .then_some(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failure<'a>(status: u16, body: &'a str, drop_fields: &'a [String]) -> UpstreamFailure<'a> {
        UpstreamFailure {
            backend: "vllm",
            kind: BackendKind::Anthropic,
            upstream_model: "Qwen/Qwen3.5-32B",
            drop_fields,
            status,
            body,
        }
    }

    fn snippets(hints: &[Hint]) -> Vec<String> {
        hints.iter().filter_map(|h| h.snippet.clone()).collect()
    }

    #[test]
    fn a_vllm_extra_forbidden_field_becomes_drop_fields_merged_with_the_existing_ones() {
        let body = r#"{"object":"error","message":"[{'type': 'extra_forbidden', 'loc': ('body', 'context_management'), 'msg': 'Extra inputs are not permitted', 'input': {'edits': []}}, {'type': 'extra_forbidden', 'loc': ('body', 'metadata', 'trace'), 'msg': 'Extra inputs are not permitted'}]","type":"BadRequestError","param":null,"code":400}"#;
        let existing = vec!["metadata.user_id".to_owned()];
        let hints = hints(&failure(400, body, &existing));
        assert_eq!(hints.len(), 1, "{hints:?}");
        assert!(hints[0].summary.contains("context_management"), "{hints:?}");
        assert_eq!(
            snippets(&hints),
            [r#"[backends.vllm]
drop_fields = ["metadata.user_id", "context_management", "metadata.trace"]"#]
        );
    }

    #[test]
    fn other_servers_name_the_field_their_own_way() {
        for body in [
            r#"{"detail":[{"type":"extra_forbidden","loc":["body","context_management"],"msg":"Extra inputs are not permitted"}]}"#,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"context_management: Extra inputs are not permitted"}}"#,
            r#"{"error":{"message":"Unrecognized request argument supplied: context_management"}}"#,
            r#"{"error":{"message":"Unknown parameter: 'context_management'."}}"#,
            "unknown field `context_management`, expected one of `model`, `messages`",
        ] {
            let hints = hints(&failure(400, body, &[]));
            assert_eq!(
                snippets(&hints),
                ["[backends.vllm]\ndrop_fields = [\"context_management\"]"],
                "{body}"
            );
        }
    }

    #[test]
    fn no_drop_fields_hint_for_the_model_field_an_existing_entry_or_an_openai_backend() {
        let body = r#"{"detail":[{"type":"extra_forbidden","loc":["body","model"],"msg":"Extra inputs are not permitted"}]}"#;
        assert!(hints(&failure(400, body, &[])).is_empty());

        let body = "context_management: Extra inputs are not permitted";
        let existing = vec!["context_management".to_owned()];
        assert!(hints(&failure(400, body, &existing)).is_empty());

        let mut openai = failure(400, body, &[]);
        openai.kind = BackendKind::OpenAi;
        assert!(
            hints(&openai).is_empty(),
            "the router wrote that body, not Claude Code"
        );
    }

    #[test]
    fn a_context_window_error_suggests_the_client_limit() {
        let body = r#"{"object":"error","message":"This model's maximum context length is 32768 tokens. However, you requested 40961 tokens (40000 in the messages, 961 in the completion).","code":400}"#;
        let hints = hints(&failure(400, body, &[]));
        assert_eq!(hints.len(), 1, "{hints:?}");
        assert!(hints[0].summary.contains("CLAUDE_CODE_MAX_CONTEXT_TOKENS"));
        assert_eq!(snippets(&hints), ["CLAUDE_CODE_MAX_CONTEXT_TOKENS=32768"]);

        let lm_studio = r#"{"error":"Context size has been exceeded."}"#;
        let hints = super::hints(&failure(400, lm_studio, &[]));
        assert_eq!(hints.len(), 1, "{hints:?}");
        assert_eq!(hints[0].snippet, None, "no number to suggest");
    }

    #[test]
    fn an_unknown_upstream_model_and_a_rejected_credential() {
        let body = r#"{"object":"error","message":"The model `Qwen/Qwen3.5-32B` does not exist.","type":"NotFoundError","code":404}"#;
        let hints = hints(&failure(404, body, &[]));
        assert_eq!(hints.len(), 1, "{hints:?}");
        assert!(hints[0].summary.contains("Qwen/Qwen3.5-32B"), "{hints:?}");

        let hints = super::hints(&failure(401, r#"{"error":"invalid api key"}"#, &[]));
        assert_eq!(hints.len(), 1, "{hints:?}");
        assert!(
            hints[0].summary.contains("anthroxy credential vllm"),
            "{hints:?}"
        );
    }

    #[test]
    fn an_ordinary_failure_has_no_hint() {
        assert!(hints(&failure(500, "internal error", &[])).is_empty());
        assert!(hints(&failure(429, r#"{"error":"rate limited"}"#, &[])).is_empty());
    }
}
