//! `drop_headers`: which of the client's headers a backend never sees.

use http::HeaderName;

/// What `@claude-code` stands for: the headers Claude Code adds to name
/// itself and its SDK, none of which carries the request. `user-agent` is not
/// among them — a gateway may route or log by it, so it goes only when named.
const CLAUDE_CODE: [&str; 4] = [
    "x-stainless-*",
    "x-app",
    "anthropic-dangerous-direct-browser-access",
    "x-claude-code-session-id",
];

pub const CLAUDE_CODE_PRESET: &str = "@claude-code";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PatternError {
    #[error("an entry must not be empty")]
    Empty,
    #[error("`{0}` is not a preset; the only one is `{CLAUDE_CODE_PRESET}`")]
    UnknownPreset(String),
    #[error("`{0}` matches every header")]
    MatchesEverything(String),
    #[error("`{0}` is not a header name or a `*` pattern")]
    NotAHeaderName(String),
}

/// One `drop_headers` entry: a header name, or a glob whose `*` stands for
/// any run of characters. Header names are lowercase on the wire, so is this.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderPattern {
    /// The literal runs between the `*`s, in order; never empty.
    parts: Vec<String>,
    open_start: bool,
    open_end: bool,
}

impl HeaderPattern {
    /// The entry as written; a preset expands to several patterns.
    pub fn parse(entry: &str) -> Result<Vec<Self>, PatternError> {
        if entry.is_empty() {
            return Err(PatternError::Empty);
        }
        if let Some(preset) = entry.strip_prefix('@') {
            if preset != &CLAUDE_CODE_PRESET[1..] {
                return Err(PatternError::UnknownPreset(entry.to_owned()));
            }
            return Ok(CLAUDE_CODE
                .iter()
                .map(|pattern| Self::one(pattern).expect("preset patterns are valid"))
                .collect());
        }
        Ok(vec![Self::one(entry)?])
    }

    fn one(entry: &str) -> Result<Self, PatternError> {
        let lower = entry.to_ascii_lowercase();
        let segments: Vec<&str> = lower.split('*').collect();
        let parts: Vec<String> = segments
            .iter()
            .filter(|part| !part.is_empty())
            .map(|part| (*part).to_owned())
            .collect();
        if parts.is_empty() {
            return Err(PatternError::MatchesEverything(entry.to_owned()));
        }
        // A pattern is a header name with the `*`s taken out, so the same
        // characters are rejected here as in a header the router sends.
        if HeaderName::from_bytes(parts.concat().as_bytes()).is_err() {
            return Err(PatternError::NotAHeaderName(entry.to_owned()));
        }
        Ok(Self {
            open_start: segments[0].is_empty(),
            open_end: segments[segments.len() - 1].is_empty(),
            parts,
        })
    }

    pub fn matches(&self, name: &str) -> bool {
        let mut rest = name;
        for (index, part) in self.parts.iter().enumerate() {
            let anchored = index == 0 && !self.open_start;
            let found = if anchored {
                rest.starts_with(part.as_str()).then_some(0)
            } else {
                rest.find(part.as_str())
            };
            let Some(at) = found else { return false };
            rest = &rest[at + part.len()..];
        }
        self.open_end || rest.is_empty()
    }
}

/// A backend's `drop_headers`, ready to match against. Empty drops nothing.
#[derive(Debug, Clone, Default)]
pub struct DropHeaders(Vec<HeaderPattern>);

impl DropHeaders {
    /// Assumes the entries passed validation.
    pub fn new(entries: &[String]) -> Self {
        Self(
            entries
                .iter()
                .flat_map(|entry| HeaderPattern::parse(entry).expect("validated drop_headers"))
                .collect(),
        )
    }

    pub fn matches(&self, name: &HeaderName) -> bool {
        self.0.iter().any(|pattern| pattern.matches(name.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, name: &str) -> bool {
        HeaderPattern::parse(pattern).unwrap()[0].matches(name)
    }

    #[test]
    fn a_name_matches_only_itself() {
        assert!(matches("x-app", "x-app"));
        assert!(matches("X-App", "x-app"), "written in any case");
        assert!(!matches("x-app", "x-app-id"));
        assert!(!matches("x-app", "my-x-app"));
    }

    #[test]
    fn a_star_stands_for_any_run() {
        assert!(matches("x-stainless-*", "x-stainless-lang"));
        assert!(matches("x-stainless-*", "x-stainless-"));
        assert!(!matches("x-stainless-*", "x-stainles"));
        assert!(matches("*-session-id", "x-claude-code-session-id"));
        assert!(matches("x-*-id", "x-session-id"));
        assert!(!matches("x-*-id", "x-session-name"));
    }

    #[test]
    fn the_preset_expands_to_what_claude_code_adds() {
        let patterns = HeaderPattern::parse(CLAUDE_CODE_PRESET).unwrap();
        assert_eq!(patterns.len(), CLAUDE_CODE.len());
        let drops = DropHeaders::new(&[CLAUDE_CODE_PRESET.to_owned()]);
        for name in [
            "x-stainless-lang",
            "x-app",
            "anthropic-dangerous-direct-browser-access",
            "x-claude-code-session-id",
        ] {
            assert!(drops.matches(&HeaderName::from_static(name)), "{name}");
        }
        assert!(
            !drops.matches(&HeaderName::from_static("user-agent")),
            "user-agent goes only when named"
        );
    }

    #[test]
    fn entries_that_are_not_patterns() {
        assert_eq!(HeaderPattern::parse(""), Err(PatternError::Empty));
        assert_eq!(
            HeaderPattern::parse("@everything"),
            Err(PatternError::UnknownPreset("@everything".to_owned()))
        );
        assert_eq!(
            HeaderPattern::parse("**"),
            Err(PatternError::MatchesEverything("**".to_owned()))
        );
        assert_eq!(
            HeaderPattern::parse("x header"),
            Err(PatternError::NotAHeaderName("x header".to_owned()))
        );
    }
}
