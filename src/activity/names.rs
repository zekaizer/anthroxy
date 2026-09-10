//! A bounded tally of the model names clients send that no route names.

use serde::Serialize;

/// Distinct names kept; the least recently seen is forgotten first.
pub const NAMES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NameCount {
    /// Escaped and cut; the client chose it.
    pub name: String,
    /// Answered 404.
    pub unknown: u64,
    /// Served by `routing.default_model`.
    pub defaulted: u64,
    pub last_seen: jiff::Timestamp,
}

#[derive(Debug, Default)]
pub struct NameTally {
    names: Vec<NameCount>,
}

impl NameTally {
    pub fn unknown(&mut self, name: &str) {
        self.seen(name).unknown += 1;
    }

    pub fn defaulted(&mut self, name: &str) {
        self.seen(name).defaulted += 1;
    }

    /// Most recently seen first.
    pub fn list(&self) -> Vec<NameCount> {
        self.names.clone()
    }

    /// The entry for `name`, moved to the front.
    fn seen(&mut self, name: &str) -> &mut NameCount {
        let name = crate::text::cut(name, 200);
        let entry = match self.names.iter().position(|n| n.name == name) {
            Some(at) => self.names.remove(at),
            None => {
                if self.names.len() == NAMES {
                    self.names.pop();
                }
                NameCount {
                    name,
                    unknown: 0,
                    defaulted: 0,
                    last_seen: jiff::Timestamp::now(),
                }
            }
        };
        self.names.insert(0, entry);
        let front = &mut self.names[0];
        front.last_seen = jiff::Timestamp::now();
        front
    }
}
