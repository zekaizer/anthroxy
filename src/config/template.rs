//! `{header:<name>}` in a value a backend is sent: read off the client's
//! request each time (ADR-0019).

use http::{HeaderMap, HeaderName};

use crate::text::short;

/// Headers that carry the client's credential. With `server.v1_auth =
/// "token"` that is the router's token, which no backend may see.
const CLIENT_CREDENTIALS: [&str; 3] = ["authorization", "x-api-key", "proxy-authorization"];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TemplateError {
    #[error("`{{header:` is not closed by `}}`")]
    Unterminated,
    /// The name as written, escaped and cut.
    #[error("`{{header:{0}}}` does not name a header")]
    NotAHeaderName(String),
    #[error("`{{header:{0}}}` would send the client's credential to the backend")]
    ClientCredential(String),
}

/// A value with its placeholders located.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template(Vec<Part>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Header(HeaderName),
}

const OPEN: &str = "{header:";

impl Template {
    pub fn parse(text: &str) -> Result<Self, TemplateError> {
        let mut parts = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find(OPEN) {
            let after = &rest[start + OPEN.len()..];
            let end = after.find('}').ok_or(TemplateError::Unterminated)?;
            let written = &after[..end];
            let name = HeaderName::from_bytes(written.as_bytes())
                .map_err(|_| TemplateError::NotAHeaderName(short(written)))?;
            if CLIENT_CREDENTIALS.contains(&name.as_str()) {
                return Err(TemplateError::ClientCredential(name.to_string()));
            }
            if start > 0 {
                parts.push(Part::Text(rest[..start].to_owned()));
            }
            parts.push(Part::Header(name));
            rest = &after[end + 1..];
        }
        if !rest.is_empty() {
            parts.push(Part::Text(rest.to_owned()));
        }
        Ok(Self(parts))
    }

    /// Whether the value depends on the request, and so may be left out.
    pub fn reads_client(&self) -> bool {
        self.0.iter().any(|part| matches!(part, Part::Header(_)))
    }

    /// The value for a request carrying `client`. `None` when a header it
    /// names is absent, empty or not UTF-8: a value filled in part is not
    /// sent.
    pub fn render(&self, client: &HeaderMap) -> Option<String> {
        let mut out = String::new();
        for part in &self.0 {
            match part {
                Part::Text(text) => out.push_str(text),
                Part::Header(name) => {
                    // Not `to_str`, which is ASCII only: a name in another
                    // script is a value like any other.
                    let value = std::str::from_utf8(client.get(name)?.as_bytes()).ok()?;
                    if value.is_empty() {
                        return None;
                    }
                    out.push_str(value);
                }
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;

    use super::*;

    fn client(headers: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(*name, HeaderValue::from_static(value));
        }
        map
    }

    fn render(text: &str, headers: &[(&'static str, &'static str)]) -> Option<String> {
        Template::parse(text).unwrap().render(&client(headers))
    }

    #[test]
    fn text_without_a_placeholder_is_sent_as_written() {
        assert_eq!(
            render("opencode/1.2.3", &[]).as_deref(),
            Some("opencode/1.2.3")
        );
        assert_eq!(render("", &[]).as_deref(), Some(""));
        // Braces that are not the placeholder are text: a chat template or a
        // JSON string holds plenty.
        assert_eq!(
            render("{{ messages }} {\"a\":1} {header}", &[]).as_deref(),
            Some("{{ messages }} {\"a\":1} {header}")
        );
    }

    #[test]
    fn a_placeholder_takes_the_client_header_value() {
        let headers = [("x-claude-code-session-id", "abc-123"), ("x-app", "cli")];
        assert_eq!(
            render("{header:x-claude-code-session-id}", &headers).as_deref(),
            Some("abc-123")
        );
        assert_eq!(
            render(
                "cc-{header:x-claude-code-session-id}/{header:x-app}!",
                &headers
            )
            .as_deref(),
            Some("cc-abc-123/cli!")
        );
        assert_eq!(
            render("{header:X-App}", &headers).as_deref(),
            Some("cli"),
            "header names are case-insensitive"
        );
    }

    #[test]
    fn text_of_any_script_surrounds_a_placeholder() {
        let headers = [("x-session", "abc")];
        assert_eq!(
            render("세션-{header:x-session}-끝 {header:x-session}é", &headers).as_deref(),
            Some("세션-abc-끝 abcé")
        );
        assert_eq!(
            Template::parse("한{header:세션}"),
            Err(TemplateError::NotAHeaderName("세션".into()))
        );
        assert_eq!(
            Template::parse("é{header:x-a}é{header:"),
            Err(TemplateError::Unterminated)
        );
    }

    #[test]
    fn a_repeated_client_header_gives_its_first_value() {
        assert_eq!(
            render("{header:x-app}", &[("x-app", "one"), ("x-app", "two")]).as_deref(),
            Some("one")
        );
    }

    #[test]
    fn a_missing_or_empty_header_leaves_no_value_at_all() {
        assert_eq!(render("cc-{header:x-session}", &[("x-app", "cli")]), None);
        assert_eq!(render("cc-{header:x-session}", &[("x-session", "")]), None);
        assert_eq!(
            render("{header:x-app}{header:x-session}", &[("x-app", "cli")]),
            None,
            "one missing header is enough"
        );
        let mut binary = HeaderMap::new();
        binary.insert("x-session", HeaderValue::from_bytes(b"\xff\xfe").unwrap());
        assert_eq!(
            Template::parse("{header:x-session}")
                .unwrap()
                .render(&binary),
            None
        );
    }

    #[test]
    fn a_header_value_outside_ascii_is_read_as_utf_8() {
        let mut client = HeaderMap::new();
        client.insert(
            "x-user-name",
            HeaderValue::from_bytes("홍길동".as_bytes()).unwrap(),
        );
        let template = Template::parse("user:{header:x-user-name}").unwrap();
        assert_eq!(template.render(&client).as_deref(), Some("user:홍길동"));
        assert!(template.reads_client());
        assert!(!Template::parse("fixed").unwrap().reads_client());
    }

    #[test]
    fn a_malformed_placeholder_is_refused() {
        assert_eq!(
            Template::parse("cc-{header:x-session"),
            Err(TemplateError::Unterminated)
        );
        assert_eq!(
            Template::parse("{header:x session}"),
            Err(TemplateError::NotAHeaderName("x session".into()))
        );
        assert_eq!(
            Template::parse("{header:}"),
            Err(TemplateError::NotAHeaderName(String::new()))
        );
    }

    #[test]
    fn the_client_credential_cannot_be_named() {
        for name in ["authorization", "X-Api-Key", "proxy-authorization"] {
            assert_eq!(
                Template::parse(&format!("{{header:{name}}}")),
                Err(TemplateError::ClientCredential(name.to_ascii_lowercase())),
                "{name}"
            );
        }
    }
}
