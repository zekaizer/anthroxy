//! What a Messages request asks, in a line: the body log lists entries by it.

use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

/// A request as the recordings list names it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RequestSummary {
    /// `messages` in the body.
    pub messages: usize,
    /// The last user message's own text, without system reminders, on one
    /// line and cut; `None` when no user message has text of its own.
    pub prompt: Option<String>,
}

/// Characters of a prompt kept before the ellipsis.
const PROMPT_CHARS: usize = 200;

const REMINDER_OPEN: &str = "<system-reminder>";
const REMINDER_CLOSE: &str = "</system-reminder>";

#[derive(Deserialize)]
struct Body {
    messages: Vec<Message>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    role: String,
    #[serde(default)]
    content: Content,
}

#[derive(Deserialize, Default)]
#[serde(untagged)]
enum Content {
    Text(String),
    Blocks(Vec<Block>),
    #[default]
    #[serde(skip)]
    None,
    Other(IgnoredAny),
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

/// Summarises a Messages request body; a body that does not read as one
/// summarises as empty.
pub fn summarize(body: &[u8]) -> RequestSummary {
    let Ok(body) = serde_json::from_slice::<Body>(body) else {
        return RequestSummary::default();
    };
    let prompt = body
        .messages
        .iter()
        .rev()
        .filter(|m| m.role == "user")
        .find_map(|m| own_text(&m.content))
        .map(|text| one_line(&text));
    RequestSummary {
        messages: body.messages.len(),
        prompt,
    }
}

/// The text a user message carries besides system reminders; tool results
/// and other blocks carry none.
fn own_text(content: &Content) -> Option<String> {
    let texts: Vec<&str> = match content {
        Content::Text(text) => vec![text],
        Content::Blocks(blocks) => blocks
            .iter()
            .filter(|b| b.kind == "text")
            .filter_map(|b| b.text.as_deref())
            .collect(),
        Content::None | Content::Other(_) => Vec::new(),
    };
    let own = texts
        .into_iter()
        .map(without_reminders)
        .collect::<Vec<_>>()
        .join("\n");
    (!own.trim().is_empty()).then_some(own)
}

fn without_reminders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(REMINDER_OPEN) {
        out.push_str(&rest[..start]);
        match rest[start..].find(REMINDER_CLOSE) {
            Some(end) => rest = &rest[start + end + REMINDER_CLOSE.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn one_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(PROMPT_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}
