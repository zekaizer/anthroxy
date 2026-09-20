//! What a Messages request asks, in a line: the body log lists entries by it.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A request as the recordings list names it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RequestSummary {
    /// `messages` in the body.
    pub messages: usize,
    /// The prompt of the turn the request belongs to, on one line and cut:
    /// the text the user typed in the latest user message that has some,
    /// a message sent while the model was working, or a slash or shell
    /// command; system reminders and Claude Code's notices are left out.
    /// `None` when the turn started without one — a notice woke the model —
    /// or no user message carries one.
    pub prompt: Option<String>,
    /// What the request sends when its last message carries no prompt, on one
    /// line and in words rather than a notation to learn: the tools whose
    /// results it returns and the notices it holds.
    /// `None` when the last message carries the prompt.
    pub step: Option<String>,
    /// The last message returns tool results, so it carries on the turn the
    /// request before it began. What the console groups a turn by; reading it
    /// out of [`Self::step`] would tie that grouping to how the line is
    /// worded.
    pub answers: bool,
}

/// Characters of a prompt kept before the ellipsis.
const PROMPT_CHARS: usize = 200;

const REMINDER_OPEN: &str = "<system-reminder>";
const REMINDER_CLOSE: &str = "</system-reminder>";
/// How a reminder carrying a message the user sent mid-turn begins.
const QUEUED: &str = "The user sent a new message while you were working:";

/// Text blocks Claude Code adds to a user message when the user stops a turn.
const INTERRUPTED: [&str; 2] = [
    "[Request interrupted by user]",
    "[Request interrupted by user for tool use]",
];

/// How user text Claude Code writes itself begins, and what it is. Matched at
/// the start of the text only, so the user's own words that mention one stay
/// text. The console has the same table; `notice_tables_match` holds the two
/// together.
pub(crate) const NOTICES: [(&str, &str); 17] = [
    ("<local-command-caveat>", "command output"),
    ("<local-command-stdout>", "command output"),
    ("<local-command-stderr>", "command output"),
    ("<bash-stdout>", "shell output"),
    ("<bash-stderr>", "shell output"),
    ("<task-notification>", "task notification"),
    ("<ide_opened_file>", "ide context"),
    ("<ide_selection>", "ide context"),
    ("Stop hook feedback:", "hook feedback"),
    ("Goal check-in:", "goal check-in"),
    ("A session-scoped Stop hook is now active", "goal set"),
    (
        "This session is being continued from a previous conversation",
        "compaction summary",
    ),
    ("Base directory for this skill:", "skill"),
    ("Another Claude session sent a message:", "agent message"),
    ("Continue from where you left off.", "continue"),
    ("[Your previous response had no visible output.", "continue"),
    ("[Image: original ", "image note"),
];

/// The notices that wake the model with nothing typed, which starts a turn of
/// its own: the prompt above one belongs to the turn before it.
pub(crate) const WAKING: [&str; 6] = [
    "task notification",
    "hook feedback",
    "goal check-in",
    "goal set",
    "agent message",
    "continue",
];

const TOOL_USES: [&str; 3] = ["tool_use", "server_tool_use", "mcp_tool_use"];
const TOOL_RESULTS: [&str; 2] = ["tool_result", "mcp_tool_result"];

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
    RequestSummary {
        messages: body.messages.len(),
        prompt: turn_prompt(&body.messages).map(|text| one_line(&text)),
        answers: answers(&body.messages),
        step: step(&body.messages).map(|text| one_line(&text)),
    }
}

/// The last message returns tool results. Read from the block types alone:
/// the console groups a turn by this, and it must not depend on how [`step`]
/// words the same thing.
fn answers(messages: &[Value]) -> bool {
    let last = messages
        .iter()
        .rev()
        .find(|m| matches!(role(m), Some("user" | "assistant")));
    let Some(Value::Array(blocks)) = last.and_then(|m| m.get("content")) else {
        return false;
    };
    blocks.iter().any(|block| {
        block
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| TOOL_RESULTS.contains(&kind))
    })
}

/// The prompt of the turn the last message belongs to: the typed text of the
/// newest user message that carries some, read backwards. A user message that
/// only wakes the model — a notice with no prompt and no tool result to carry
/// the running turn — starts a turn of its own, and the reading stops there
/// rather than borrowing the prompt of the turn before it.
fn turn_prompt(messages: &[Value]) -> Option<String> {
    for message in messages.iter().rev().filter(|m| role(m) == Some("user")) {
        let read = read(message.get("content"));
        if read.prompt.is_some() {
            return read.prompt;
        }
        if read.wakes() && read.results.is_empty() {
            return None;
        }
    }
    None
}

fn role(message: &Value) -> Option<&str> {
    message.get("role").and_then(Value::as_str)
}

/// What one user message carries.
#[derive(Default)]
struct Read<'a> {
    prompt: Option<String>,
    /// Claude Code's notices, in order, each once.
    notices: Vec<&'static str>,
    /// `tool_use_id` of each tool result, in order; `None` for one without.
    results: Vec<Option<&'a str>>,
    reminders: bool,
}

impl Read<'_> {
    /// One of the notices this message carries wakes the model with nothing
    /// typed.
    fn wakes(&self) -> bool {
        self.notices.iter().any(|label| WAKING.contains(label))
    }
}

/// A user message's prompt: its typed text beside reminders and notices, or
/// the text of a later reminder through which Claude Code delivers a message
/// the user sent while the model was working.
fn read(content: Option<&Value>) -> Read<'_> {
    let mut read = Read::default();
    let texts: Vec<&str> = match content {
        Some(Value::String(text)) => vec![text],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| {
                let kind = block.get("type").and_then(Value::as_str)?;
                if TOOL_RESULTS.contains(&kind) {
                    read.results
                        .push(block.get("tool_use_id").and_then(Value::as_str));
                    return None;
                }
                (kind == "text")
                    .then(|| block.get("text").and_then(Value::as_str))
                    .flatten()
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut prompt = String::new();
    let mut queued = false;
    // Set by a slash command until other text or a notice: the text right
    // after a prompt command is what it expands to, not typed.
    let mut command = false;
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
            match classify(own.trim()) {
                Piece::Empty => {}
                Piece::Prompt(_) if command => command = false,
                Piece::Notice(label) => {
                    command = false;
                    if !read.notices.contains(&label) {
                        read.notices.push(label);
                    }
                }
                Piece::Command(typed) => {
                    command = true;
                    typed_text(&mut prompt, &mut queued, &typed);
                }
                Piece::Prompt(typed) => typed_text(&mut prompt, &mut queued, &typed),
            }
            if let Some(reminder) = reminder {
                read.reminders = true;
                if let Some(sent) = reminder.trim_start().strip_prefix(QUEUED)
                    && !sent.trim().is_empty()
                {
                    prompt = sent.to_owned();
                    queued = true;
                }
            }
            if next.is_empty() {
                break;
            }
            rest = next;
        }
    }
    read.prompt = (!prompt.trim().is_empty()).then_some(prompt);
    read
}

/// Adds text the user typed after what came before in the message; text
/// after a message sent mid-turn replaces it.
fn typed_text(prompt: &mut String, queued: &mut bool, typed: &str) {
    if *queued {
        prompt.clear();
        *queued = false;
    }
    prompt.push_str(typed);
    prompt.push('\n');
}

enum Piece {
    Empty,
    Prompt(String),
    /// A slash or shell command, as the user typed it.
    Command(String),
    Notice(&'static str),
}

/// What a trimmed stretch of user text outside reminders is.
fn classify(text: &str) -> Piece {
    if text.is_empty() {
        return Piece::Empty;
    }
    if INTERRUPTED.contains(&text) {
        return Piece::Notice("interrupted");
    }
    if let Some((_, label)) = NOTICES.iter().find(|(start, _)| text.starts_with(start)) {
        return Piece::Notice(label);
    }
    if (text.starts_with("<command-name>") || text.starts_with("<command-message>"))
        && let Some(name) = tagged(text, "command-name")
    {
        let args = tagged(text, "command-args").unwrap_or_default();
        return Piece::Command(format!("{name} {args}").trim().to_owned());
    }
    if text.starts_with("<bash-input>")
        && let Some(command) = tagged(text, "bash-input")
    {
        return Piece::Command(format!("! {command}"));
    }
    Piece::Prompt(text.to_owned())
}

/// The trimmed text between the first `<tag>` and its closing tag.
fn tagged<'a>(text: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    Some(text[start..end].trim())
}

/// What the last user or assistant message sends when it is no prompt.
fn step(messages: &[Value]) -> Option<String> {
    let last = messages
        .iter()
        .rev()
        .find(|m| matches!(role(m), Some("user" | "assistant")))?;
    if role(last) == Some("assistant") {
        return Some("assistant prefill".to_owned());
    }
    let read = read(last.get("content"));
    if read.prompt.is_some() {
        return None;
    }
    let mut parts = Vec::new();
    if !read.results.is_empty() {
        parts.push(format!("returns {}", tool_names(messages, &read.results)));
    }
    parts.extend(read.notices.iter().map(|label| (*label).to_owned()));
    if parts.is_empty() {
        parts.push(
            if read.reminders {
                "system reminder"
            } else {
                "no text"
            }
            .to_owned(),
        );
    }
    Some(parts.join(" · "))
}

/// Tool results named at most this many at a time; the rest are counted.
const NAMED_RESULTS: usize = 3;
/// Characters of a call's hint kept before the ellipsis.
const HINT_CHARS: usize = 60;
/// Input fields that say what a call did, in order of preference. A path
/// is cut to its last segment.
const HINTS: [&str; 9] = [
    "description",
    "file_path",
    "notebook_path",
    "path",
    "pattern",
    "url",
    "query",
    "command",
    "skill",
];

/// The calls `results` answer, in order: each tool's name and a hint from its
/// input, the same tool without a hint once with a count, `tool` for a call
/// the body does not hold; past [`NAMED_RESULTS`], a count of the rest.
fn tool_names(messages: &[Value], results: &[Option<&str>]) -> String {
    let mut calls = HashMap::new();
    for message in messages.iter().filter(|m| role(m) == Some("assistant")) {
        let Some(Value::Array(blocks)) = message.get("content") else {
            continue;
        };
        for block in blocks {
            if block
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| TOOL_USES.contains(&kind))
                && let (Some(id), Some(name)) = (
                    block.get("id").and_then(Value::as_str),
                    block.get("name").and_then(Value::as_str),
                )
            {
                calls.insert(id, (name, hint(block.get("input"))));
            }
        }
    }
    let mut named: Vec<(&str, Option<String>, usize)> = Vec::new();
    for id in results {
        let (name, hint) = id
            .and_then(|id| calls.get(id).cloned())
            .unwrap_or(("tool", None));
        match named
            .iter_mut()
            .find(|(seen, seen_hint, _)| hint.is_none() && seen_hint.is_none() && *seen == name)
        {
            Some((_, _, count)) => *count += 1,
            None => named.push((name, hint, 1)),
        }
    }
    let mut line = named
        .iter()
        .take(NAMED_RESULTS)
        .map(|(name, hint, count)| match (hint, count) {
            (Some(hint), _) => format!("{name} {hint}"),
            (None, 1) => (*name).to_owned(),
            (None, n) => format!("{name} ×{n}"),
        })
        .collect::<Vec<_>>()
        .join(", ");
    if named.len() > NAMED_RESULTS {
        line.push_str(&format!(" +{} more", named.len() - NAMED_RESULTS));
    }
    line
}

/// What a call's input says it did: the first line of the first hint field
/// that holds text, cut.
fn hint(input: Option<&Value>) -> Option<String> {
    let input = input?;
    let (field, text) = HINTS.iter().find_map(|field| {
        let text = input
            .get(*field)?
            .as_str()?
            .lines()
            .find(|l| !l.trim().is_empty())?;
        Some((*field, text.trim()))
    })?;
    let text = if field.ends_with("path") {
        text.trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(text)
    } else {
        text
    };
    let text = one_line(text);
    Some(match text.char_indices().nth(HINT_CHARS) {
        Some((cut, _)) => format!("{}…", &text[..cut]),
        None => text,
    })
}

fn one_line(text: &str) -> String {
    let mut line = String::new();
    // A pasted prompt can be megabytes; only the words before the cut are
    // ever built.
    for word in text.split_whitespace() {
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
        if line.chars().count() > PROMPT_CHARS {
            let cut = line.char_indices().nth(PROMPT_CHARS).map(|(at, _)| at);
            if let Some(cut) = cut {
                line.truncate(cut);
                line.push('…');
                return line;
            }
        }
    }
    line
}
