//! What a backend's configuration sets in or removes from the request body it
//! receives (`set_fields`, ADR-0019; `drop_fields`), in whichever format that
//! body is, and what a preset does to a translated one (ADR-0022).

use serde_json::{Map, Value};

/// One field with its value for this request.
#[derive(Debug, Clone, PartialEq)]
pub struct BodyField {
    /// Object keys from the root down to the field; never empty.
    pub path: Vec<String>,
    pub value: Value,
}

/// Sets every one of `fields` in `object`, replacing what is there. A step of
/// a path that is missing or is not an object becomes one.
pub fn set_all(object: &mut Map<String, Value>, fields: &[BodyField]) {
    for field in fields {
        set(object, &field.path, &field.value);
    }
}

fn set(object: &mut Map<String, Value>, path: &[String], value: &Value) {
    let Some((key, rest)) = path.split_first() else {
        return;
    };
    if rest.is_empty() {
        object.insert(key.clone(), value.clone());
        return;
    }
    let step = object
        .entry(key.clone())
        .or_insert_with(|| Value::Object(Map::new()));
    if !step.is_object() {
        *step = Value::Object(Map::new());
    }
    if let Value::Object(inner) = step {
        set(inner, rest, value);
    }
}

/// Whether `path` named a field, which is now gone. A path is dot-separated
/// object keys; one whose prefix is missing or not an object removes nothing.
/// The fields left keep their order.
pub fn remove(object: &mut Map<String, Value>, path: &str) -> bool {
    match path.split_once('.') {
        None => object.shift_remove(path).is_some(),
        Some((head, rest)) => match object.get_mut(head) {
            Some(Value::Object(inner)) => remove(inner, rest),
            _ => false,
        },
    }
}

/// What the configuration does to a body once it is written, in the order
/// it is done: `omit` removed, `set` set in order, then the `order` fields
/// put first.
#[derive(Debug, Clone, Copy, Default)]
pub struct BodyEdits<'a> {
    /// Dot-separated paths, as [`remove`] reads them.
    pub omit: &'a [String],
    pub set: &'a [BodyField],
    /// Top-level field names.
    pub order: &'a [String],
}

impl BodyEdits<'_> {
    pub fn apply(&self, object: &mut Map<String, Value>) {
        for path in self.omit {
            remove(object, path);
        }
        set_all(object, self.set);
        if self.order.is_empty() {
            return;
        }
        let mut rest = std::mem::take(object);
        for name in self.order {
            if let Some((name, value)) = rest.shift_remove_entry(name) {
                object.insert(name, value);
            }
        }
        object.extend(rest);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn removed(body: Value, path: &str) -> (bool, String) {
        let Value::Object(mut object) = body else {
            panic!("an object");
        };
        let found = remove(&mut object, path);
        (found, Value::Object(object).to_string())
    }

    #[test]
    fn a_named_field_is_removed_and_the_rest_keep_their_order() {
        let body = json!({"a": 1, "metadata": {"user_id": "u", "keep": true}, "z": 2});
        assert_eq!(
            removed(body.clone(), "a"),
            (
                true,
                r#"{"metadata":{"user_id":"u","keep":true},"z":2}"#.to_owned()
            )
        );
        assert_eq!(
            removed(body, "metadata.user_id"),
            (true, r#"{"a":1,"metadata":{"keep":true},"z":2}"#.to_owned())
        );
    }

    #[test]
    fn a_path_that_names_nothing_removes_nothing() {
        let body = json!({"a": 1, "list": [{"b": 2}], "text": "a.b"});
        let untouched = body.to_string();
        for path in ["b", "a.b", "missing.b", "list.b", "text.b", ""] {
            assert_eq!(
                removed(body.clone(), path),
                (false, untouched.clone()),
                "{path:?}"
            );
        }
    }

    fn set(body: Value, fields: &[(&str, Value)]) -> Value {
        let Value::Object(mut object) = body else {
            panic!("an object");
        };
        let fields: Vec<BodyField> = fields
            .iter()
            .map(|(path, value)| BodyField {
                path: path.split('.').map(str::to_owned).collect(),
                value: value.clone(),
            })
            .collect();
        set_all(&mut object, &fields);
        Value::Object(object)
    }

    #[test]
    fn a_field_is_added_with_the_objects_on_its_way() {
        assert_eq!(
            set(
                json!({"model": "m"}),
                &[
                    ("extraData.sessionId", json!("s-1")),
                    ("extraData.clientVersion", json!("1.0")),
                    ("user", json!("u")),
                ]
            ),
            json!({
                "model": "m",
                "extraData": {"sessionId": "s-1", "clientVersion": "1.0"},
                "user": "u"
            })
        );
    }

    #[test]
    fn a_field_that_is_there_is_replaced_where_it_is() {
        let out = set(
            json!({"a": 1, "metadata": {"user_id": "u", "keep": true}, "z": 2}),
            &[("metadata.user_id", json!("other")), ("a", json!([1, 2]))],
        );
        assert_eq!(
            out,
            json!({"a": [1, 2], "metadata": {"user_id": "other", "keep": true}, "z": 2})
        );
        let keys: Vec<&String> = out.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["a", "metadata", "z"], "order kept");
    }

    #[test]
    fn a_step_that_is_not_an_object_becomes_one() {
        assert_eq!(
            set(
                json!({"extraData": "{\"old\":1}", "a": {"b": [1]}}),
                &[("extraData.sessionId", json!("s")), ("a.b.c", json!(true))]
            ),
            json!({"extraData": {"sessionId": "s"}, "a": {"b": {"c": true}}})
        );
    }

    fn edited(body: Value, edits: &BodyEdits<'_>) -> String {
        let Value::Object(mut object) = body else {
            panic!("an object");
        };
        edits.apply(&mut object);
        Value::Object(object).to_string()
    }

    fn strings(list: &[&str]) -> Vec<String> {
        list.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn edits_omit_then_set_then_order() {
        let set = [
            BodyField {
                path: vec!["user".into()],
                value: json!("set"),
            },
            BodyField {
                path: vec!["extra".into(), "a".into()],
                value: json!(1),
            },
            BodyField {
                path: vec!["extra".into(), "a".into()],
                value: json!(2),
            },
        ];
        let body = json!({
            "model": "m", "messages": [], "user": "sent", "effort": "high",
            "meta": {"keep": 1, "gone": 2}, "max_tokens": 5, "stream": true
        });
        assert_eq!(
            edited(
                body,
                &BodyEdits {
                    omit: &strings(&["user", "effort", "meta.gone", "absent"]),
                    set: &set,
                    order: &strings(&["model", "max_tokens", "absent", "extra", "messages"]),
                }
            ),
            r#"{"model":"m","max_tokens":5,"extra":{"a":2},"messages":[],"meta":{"keep":1},"stream":true,"user":"set"}"#,
            "a field omitted and then set is there, where a new field goes; \
             a listed field the body lacks is skipped"
        );
    }

    #[test]
    fn no_edit_leaves_the_body_as_written() {
        let body = json!({"model": "m", "messages": [], "stream": false});
        assert_eq!(
            edited(body.clone(), &BodyEdits::default()),
            body.to_string()
        );
    }
}
