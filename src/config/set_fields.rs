//! `set_fields`: fields a backend's request body is given, each a fixed value
//! or one read off the client's request (ADR-0019). A preset's take
//! `{<variable>}` too, until the preset is bound (ADR-0022).

use std::collections::BTreeMap;

use http::HeaderMap;
use serde_json::Value;

use super::template::PresetValue;
use super::{Template, TemplateError};
use crate::body_field::BodyField;
use crate::text::short;

/// Top-level fields the router reads to route a request and relay its answer.
const ROUTER_OWNED: [&str; 2] = ["model", "stream"];

/// Top-level fields a preset leaves as they are: the router's two and the
/// conversation.
const PRESET_KEPT: [&str; 3] = ["model", "stream", "messages"];

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FieldError {
    #[error("the path has an empty segment")]
    EmptySegment,
    #[error("`{0}` is read by the router and cannot be set")]
    RouterOwned(&'static str),
    #[error("`{0}` is not a preset's to change")]
    PresetKept(&'static str),
    #[error("a number that is not finite has no JSON form")]
    NotFinite,
    #[error(transparent)]
    Placeholder(#[from] TemplateError),
}

/// A value as the file wrote it, with the placeholders of its strings located
/// as an `S`.
#[derive(Debug, Clone, PartialEq)]
enum Node<S> {
    Fixed(Value),
    Text(S),
    Array(Vec<Node<S>>),
    Object(Vec<(String, Node<S>)>),
}

/// A backend's `set_fields`, or a preset's once bound, ready to be read
/// against a request.
#[derive(Debug, Default)]
pub struct SetFields(Vec<(Vec<String>, Node<Template>)>);

/// A preset's `set_fields`, its variables not yet bound.
#[derive(Debug, Default)]
pub struct PresetFields(Vec<(Vec<String>, Node<PresetValue>)>);

impl<S> Node<S> {
    fn compile(
        value: &toml::Value,
        parse: fn(&str) -> Result<S, TemplateError>,
    ) -> Result<Self, FieldError> {
        Ok(match value {
            toml::Value::String(text) => Node::Text(parse(text)?),
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
            toml::Value::Array(items) => Node::Array(
                items
                    .iter()
                    .map(|item| Node::compile(item, parse))
                    .collect::<Result<_, _>>()?,
            ),
            toml::Value::Table(table) => Node::Object(
                table
                    .iter()
                    .map(|(key, value)| Ok((key.clone(), Node::compile(value, parse)?)))
                    .collect::<Result<_, FieldError>>()?,
            ),
        })
    }

    /// Every string in the value, at any depth.
    fn texts<'a>(&'a self, out: &mut Vec<&'a S>) {
        match self {
            Node::Fixed(_) => {}
            Node::Text(text) => out.push(text),
            Node::Array(items) => items.iter().for_each(|item| item.texts(out)),
            Node::Object(entries) => entries.iter().for_each(|(_, value)| value.texts(out)),
        }
    }

    /// The value with each string turned by `turn`.
    fn map<T>(&self, turn: &impl Fn(&S) -> T) -> Node<T> {
        match self {
            Node::Fixed(value) => Node::Fixed(value.clone()),
            Node::Text(text) => Node::Text(turn(text)),
            Node::Array(items) => Node::Array(items.iter().map(|item| item.map(turn)).collect()),
            Node::Object(entries) => Node::Object(
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), value.map(turn)))
                    .collect(),
            ),
        }
    }
}

impl Node<Template> {
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

/// Every field `config` sets, by path. A table is the paths of its entries:
/// TOML reads an unquoted `a.b = 1` as a table inside `a`, and that, `a = { b
/// = 1 }` and `"a.b" = 1` have to set the same one field. An empty table and
/// whatever is inside an array are values.
pub fn leaves(config: &BTreeMap<String, toml::Value>) -> Vec<(String, &toml::Value)> {
    fn walk<'a>(path: String, value: &'a toml::Value, out: &mut Vec<(String, &'a toml::Value)>) {
        match value {
            toml::Value::Table(table) if !table.is_empty() => {
                for (key, value) in table {
                    walk(format!("{path}.{key}"), value, out);
                }
            }
            _ => out.push((path, value)),
        }
    }
    let mut out = Vec::new();
    for (key, value) in config {
        walk(key.clone(), value, &mut out);
    }
    out
}

/// Whether `path` and `value`, one of [`leaves`], are a field the router can
/// set.
pub fn check(path: &str, value: &toml::Value) -> Result<(), FieldError> {
    if let Some(owned) = kept_root(path, &ROUTER_OWNED)? {
        return Err(FieldError::RouterOwned(owned));
    }
    Node::compile(value, Template::parse).map(drop)
}

/// Whether a preset may change the field at the dotted `path`.
fn check_preset_path(path: &str) -> Result<(), FieldError> {
    match kept_root(path, &PRESET_KEPT)? {
        Some(kept) => Err(FieldError::PresetKept(kept)),
        None => Ok(()),
    }
}

/// The one of `kept` the dotted `path` starts at.
fn kept_root(
    path: &str,
    kept: &'static [&'static str],
) -> Result<Option<&'static str>, FieldError> {
    if path.split('.').any(str::is_empty) {
        return Err(FieldError::EmptySegment);
    }
    let root = path.split('.').next().unwrap_or(path);
    Ok(kept.iter().copied().find(|kept| *kept == root))
}

/// Every two of `paths` where the second names the first's field or one
/// inside it: whichever is set last would undo the other.
pub fn overlaps<'a>(paths: impl IntoIterator<Item = &'a String>) -> Vec<(&'a str, &'a str)> {
    let paths: Vec<&str> = paths.into_iter().map(String::as_str).collect();
    let mut found = Vec::new();
    for (at, outer) in paths.iter().enumerate() {
        for (other, inner) in paths.iter().enumerate() {
            let same = inner == outer && at < other;
            let inside = inner
                .strip_prefix(outer)
                .is_some_and(|rest| rest.starts_with('.'));
            if same || inside {
                found.push((*outer, *inner));
            }
        }
    }
    found
}

impl SetFields {
    /// Assumes every one of `config`'s [`leaves`] passed [`check`].
    pub fn new(config: &BTreeMap<String, toml::Value>) -> Self {
        Self(
            leaves(config)
                .into_iter()
                .map(|(path, value)| {
                    (
                        segments(&path),
                        Node::compile(value, Template::parse).expect("validated set_fields value"),
                    )
                })
                .collect(),
        )
    }

    /// These fields, then `later`'s: set in that order, so `later`'s value
    /// stands where both name a path.
    pub fn then(mut self, later: SetFields) -> SetFields {
        self.0.extend(later.0);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The path of every field set, each once.
    pub fn paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = Vec::new();
        for (path, _) in &self.0 {
            let path = path.join(".");
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
        paths
    }

    /// The fields for a request carrying `client`, in the order they are
    /// set. One whose value names a header `client` lacks is left out whole.
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

impl PresetFields {
    /// The fields `config` sets that a preset can set, and every problem of
    /// the rest: a path a preset cannot set, a value with no JSON form or a
    /// bad placeholder, two paths that set one field.
    pub fn compile(config: &BTreeMap<String, toml::Value>) -> (Self, Vec<String>) {
        let mut problems = Vec::new();
        let mut fields = Vec::new();
        let leaves = leaves(config);
        for (path, value) in &leaves {
            let node =
                check_preset_path(path).and_then(|()| Node::compile(value, PresetValue::parse));
            match node {
                Ok(node) => fields.push((segments(path), node)),
                Err(problem) => problems.push(format!("`{}`: {problem}", short(path))),
            }
        }
        for (outer, inner) in overlaps(leaves.iter().map(|(path, _)| path)) {
            problems.push(format!(
                "`{}` and `{}` set the same field",
                short(outer),
                short(inner)
            ));
        }
        (Self(fields), problems)
    }

    /// Each field's path with the variables its strings name.
    pub fn variables(&self) -> impl Iterator<Item = (String, Vec<&str>)> {
        self.0.iter().map(|(path, node)| {
            let mut texts = Vec::new();
            node.texts(&mut texts);
            let names = texts.iter().flat_map(|text| text.variables()).collect();
            (path.join("."), names)
        })
    }

    /// The fields with every variable replaced by its value in `values`.
    /// Assumes `values` has every one of [`Self::variables`].
    pub fn bind(&self, values: &BTreeMap<String, String>) -> SetFields {
        SetFields(
            self.0
                .iter()
                .map(|(path, node)| (path.clone(), node.map(&|text| text.bind(values))))
                .collect(),
        )
    }
}

fn segments(path: &str) -> Vec<String> {
    path.split('.').map(str::to_owned).collect()
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
                ("extra.a".to_owned(), json!("x")),
                ("extra.b".to_owned(), json!([{"c": 1}])),
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
                ("meta.agent".to_owned(), json!("claude-cli/2.0")),
                ("tags".to_owned(), json!(["fixed", "cli"])),
            ]
        );
    }

    #[test]
    fn a_table_is_the_paths_of_its_entries() {
        // TOML reads an unquoted `metadata.user_id` as a table inside
        // `metadata`; the three spellings must set the same one field.
        for text in [
            "metadata.user_id = \"u\"",
            "metadata = { user_id = \"u\" }",
            "\"metadata.user_id\" = \"u\"",
            "[metadata]\nuser_id = \"u\"",
        ] {
            assert_eq!(
                resolved(text, &[]),
                [("metadata.user_id".to_owned(), json!("u"))],
                "{text}"
            );
        }
        assert_eq!(
            resolved("a = { b = { c = 1, d = {} }, e = [{ f = {} }] }", &[]),
            [
                ("a.b.c".to_owned(), json!(1)),
                ("a.b.d".to_owned(), json!({})),
                ("a.e".to_owned(), json!([{"f": {}}])),
            ],
            "an empty table and a table inside an array are values"
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
        assert_eq!(
            overlaps(&paths(&["a.b", "x", "a.b"])),
            [("a.b", "a.b")],
            "two spellings of one path"
        );
    }

    fn bound(text: &str, values: &[(&str, &str)]) -> SetFields {
        let values = values
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        PresetFields::compile(&fields(text)).0.bind(&values)
    }

    fn set(fields: &SetFields, headers: &[(&'static str, &'static str)]) -> Vec<(String, Value)> {
        fields
            .resolve(&client(headers))
            .into_iter()
            .map(|field| (field.path.join("."), field.value))
            .collect()
    }

    #[test]
    fn a_preset_field_takes_a_variable_at_any_depth() {
        let text = r#"
"extra.agent" = "c/{version} {header:x-app}"
tags = ["{version}", { os = "{os}" }, 3]
top_k = 40
"#;
        let (fields, problems) = PresetFields::compile(&fields(text));
        assert_eq!(problems, [] as [String; 0]);
        assert_eq!(
            fields.variables().collect::<Vec<_>>(),
            [
                ("extra.agent".to_owned(), vec!["version"]),
                ("tags".to_owned(), vec!["version", "os"]),
                ("top_k".to_owned(), vec![]),
            ]
        );
        let bound = bound(text, &[("version", "{header:x-app}"), ("os", "linux")]);
        assert_eq!(
            set(&bound, &[("x-app", "cli")]),
            [
                ("extra.agent".to_owned(), json!("c/{header:x-app} cli")),
                (
                    "tags".to_owned(),
                    json!(["{header:x-app}", {"os": "linux"}, 3])
                ),
                ("top_k".to_owned(), json!(40)),
            ],
            "what a variable holds is text"
        );
    }

    #[test]
    fn fields_set_later_come_after_and_are_listed_once() {
        let preset = bound("user = \"preset\"\ntop_k = 40\n", &[]);
        let backend = SetFields::new(&fields("user = \"backend\"\nseed = 7\n"));
        let both = preset.then(backend);
        assert_eq!(
            set(&both, &[]),
            [
                ("top_k".to_owned(), json!(40)),
                ("user".to_owned(), json!("preset")),
                ("seed".to_owned(), json!(7)),
                ("user".to_owned(), json!("backend")),
            ]
        );
        assert_eq!(both.paths(), ["top_k", "user", "seed"]);
    }
}
