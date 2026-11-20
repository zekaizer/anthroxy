//! Client emulation presets (ADR-0022): a preset file once checked, and a
//! preset with a value for each of its variables.

mod library;
mod parse;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use http::HeaderName;

pub use library::{BindProblem, PresetLibrary};

use super::set_fields::PresetFields;
use super::template::PresetValue;
use super::{SetFields, Template};
use crate::openai::{MaxTokensField, SystemPlacement, ToolChoiceDefault, ToolOnlyContent};

/// A preset file whose every key and value passed the checks. An absent key
/// holds what the router does without a preset.
#[derive(Default)]
pub struct Preset {
    pub name: String,
    /// Declared variables with their defaults.
    pub variables: BTreeMap<String, String>,
    /// Every variable a value names is declared.
    headers: Vec<(HeaderName, PresetValue)>,
    /// Every entry is a pattern [`super::DropHeaders::new`] takes.
    pub drop_headers: Vec<String>,
    /// Lower-case names, each once.
    pub header_order: Vec<HeaderName>,
    pub header_case: HeaderCase,
    pub http_version: HttpVersion,
    /// Every variable a value names is declared.
    set_fields: PresetFields,
    /// Dot-separated paths, none under a field a preset leaves alone.
    pub omit_fields: Vec<String>,
    /// Top-level field names, each once.
    pub field_order: Vec<String>,
    pub mid_conversation_system: Option<SystemPlacement>,
    pub max_tokens_field: MaxTokensField,
    pub default_tool_choice: Option<ToolChoiceDefault>,
    pub tool_only_content: ToolOnlyContent,
    pub tool_strict: Option<bool>,
    /// Not empty, and not a field an assistant message already has.
    pub reasoning_field: Option<String>,
}

/// Names only: a header value may be a key, and the value of a variable is
/// written into one.
impl fmt::Debug for Preset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let headers: Vec<&str> = self.headers.iter().map(|(name, _)| name.as_str()).collect();
        f.debug_struct("Preset")
            .field("name", &self.name)
            .field("variables", &self.variables.keys())
            .field("headers", &headers)
            .finish_non_exhaustive()
    }
}

impl Preset {
    /// The preset `text` describes. `Err` holds every problem, each as
    /// `<key path>: <problem>`.
    pub fn parse(name: &str, text: &str) -> Result<Preset, Vec<String>> {
        parse::parse(name, text)
    }

    /// Whether a header value names `variable`.
    fn sends_in_a_header(&self, variable: &str) -> bool {
        self.headers
            .iter()
            .any(|(_, value)| value.variables().any(|name| name == variable))
    }
}

/// How the header names of an HTTP/1.1 request are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeaderCase {
    /// `content-type`
    #[default]
    Lower,
    /// `Content-Type`: a capital first and after each hyphen.
    Title,
}

/// The HTTP version a route speaks with its backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
pub enum HttpVersion {
    /// The one the server and the router agree on.
    #[default]
    #[serde(skip)]
    Negotiated,
    /// HTTP/1.1 though the server offers HTTP/2.
    #[serde(rename = "1.1")]
    Http11,
}

/// A preset with a value for each of its variables: what one `emulate`
/// selects, as [`PresetLibrary::bind`] builds it.
#[derive(Clone)]
pub struct Emulation {
    pub preset: Arc<Preset>,
    /// Every declared variable: the value `emulate` gave, else the default.
    values: BTreeMap<String, String>,
}

/// Names only, as a [`Preset`] is shown.
impl fmt::Debug for Emulation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Emulation")
            .field("preset", &self.preset.name)
            .field("variables", &self.values.keys())
            .finish()
    }
}

impl Emulation {
    /// The preset's headers with every variable replaced by its value.
    pub fn headers(&self) -> Vec<(HeaderName, Template)> {
        self.preset
            .headers
            .iter()
            .map(|(name, value)| (name.clone(), value.bind(&self.values)))
            .collect()
    }

    /// The preset's `set_fields` with every variable replaced by its value.
    pub fn set_fields(&self) -> SetFields {
        self.preset.set_fields.bind(&self.values)
    }
}
