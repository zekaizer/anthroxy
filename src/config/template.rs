//! `{header:<name>}` in a value a backend is sent: read off the client's
//! request each time (ADR-0019). A preset's values also take `{<variable>}`,
//! replaced once when the preset is bound (ADR-0022).

use std::collections::BTreeMap;

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

/// A value as a preset writes it: literal text, `{header:<name>}` and
/// `{<variable>}`. Only a preset has this form; a backend's own values go
/// through [`Template::parse`], where `{name}` stays literal text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetValue(Vec<PresetPart>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum PresetPart {
    Text(String),
    Header(HeaderName),
    /// Written `{name}`; a name passes [`is_variable_name`].
    Variable(String),
}

/// Letters, digits and `_`, at least one.
pub fn is_variable_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

impl PresetValue {
    /// `{header:` placeholders under [`Template::parse`]'s rules; in the text
    /// between them every `{name}` is a variable, and any other brace is
    /// text.
    pub fn parse(text: &str) -> Result<Self, TemplateError> {
        let mut parts = Vec::new();
        for part in Template::parse(text)?.0 {
            match part {
                Part::Header(name) => parts.push(PresetPart::Header(name)),
                Part::Text(text) => variables_in(&text, &mut parts),
            }
        }
        Ok(Self(parts))
    }

    /// Every variable `text` names, once for each time it does, whether or
    /// not [`Self::parse`] takes `text`: for a value it takes, these are its
    /// [`Self::variables`].
    pub fn named_in(text: &str) -> Vec<String> {
        let mut parts = Vec::new();
        variables_in(text, &mut parts);
        parts
            .into_iter()
            .filter_map(|part| match part {
                PresetPart::Variable(name) => Some(name),
                _ => None,
            })
            .collect()
    }

    /// Whether the value depends on the request, and so may be left out.
    pub fn reads_client(&self) -> bool {
        self.0
            .iter()
            .any(|part| matches!(part, PresetPart::Header(_)))
    }

    /// Every variable the value names, once for each time it does.
    pub fn variables(&self) -> impl Iterator<Item = &str> {
        self.0.iter().filter_map(|part| match part {
            PresetPart::Variable(name) => Some(name.as_str()),
            _ => None,
        })
    }

    /// The value with every variable replaced by its value in `values`, as
    /// text: what a variable holds is never read for placeholders. Assumes
    /// `values` has every one of [`Self::variables`].
    pub fn bind(&self, values: &BTreeMap<String, String>) -> Template {
        fn text(parts: &mut Vec<Part>, more: &str) {
            match parts.last_mut() {
                Some(Part::Text(text)) => text.push_str(more),
                _ if more.is_empty() => {}
                _ => parts.push(Part::Text(more.to_owned())),
            }
        }
        let mut parts = Vec::new();
        for part in &self.0 {
            match part {
                PresetPart::Text(literal) => text(&mut parts, literal),
                PresetPart::Variable(name) => text(&mut parts, &values[name]),
                PresetPart::Header(name) => parts.push(Part::Header(name.clone())),
            }
        }
        Template(parts)
    }
}

/// `text` cut at its `{name}` variables.
fn variables_in(text: &str, parts: &mut Vec<PresetPart>) {
    let mut literal = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let name = after.find('}').map(|close| &after[..close]);
        match name.filter(|name| is_variable_name(name)) {
            Some(name) => {
                literal.push_str(&rest[..open]);
                if !literal.is_empty() {
                    parts.push(PresetPart::Text(std::mem::take(&mut literal)));
                }
                parts.push(PresetPart::Variable(name.to_owned()));
                rest = &after[name.len() + 1..];
            }
            None => {
                literal.push_str(&rest[..=open]);
                rest = after;
            }
        }
    }
    literal.push_str(rest);
    if !literal.is_empty() {
        parts.push(PresetPart::Text(literal));
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

    fn bound(text: &str, values: &[(&str, &str)]) -> Template {
        let values = values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        PresetValue::parse(text).unwrap().bind(&values)
    }

    #[test]
    fn a_preset_value_takes_variables_beside_client_headers() {
        let value =
            PresetValue::parse("opencode/{version} ({os}; {header:x-app}) {version}").unwrap();
        assert_eq!(
            value.variables().collect::<Vec<_>>(),
            ["version", "os", "version"]
        );
        assert!(value.reads_client());
        let template = value.bind(
            &[("version", "1.19.6"), ("os", "linux")]
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .into(),
        );
        assert_eq!(
            template.render(&client(&[("x-app", "cli")])).as_deref(),
            Some("opencode/1.19.6 (linux; cli) 1.19.6")
        );
        assert!(!PresetValue::parse("{version}").unwrap().reads_client());
    }

    #[test]
    fn the_variables_of_a_text_are_those_of_the_value_it_parses_to() {
        for text in [
            "opencode/{version} ({os}; {header:x-app}) {version}",
            "{a}{header:x-b}{{c}} {d",
            "{header:x-a}",
            "",
        ] {
            let value = PresetValue::parse(text).unwrap();
            assert_eq!(
                PresetValue::named_in(text),
                value.variables().collect::<Vec<_>>(),
                "{text}"
            );
        }
        assert_eq!(
            PresetValue::named_in("{nope} {header:x session} {v}"),
            ["nope", "v"],
            "a placeholder that names no header hides none"
        );
        assert_eq!(PresetValue::named_in("{header:{v}"), ["v"]);
    }

    #[test]
    fn a_bound_value_is_the_template_its_text_would_parse_to() {
        assert_eq!(
            bound("a-{v}-{header:x-app}{v}", &[("v", "1")]),
            Template::parse("a-1-{header:x-app}1").unwrap()
        );
        assert_eq!(bound("{v}", &[("v", "")]), Template::parse("").unwrap());
    }

    #[test]
    fn what_a_variable_holds_is_text() {
        let template = bound("{v}", &[("v", "{header:x-app} {v}")]);
        assert!(!template.reads_client());
        assert_eq!(
            template.render(&client(&[("x-app", "cli")])).as_deref(),
            Some("{header:x-app} {v}")
        );
    }

    #[test]
    fn a_brace_that_is_not_a_name_is_text_in_a_preset_value_too() {
        for text in [
            "{{ messages }}",
            "{\"a\":1}",
            "{}",
            "{a-b}",
            "{a b}",
            "{open",
            "close}",
            "{한}",
        ] {
            let value = PresetValue::parse(text).unwrap();
            assert_eq!(value.variables().count(), 0, "{text}");
            assert_eq!(
                value
                    .bind(&BTreeMap::new())
                    .render(&HeaderMap::new())
                    .as_deref(),
                Some(text)
            );
        }
        let value = PresetValue::parse("{{v}} {a{v}").unwrap();
        assert_eq!(value.variables().collect::<Vec<_>>(), ["v", "v"]);
        assert_eq!(
            PresetValue::parse("{v}{header:x session}"),
            Err(TemplateError::NotAHeaderName("x session".into()))
        );
    }
}
