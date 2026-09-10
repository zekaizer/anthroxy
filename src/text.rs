//! Text on its way into a message or a log line: escaped, so one line stays
//! one line, and cut, so what someone outside the router chose cannot be
//! echoed back at its own length.

/// A name the router did not choose — a model id, a request path, a method.
pub fn short(name: &str) -> String {
    cut(name, 64)
}

/// `text` escaped and cut to `max` characters, with an ellipsis when it was
/// cut. `max` counts escaped characters, so the result is never longer.
pub fn cut(text: &str, max: usize) -> String {
    let escaped: Vec<char> = text.escape_debug().take(max + 1).collect();
    match escaped.len() > max {
        true => escaped[..max].iter().collect::<String>() + "…",
        false => escaped.iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_break_cannot_survive() {
        assert_eq!(cut("one\ntwo\ttab", 64), "one\\ntwo\\ttab");
        assert_eq!(cut("plain", 64), "plain");
    }

    #[test]
    fn what_is_too_long_ends_in_an_ellipsis() {
        assert_eq!(cut("abcdef", 3), "abc…");
        assert_eq!(cut("abc", 3), "abc", "exactly the limit is not cut");
        assert_eq!(short(&"m".repeat(100)).chars().count(), 65);
    }
}
