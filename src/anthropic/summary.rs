//! What a Messages request asks, in a line: the body log lists entries by it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A request as the recordings list names it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RequestSummary {
    /// `messages` in the body.
    pub messages: usize,
    /// The user's last prompt, on one line and cut: a user message's own text
    /// without system reminders, or a message the user sent while the model
    /// was working; `None` when there is neither.
    pub prompt: Option<String>,
}

/// Characters of a prompt kept before the ellipsis.
const PROMPT_CHARS: usize = 200;

const REMINDER_OPEN: &str = "<system-reminder>";
const REMINDER_CLOSE: &str = "</system-reminder>";
/// How a reminder carrying a message the user sent mid-turn begins.
const QUEUED: &str = "The user sent a new message while you were working:";
/// Text blocks Claude Code adds to a user message when the user stops a turn;
/// the prompt that follows comes in the same message.
const INTERRUPTED: [&str; 2] = [
    "[Request interrupted by user]",
    "[Request interrupted by user for tool use]",
];

#[derive(Deserialize)]
struct Body {
    messages: Vec<Value>,
}

/// Summarises a Messages request body; a body without a `messages` array
/// summarises as empty, and a message or block of the wrong shape carries no
/// prompt.
pub fn summarize(body: &[u8]) -> RequestSummary {
    let Ok(body) = serde_json::from_slice::<Body>(body) else {
        return RequestSummary::default();
    };
    let prompt = body
        .messages
        .iter()
        .rev()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        .find_map(|m| prompt_in(m.get("content")))
        .map(|text| one_line(&text));
    RequestSummary {
        messages: body.messages.len(),
        prompt,
    }
}

/// The prompt a user message carries: its own text beside system reminders,
/// or the text of a later reminder through which Claude Code delivers a
/// message the user sent while the model was working. Tool results, other
/// blocks and interruption notices carry none.
fn prompt_in(content: Option<&Value>) -> Option<String> {
    let texts: Vec<&str> = match content {
        Some(Value::String(text)) => vec![text],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .filter(|text| !INTERRUPTED.contains(&text.trim()))
            .collect(),
        _ => Vec::new(),
    };
    let mut prompt = String::new();
    let mut queued = false;
    for text in texts {
        let mut rest = text;
        loop {
            let (own, reminder, next) = match rest.find(REMINDER_OPEN) {
                Some(start) => {
                    let inner = &rest[start + REMINDER_OPEN.len()..];
                    match inner.find(REMINDER_CLOSE) {
                        Some(end) => (
                            &rest[..start],
                            Some(&inner[..end]),
                            &inner[end + REMINDER_CLOSE.len()..],
                        ),
                        // An unclosed tag is text like any other.
                        None => (rest, None, ""),
                    }
                }
                None => (rest, None, ""),
            };
            if !own.trim().is_empty() {
                if queued {
                    prompt.clear();
                    queued = false;
                }
                prompt.push_str(own);
                prompt.push('\n');
            }
            if let Some(sent) = reminder.and_then(|r| r.trim_start().strip_prefix(QUEUED))
                && !sent.trim().is_empty()
            {
                prompt = sent.to_owned();
                queued = true;
            }
            if next.is_empty() {
                break;
            }
            rest = next;
        }
    }
    (!prompt.trim().is_empty()).then_some(prompt)
}

fn one_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(PROMPT_CHARS) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}
