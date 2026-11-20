//! Preset text → [`Preset`]. The table is walked by hand, so every problem
//! is reported and each names its key path.

use std::collections::BTreeMap;

use http::{HeaderName, HeaderValue};

use super::Preset;
use crate::config::emulate::PRESET_KEY;
use crate::config::header_rules;
use crate::config::set_fields::PresetFields;
use crate::config::template::{PresetValue, is_variable_name};
use crate::text::short;

/// Reads one key's value into the preset, or says what is wrong with it.
type Reader = fn(&toml::Value, &mut Preset, &mut Vec<String>);

/// Every preset key with what reads it, in the order they are read: a later
/// reader may check against what an earlier one stored.
const KEYS: [(&str, Reader); 4] = [
    ("variables", variables),
    ("headers", headers),
    ("drop_headers", drop_headers),
    ("set_fields", set_fields),
];

pub(super) fn parse(name: &str, text: &str) -> Result<Preset, Vec<String>> {
    let table: toml::Table = toml::from_str(text).map_err(|error| vec![syntax(text, &error)])?;
    let mut problems = Vec::new();
    let mut preset = Preset {
        name: name.to_owned(),
        ..Preset::default()
    };
    for (key, read) in KEYS {
        if let Some(value) = table.get(key) {
            read(value, &mut preset, &mut problems);
        }
    }
    for key in table.keys() {
        if !KEYS.iter().any(|(known, _)| known == key) {
            problems.push(format!("`{}` is not a preset key", short(key)));
        }
    }
    match problems.is_empty() {
        true => Ok(preset),
        false => Err(problems),
    }
}

/// One line: a problem is a list item wherever it is shown.
fn syntax(text: &str, error: &toml::de::Error) -> String {
    let line = error.span().map(|span| {
        let before = &text.as_bytes()[..span.start.min(text.len())];
        before.iter().filter(|byte| **byte == b'\n').count() + 1
    });
    match line {
        Some(line) => format!("line {line}: {}", short(error.message())),
        None => short(error.message()),
    }
}

fn variables(value: &toml::Value, preset: &mut Preset, problems: &mut Vec<String>) {
    let Some(table) = value.as_table() else {
        problems.push("variables: must be a table of names and default values".to_owned());
        return;
    };
    for (name, default) in table {
        let field = format!("variables.{}", short(name));
        if !is_variable_name(name) {
            problems.push(format!("{field}: a name is letters, digits and `_`"));
            continue;
        }
        if name == PRESET_KEY {
            problems.push(format!(
                "{field}: `{PRESET_KEY}` names the preset in `emulate` and cannot be a variable"
            ));
        } else if !default.is_str() {
            problems.push(format!("{field}: the default must be a string"));
        }
        // Declared whatever is wrong with it - a value that names it is not
        // a second problem.
        let default = default.as_str().unwrap_or_default();
        preset.variables.insert(name.clone(), default.to_owned());
    }
}

fn headers(value: &toml::Value, preset: &mut Preset, problems: &mut Vec<String>) {
    let Some(table) = strings("headers", value, problems) else {
        return;
    };
    header_rules::check_forced_headers(
        "headers",
        &table,
        |value| PresetValue::parse(value).map(|value| value.reads_client()),
        problems,
    );
    // The variables any entry names, one with a problem of its own included.
    let mut sent: Vec<String> = Vec::new();
    for (name, text) in &table {
        let field = format!("headers: `{}`", short(name));
        let named = PresetValue::named_in(text);
        undeclared(&field, named.iter().map(String::as_str), preset, problems);
        sent.extend(named);
        if let (Ok(header), Ok(value)) = (
            HeaderName::from_bytes(name.as_bytes()),
            PresetValue::parse(text),
        ) {
            preset.headers.push((header, value));
        }
    }
    // The rule of a value `emulate` gives: only what a header sends is
    // checked as a header value.
    for (variable, default) in &preset.variables {
        if sent.contains(variable) && HeaderValue::from_str(default).is_err() {
            problems.push(format!(
                "variables.{}: the default cannot be sent in a header",
                short(variable)
            ));
        }
    }
}

fn drop_headers(value: &toml::Value, preset: &mut Preset, problems: &mut Vec<String>) {
    let Some(entries) = string_list("drop_headers", value, problems) else {
        return;
    };
    let forced: Vec<&str> = preset
        .headers
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    header_rules::check_drop_headers("drop_headers", &entries, &forced, "this preset", problems);
    preset.drop_headers = entries;
}

fn set_fields(value: &toml::Value, preset: &mut Preset, problems: &mut Vec<String>) {
    let Some(table) = value.as_table() else {
        problems.push("set_fields: must be a table of body fields and values".to_owned());
        return;
    };
    let config = table.clone().into_iter().collect();
    let (fields, found) = PresetFields::compile(&config);
    problems.extend(found.iter().map(|problem| format!("set_fields: {problem}")));
    for (path, variables) in fields.variables() {
        let field = format!("set_fields: `{}`", short(&path));
        undeclared(&field, variables.into_iter(), preset, problems);
    }
    preset.set_fields = fields;
}

/// A problem at `field` for each of `variables` the preset does not declare.
fn undeclared<'a>(
    field: &str,
    variables: impl Iterator<Item = &'a str>,
    preset: &Preset,
    problems: &mut Vec<String>,
) {
    let mut reported: Vec<&str> = Vec::new();
    for variable in variables {
        if !preset.variables.contains_key(variable) && !reported.contains(&variable) {
            reported.push(variable);
            problems.push(format!(
                "{field}: `{{{}}}` names no variable in [variables]",
                short(variable)
            ));
        }
    }
}

/// `value` as a table of strings; `None`, with the problem said, when it is
/// not a table. An entry that is not a string is a problem and is left out.
fn strings(
    field: &str,
    value: &toml::Value,
    problems: &mut Vec<String>,
) -> Option<BTreeMap<String, String>> {
    let Some(table) = value.as_table() else {
        problems.push(format!("{field}: must be a table of names and values"));
        return None;
    };
    let mut out = BTreeMap::new();
    for (key, value) in table {
        match value.as_str() {
            Some(text) => {
                out.insert(key.clone(), text.to_owned());
            }
            None => problems.push(format!(
                "{field}: `{}`: the value must be a string",
                short(key)
            )),
        }
    }
    Some(out)
}

/// `value` as a list of strings; `None`, with the problem said, when it is
/// not a list. An entry that is not a string is a problem and is left out.
fn string_list(
    field: &str,
    value: &toml::Value,
    problems: &mut Vec<String>,
) -> Option<Vec<String>> {
    let Some(items) = value.as_array() else {
        problems.push(format!("{field}: must be a list of strings"));
        return None;
    };
    let mut entries = Vec::new();
    for (index, item) in items.iter().enumerate() {
        match item.as_str() {
            Some(text) => entries.push(text.to_owned()),
            None => problems.push(format!("{field}: entry {} must be a string", index + 1)),
        }
    }
    Some(entries)
}
