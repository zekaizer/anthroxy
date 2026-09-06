//! The header a backend credential travels in.

use std::fmt;

use http::header::{AUTHORIZATION, HeaderName, HeaderValue};
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, Visitor};

pub static X_API_KEY: HeaderName = HeaderName::from_static("x-api-key");

/// Configured as `"bearer"`, `"x_api_key"`, or `{ name = "...", scheme = "..." }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialHeader {
    pub name: HeaderName,
    /// Written before the secret, separated by one space.
    pub scheme: Option<String>,
}

impl CredentialHeader {
    /// `Authorization: Bearer <secret>`
    pub fn bearer() -> Self {
        Self {
            name: AUTHORIZATION,
            scheme: Some("Bearer".to_owned()),
        }
    }

    /// `x-api-key: <secret>`
    pub fn x_api_key() -> Self {
        Self {
            name: X_API_KEY.clone(),
            scheme: None,
        }
    }

    /// Any header. `scheme` must be one word that can travel in a header.
    pub fn custom(name: &str, scheme: Option<String>) -> Result<Self, String> {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("`{}` is not a valid header name", name.escape_debug()))?;
        if let Some(scheme) = &scheme
            && (scheme.is_empty()
                || scheme.contains(char::is_whitespace)
                || HeaderValue::from_str(scheme).is_err())
        {
            return Err(format!(
                "scheme `{}` must be a single word",
                scheme.escape_debug()
            ));
        }
        Ok(Self { name, scheme })
    }

    /// The header value carrying `secret`.
    pub fn value(&self, secret: &str) -> String {
        match &self.scheme {
            Some(scheme) => format!("{scheme} {secret}"),
            None => secret.to_owned(),
        }
    }
}

impl Default for CredentialHeader {
    fn default() -> Self {
        Self::bearer()
    }
}

impl<'de> Deserialize<'de> for CredentialHeader {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(HeaderVisitor)
    }
}

struct HeaderVisitor;

impl<'de> Visitor<'de> for HeaderVisitor {
    type Value = CredentialHeader;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(r#""bearer", "x_api_key", or { name = "...", scheme = "..." }"#)
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Self::Value, E> {
        match text {
            "bearer" => Ok(CredentialHeader::bearer()),
            "x_api_key" => Ok(CredentialHeader::x_api_key()),
            other => Err(de::Error::unknown_variant(other, &["bearer", "x_api_key"])),
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        const FIELDS: &[&str] = &["name", "scheme"];
        let mut name: Option<String> = None;
        let mut scheme: Option<String> = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "name" => name = Some(map.next_value()?),
                "scheme" => scheme = Some(map.next_value()?),
                other => return Err(de::Error::unknown_field(other, FIELDS)),
            }
        }
        let name = name.ok_or_else(|| de::Error::missing_field("name"))?;
        CredentialHeader::custom(&name, scheme).map_err(de::Error::custom)
    }
}
