//! Terminal styling and table layout for command output.

use std::io::IsTerminal;

use owo_colors::OwoColorize;

#[derive(Debug, Clone, Copy)]
pub struct Style {
    color: bool,
}

impl Style {
    /// Colour only when stdout is a terminal and `NO_COLOR` is unset.
    pub fn detect() -> Self {
        Self {
            color: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    pub const fn plain() -> Self {
        Self { color: false }
    }

    pub fn ok(&self, text: &str) -> String {
        if self.color {
            text.green().to_string()
        } else {
            text.to_owned()
        }
    }

    pub fn warn(&self, text: &str) -> String {
        if self.color {
            text.yellow().to_string()
        } else {
            text.to_owned()
        }
    }

    pub fn err(&self, text: &str) -> String {
        if self.color {
            text.red().to_string()
        } else {
            text.to_owned()
        }
    }

    pub fn bold(&self, text: &str) -> String {
        if self.color {
            text.bold().to_string()
        } else {
            text.to_owned()
        }
    }

    pub fn dim(&self, text: &str) -> String {
        if self.color {
            text.dimmed().to_string()
        } else {
            text.to_owned()
        }
    }

    pub fn ok_mark(&self) -> String {
        self.ok("✓")
    }

    pub fn warn_mark(&self) -> String {
        self.warn("!")
    }

    pub fn err_mark(&self) -> String {
        self.err("✗")
    }
}

/// Left-aligned columns separated by two spaces. Cells may contain ANSI
/// sequences; width is computed on visible characters.
pub fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; columns];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(visible_width(cell));
        }
    }
    let mut out = String::new();
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().enumerate() {
            line.push_str(cell);
            if i + 1 < row.len() {
                let pad = widths[i] - visible_width(cell) + 2;
                line.push_str(&" ".repeat(pad));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn visible_width(text: &str) -> usize {
    let mut width = 0;
    let mut in_escape = false;
    for c in text.chars() {
        match (in_escape, c) {
            (true, 'm') => in_escape = false,
            (true, _) => {}
            (false, '\u{1b}') => in_escape = true,
            (false, _) => width += 1,
        }
    }
    width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_aligns_columns_and_ignores_ansi() {
        let rows = vec![
            vec!["id".to_owned(), "backend".to_owned()],
            vec!["\u{1b}[32mok\u{1b}[0m".to_owned(), "x".to_owned()],
            vec!["longer-id".to_owned(), "y".to_owned()],
        ];
        let out = table(&rows);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "id         backend");
        assert_eq!(lines[1], "\u{1b}[32mok\u{1b}[0m         x");
        assert_eq!(lines[2], "longer-id  y");
    }
}
