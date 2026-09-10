use std::collections::BTreeMap;

use super::{Event, StopReason, Usage};

/// A completed response: the events of one stream folded together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    pub model: String,
    pub blocks: Vec<Block>,
    pub stop_reason: StopReason,
    pub usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Thinking(String),
    Text(String),
    /// `arguments` is the raw JSON text as the backend produced it.
    ToolUse {
        id: String,
        name: String,
        arguments: String,
    },
}

impl Message {
    /// Folds `events` with the same block rules a stream encoder applies:
    /// consecutive deltas of one kind share a block, a different kind opens a
    /// new one, tool calls are keyed by their index. `Err` carries the
    /// message of an `Event::Error`.
    pub fn from_events(events: impl IntoIterator<Item = Event>) -> Result<Self, String> {
        let mut message = Self {
            id: String::new(),
            model: String::new(),
            blocks: Vec::new(),
            stop_reason: StopReason::EndTurn,
            usage: Usage::default(),
        };
        let mut tools: BTreeMap<u32, usize> = BTreeMap::new();
        let mut finish = None;
        for event in events {
            match event {
                Event::Start { id, model } => {
                    message.id = id;
                    message.model = model;
                }
                Event::ThinkingDelta(text) => match message.blocks.last_mut() {
                    Some(Block::Thinking(existing)) => existing.push_str(&text),
                    _ => message.blocks.push(Block::Thinking(text)),
                },
                Event::TextDelta(text) => match message.blocks.last_mut() {
                    Some(Block::Text(existing)) => existing.push_str(&text),
                    _ => message.blocks.push(Block::Text(text)),
                },
                Event::ToolCallStart { index, id, name } => {
                    tools.insert(index, message.blocks.len());
                    message.blocks.push(Block::ToolUse {
                        id,
                        name,
                        arguments: String::new(),
                    });
                }
                Event::ToolCallDelta { index, arguments } => {
                    if let Some(Block::ToolUse { arguments: all, .. }) =
                        tools.get(&index).map(|&at| &mut message.blocks[at])
                    {
                        all.push_str(&arguments);
                    }
                }
                Event::Finish(reason) => finish = Some(reason),
                Event::Usage(usage) => message.usage = usage,
                Event::Error(text) => return Err(text),
                Event::Done => break,
            }
        }
        message.stop_reason = match finish {
            Some(StopReason::EndTurn) | None if !tools.is_empty() => StopReason::ToolUse,
            Some(reason) => reason,
            None => StopReason::EndTurn,
        };
        Ok(message)
    }
}
