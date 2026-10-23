//! What a Messages request asks for besides its content: the parameters the
//! console shows with an exchange.

use std::fmt;

use serde::de::{Deserializer, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Counts and settings of one request body, read leniently: a field of the
/// wrong shape reads as absent and is left for the backend to reject.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RequestParams {
    /// The body as received.
    pub bytes: u64,
    pub max_tokens: Option<u64>,
    /// `thinking.type`, as sent.
    pub thinking: Option<String>,
    /// `thinking.budget_tokens`.
    pub thinking_budget: Option<u64>,
    pub messages: usize,
    /// Entries in `tools`; `mcp_tools` of them are named `mcp__…`.
    pub tools: usize,
    pub mcp_tools: usize,
    /// Blocks in `system`; a string is one.
    pub system_blocks: usize,
}

#[derive(Deserialize)]
struct Body {
    #[serde(default)]
    max_tokens: Option<Value>,
    #[serde(default)]
    thinking: Option<Value>,
    #[serde(default)]
    messages: Count,
    #[serde(default)]
    tools: Option<Value>,
    #[serde(default)]
    system: Option<Value>,
}

/// The length of an array, skipped rather than kept: `messages` is most of a
/// body. Anything but an array counts zero.
#[derive(Default)]
struct Count(usize);

impl<'de> Deserialize<'de> for Count {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(CountVisitor)
    }
}

struct CountVisitor;

impl<'de> Visitor<'de> for CountVisitor {
    type Value = Count;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Count, A::Error> {
        let mut count = 0;
        while seq.next_element::<IgnoredAny>()?.is_some() {
            count += 1;
        }
        Ok(Count(count))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Count, A::Error> {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Count(0))
    }

    fn visit_bool<E>(self, _: bool) -> Result<Count, E> {
        Ok(Count(0))
    }

    fn visit_i64<E>(self, _: i64) -> Result<Count, E> {
        Ok(Count(0))
    }

    fn visit_u64<E>(self, _: u64) -> Result<Count, E> {
        Ok(Count(0))
    }

    fn visit_f64<E>(self, _: f64) -> Result<Count, E> {
        Ok(Count(0))
    }

    fn visit_str<E>(self, _: &str) -> Result<Count, E> {
        Ok(Count(0))
    }

    fn visit_unit<E>(self) -> Result<Count, E> {
        Ok(Count(0))
    }
}

/// Reads the parameters of a Messages request body; a body that is not a
/// JSON object reads as its size alone.
pub fn params(body: &[u8]) -> RequestParams {
    let bytes = body.len() as u64;
    // A derived struct also reads a JSON array, positionally.
    let object = body.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{');
    let Some(body) = object
        .then(|| serde_json::from_slice::<Body>(body).ok())
        .flatten()
    else {
        return RequestParams {
            bytes,
            ..RequestParams::default()
        };
    };
    let thinking = body.thinking.as_ref();
    let tools = match &body.tools {
        Some(Value::Array(tools)) => tools.as_slice(),
        _ => &[],
    };
    RequestParams {
        bytes,
        max_tokens: body.max_tokens.as_ref().and_then(Value::as_u64),
        thinking: thinking
            .and_then(|t| t.get("type"))
            .and_then(Value::as_str)
            .map(str::to_owned),
        thinking_budget: thinking
            .and_then(|t| t.get("budget_tokens"))
            .and_then(Value::as_u64),
        messages: body.messages.0,
        tools: tools.len(),
        mcp_tools: tools
            .iter()
            .filter(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.starts_with("mcp__"))
            })
            .count(),
        system_blocks: match &body.system {
            Some(Value::String(_)) => 1,
            Some(Value::Array(blocks)) => blocks.len(),
            _ => 0,
        },
    }
}
