//! `server.token`: the value itself, or the variable that holds it.

use std::fmt;

use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};

use super::env::{self, EnvSecretError};

/// Configured as `"value"` or `{ kind = "env", name = "VAR" }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenConfig {
    Literal(String),
    Env { name: String },
}

impl TokenConfig {
    /// The token now. A variable is read here rather than at load, so a
    /// command that never uses the token works without it.
    pub fn resolve(&self) -> Result<String, EnvSecretError> {
        match self {
            TokenConfig::Literal(value) => Ok(value.clone()),
            TokenConfig::Env { name } => env::secret(name, std::env::var(name).ok()),
        }
    }
}

impl<'de> Deserialize<'de> for TokenConfig {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(TokenVisitor)
    }
}

struct TokenVisitor;

impl<'de> Visitor<'de> for TokenVisitor {
    type Value = TokenConfig;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(r#"a string or { kind = "env", name = "..." }"#)
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
        Ok(TokenConfig::Literal(text.to_owned()))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        const FIELDS: &[&str] = &["kind", "name"];
        let mut kind: Option<String> = None;
        let mut name: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "kind" => kind = Some(map.next_value()?),
                "name" => name = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, FIELDS)),
            }
        }
        match kind.as_deref() {
            Some("env") => {}
            Some(other) => return Err(de::Error::unknown_variant(other, &["env"])),
            None => return Err(de::Error::missing_field("kind")),
        }
        let name = name.ok_or_else(|| de::Error::missing_field("name"))?;
        Ok(TokenConfig::Env { name })
    }
}
