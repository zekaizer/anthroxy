//! `set_fields`: fields a backend's request body is given, each a fixed value
//! or one read off the client's request (ADR-0019).

use std::collections::BTreeMap;

use http::HeaderMap;
use serde_json::Value;

use super::{Template, TemplateError};
use crate::body_field::BodyField;

/// Top-level fields the router reads to route a request and relay its answer.
const ROUTER_OWNED: [&str; 2] = ["model", "stream"];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FieldError {
    #[error("the path has an empty segment")]
    EmptySegment,
    #[error("`{0}` is read by the router and cannot be set")]
    RouterOwned(&'static str),
    #[error("a number that is not finite has no JSON form")]
    NotFinite,
    #[error(transparent)]
    Placeholder(#[from] TemplateError),
}

/// A value as the file wrote it, with the placeholders of its strings located.
#[derive(Debug, Clone, PartialEq)]
enum Node {
    Fixed(Value),
    Text(Template),
    Array(Vec<Node>),
    Object(Vec<(String, Node)>),
}

/// A backend's `set_fields`, ready to be read against a request.
#[derive(Debug, Default)]
pub struct SetFields(Vec<(Vec<String>, Node)>);

impl Node {
    fn compile(value: &toml::Value) -> Result<Self, FieldError> {
        Ok(match value {
            toml::Value::String(text) => Node::Text(Template::parse(text)?),
            toml::Value::Integer(number) => Node::Fixed(Value::from(*number)),
            toml::Value::Float(number) => Node::Fixed(
                serde_json::Number::from_f64(*number)
                    .map(Value::Number)
                    .ok_or(FieldError::NotFinite)?,
            ),
            toml::Value::Boolean(flag) => Node::Fixed(Value::Bool(*flag)),
            // JSON has no date; loading the file hands one over as its text
            // already, and this keeps a value built by hand the same.
            toml::Value::Datetime(date) => Node::Fixed(Value::String(date.to_string())),
            toml::Value::Array(items) => {
                Node::Array(items.iter().map(Node::compile).collect::<Result<_, _>>()?)
            }
            toml::Value::Table(table) => Node::Object(
                table
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), Node::compile(value)?)))
                    .collect::<Result<_, FieldError>>()?,
            ),
        })
    }

    /// `None` when a string in it names a header `client` lacks.
    fn render(&self, client: &HeaderMap) -> Option<Value> {
        Some(match self {
            Node::Fixed(value) => value.clone(),
            Node::Text(template) => Value::String(template.render(client)?),
            Node::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| item.render(client))
                    .collect::<Option<_>>()?,
            ),
            Node::Object(entries) => Value::Object(
                entries
                    .iter()
                    .map(|(key, value)| Some((key.clone(), value.render(client)?)))
                    .collect::<Option<_>>()?,
            ),
        })
    }
}

/// Whether `path` and `value` are an entry the router can apply.
pub fn check(path: &str, value: &toml::Value) -> Result<(), FieldError> {
    if path.split('.').any(str::is_empty) {
        return Err(FieldError::EmptySegment);
    }
    let root = path.split('.').next().unwrap_or(path);
    if let Some(owned) = ROUTER_OWNED.iter().find(|owned| **owned == root) {
        return Err(FieldError::RouterOwned(owned));
    }
    Node::compile(value).map(drop)
}

/// Every two of `paths` where the second names a field inside the first:
/// whichever is set last would undo the other.
pub fn overlaps<'a>(paths: impl IntoIterator<Item = &'a String>) -> Vec<(&'a str, &'a str)> {
    let paths: Vec<&str> = paths.into_iter().map(String::as_str).collect();
    let mut found = Vec::new();
    for outer in &paths {
        for inner in &paths {
            if inner
                .strip_prefix(outer)
                .is_some_and(|rest| rest.starts_with('.'))
            {
                found.push((*outer, *inner));
            }
        }
    }
    found
}

impl SetFields {
    /// Assumes every entry of `config` passed [`check`].
    pub fn new(config: &BTreeMap<String, toml::Value>) -> Self {
        Self(
            config
                .iter()
                .map(|(path, value)| {
                    (
                        path.split('.').map(str::to_owned).collect(),
                        Node::compile(value).expect("validated set_fields value"),
                    )
                })
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The configured paths, as written.
    pub fn paths(&self) -> Vec<String> {
        self.0.iter().map(|(path, _)| path.join(".")).collect()
    }

    /// The fields for a request carrying `client`. One whose value names a
    /// header `client` lacks is left out whole.
    pub fn resolve(&self, client: &HeaderMap) -> Vec<BodyField> {
        self.0
            .iter()
            .filter_map(|(path, node)| {
                Some(BodyField {
                    path: path.clone(),
                    value: node.render(client)?,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use http::HeaderValue;
    use serde_json::json;

    use super::*;

    fn fields(text: &str) -> BTreeMap<String, toml::Value> {
        toml::from_str(text).unwrap()
    }

    fn client(headers: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.append(*name, HeaderValue::from_static(value));
        }
        map
    }

    fn resolved(text: &str, headers: &[(&'static str, &'static str)]) -> Vec<(String, Value)> {
        SetFields::new(&fields(text))
            .resolve(&client(headers))
            .into_iter()
            .map(|field| (field.path.join("."), field.value))
            .collect()
    }

    #[test]
    fn fixed_values_keep_their_type() {
        assert_eq!(
            resolved(
                r#"
"extraData.clientVersion" = "1.2.3"
"chat_template_kwargs.enable_thinking" = false
top_k = 40
"penalty" = 1.5
stop_token_ids = [1, 2]
extra = { a = "x", b = [{ c = 1 }] }
since = 1979-05-27T07:32:00Z
"#,
                &[]
            ),
            [
                (
                    "chat_template_kwargs.enable_thinking".to_owned(),
                    json!(false)
                ),
                ("extra".to_owned(), json!({"a": "x", "b": [{"c": 1}]})),
                ("extraData.clientVersion".to_owned(), json!("1.2.3")),
                ("penalty".to_owned(), json!(1.5)),
                ("since".to_owned(), json!("1979-05-27T07:32:00Z")),
                ("stop_token_ids".to_owned(), json!([1, 2])),
                ("top_k".to_owned(), json!(40)),
            ]
        );
    }

    #[test]
    fn a_string_at_any_depth_reads_the_client_request() {
        let text = r#"
"extraData.sessionId" = "cc-{header:x-claude-code-session-id}"
tags = ["fixed", "{header:x-app}"]
meta = { agent = "{header:user-agent}" }
"#;
        assert_eq!(
            resolved(
                text,
                &[
                    ("x-claude-code-session-id", "abc"),
                    ("x-app", "cli"),
                    ("user-agent", "claude-cli/2.0")
                ]
            ),
            [
                ("extraData.sessionId".to_owned(), json!("cc-abc")),
                ("meta".to_owned(), json!({"agent": "claude-cli/2.0"})),
                ("tags".to_owned(), json!(["fixed", "cli"])),
            ]
        );
    }

    #[test]
    fn a_field_with_nothing_to_read_is_left_out_whole() {
        let text = r#"
"extraData.sessionId" = "{header:x-claude-code-session-id}"
"extraData.clientVersion" = "1.2.3"
tags = ["fixed", "{header:x-app}"]
"#;
        assert_eq!(
            resolved(text, &[]),
            [("extraData.clientVersion".to_owned(), json!("1.2.3"))]
        );
    }

    #[test]
    fn the_paths_are_listed_as_written() {
        let set = SetFields::new(&fields("\"a.b\" = 1\nc = 2\n"));
        assert_eq!(set.paths(), ["a.b", "c"]);
        assert!(!set.is_empty());
        assert!(SetFields::default().is_empty());
    }

    #[test]
    fn an_entry_the_router_cannot_apply_is_refused() {
        let value = |text: &str| fields(&format!("v = {text}"))["v"].clone();
        let ok = value("1");
        assert_eq!(check("a..b", &ok), Err(FieldError::EmptySegment));
        assert_eq!(check("", &ok), Err(FieldError::EmptySegment));
        assert_eq!(check("model", &ok), Err(FieldError::RouterOwned("model")));
        assert_eq!(
            check("stream.x", &ok),
            Err(FieldError::RouterOwned("stream"))
        );
        assert_eq!(check("metadata.model", &ok), Ok(()));
        assert_eq!(check("a", &value("[1.0, nan]")), Err(FieldError::NotFinite));
        assert_eq!(
            check("a", &value("{ b = inf }")),
            Err(FieldError::NotFinite)
        );
        assert_eq!(
            check("a", &value("{ b = [\"{header:authorization}\"] }")),
            Err(FieldError::Placeholder(TemplateError::ClientCredential(
                "authorization".into()
            )))
        );
    }

    #[test]
    fn one_path_inside_another_is_an_overlap() {
        let paths = |list: &[&str]| -> Vec<String> { list.iter().map(|p| p.to_string()).collect() };
        assert_eq!(overlaps(&paths(&["a.b", "a-b", "a.c", "ab"])), []);
        assert_eq!(overlaps(&paths(&["a.b.c", "x", "a.b"])), [("a.b", "a.b.c")]);
        assert_eq!(
            overlaps(&paths(&["a", "a-b", "a.b", "a.b.c"])),
            [("a", "a.b"), ("a", "a.b.c"), ("a.b", "a.b.c")]
        );
    }
}
