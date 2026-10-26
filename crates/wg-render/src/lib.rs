//! wg-render: rich-compatible tables, key-value blocks and colour rules.
//!
//! Reimplements the part of Python `rich` that `workgraph.cli.common` uses (`table`, `kv`,
//! `fail`, the error report): markup parsing, word wrapping with cell widths, and the heavy-head
//! table layout. Every function returns the text the Python code would have written, so callers
//! decide where it goes. Output is always plain text: styles (bold, colours) are parsed so tags
//! match and vanish exactly as in rich, but no escape sequences are produced, whether or not
//! [`Console::color`] is set. Emoji shortcodes such as `:rocket:` are replaced before the markup is
//! parsed, as rich does.

mod cells;
mod emoji;
mod emoji_data;
mod markup;
mod rich_data;
mod table;
mod text;

pub use cells::{cell_len, char_cells};
pub use markup::{MarkupError, escape};

use text::{Justify, crop, wrap};

/// Console width when nothing says otherwise.
pub const DEFAULT_WIDTH: usize = 80;

/// What `rich.console.Console` reads from the process to decide its width and colour.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConsoleEnv<'a> {
    /// The `COLUMNS` variable.
    pub columns: Option<&'a str>,
    /// The `TERM` variable.
    pub term: Option<&'a str>,
    /// Whether output goes to a terminal (see [`terminal_detected`]).
    pub is_terminal: bool,
    /// Width reported by the operating system for the terminal, if there is one.
    pub terminal_width: Option<usize>,
}

/// `Console.is_terminal`: whether the output stream counts as a terminal.
///
/// `TTY_COMPATIBLE` of `0` or `1` decides outright, then a set `FORCE_COLOR` (true unless
/// empty), then whether the stream really is a terminal.
#[must_use]
pub fn terminal_detected(
    isatty: bool,
    tty_compatible: Option<&str>,
    force_color: Option<&str>,
) -> bool {
    match tty_compatible {
        Some("0") => false,
        Some("1") => true,
        _ => force_color.map_or(isatty, |value| !value.is_empty()),
    }
}

/// The text one call wrote, and the markup error that stopped it, if any.
///
/// Python writes as it goes, so output produced before an error is kept: `kv` prints the lines
/// before the failing one, a table prints nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Printed {
    /// Everything written before the call ended.
    pub text: String,
    /// The `MarkupError` the Python call would have raised.
    pub error: Option<MarkupError>,
}

impl Printed {
    fn ok(text: String) -> Self {
        Self { text, error: None }
    }
}

/// A console of a fixed width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Console {
    width: usize,
    color: bool,
}

impl Console {
    /// A plain console `width` cells wide.
    #[must_use]
    pub fn new(width: usize) -> Self {
        Self {
            width,
            color: false,
        }
    }

    /// A console sized and coloured the way rich would for this environment: `COLUMNS` (digits
    /// only) beats the terminal's own width, and zero or no width means 80. A dumb terminal is 80
    /// wide with no colour; colour is on only for a terminal.
    #[must_use]
    pub fn from_env(env: &ConsoleEnv<'_>) -> Self {
        let dumb = env.is_terminal
            && env
                .term
                .is_some_and(|t| matches!(t.to_lowercase().as_str(), "dumb" | "unknown"));
        if dumb {
            return Self::new(DEFAULT_WIDTH);
        }
        let from_columns = env
            .columns
            .filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_digit()))
            .map(|c| c.parse::<usize>().unwrap_or(usize::MAX));
        let width = from_columns.or(env.terminal_width).unwrap_or(0);
        Self {
            width: if width == 0 { DEFAULT_WIDTH } else { width },
            color: env.is_terminal,
        }
    }

    /// Width in cells.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Whether the console would colour output: only when writing to a terminal.
    #[must_use]
    pub fn color(&self) -> bool {
        self.color
    }

    /// `console.print(markup)`: one wrapped paragraph per call, each line ending in a newline.
    #[must_use]
    pub fn print(&self, markup: &str) -> Printed {
        match self.print_lines(markup) {
            Ok(text) => Printed::ok(text),
            Err(error) => Printed {
                text: String::new(),
                error: Some(error),
            },
        }
    }

    fn print_lines(&self, markup: &str) -> Result<String, MarkupError> {
        let plain = markup::render_plain(markup)?;
        Ok(lines_to_string(
            &wrap(&plain, self.width, Justify::Default),
            self.width,
        ))
    }

    /// `common.table`: a table, or the dim "nothing to show" line when there are no rows.
    ///
    /// Cells are markup, as in Python; pass values through [`escape`] to show them literally.
    #[must_use]
    pub fn table(&self, rows: &[Vec<String>], headers: &[&str], title: Option<&str>) -> Printed {
        let title = title.filter(|t| !t.is_empty());
        if rows.is_empty() {
            let prefix = title.map_or_else(String::new, |t| format!("{t}: "));
            return self.print(&format!("[dim]{prefix}nothing to show[/dim]"));
        }
        match table::render_table(rows, headers, title, self.width) {
            Ok(lines) => Printed::ok(lines_to_string(&lines, self.width)),
            Err(error) => Printed {
                text: String::new(),
                error: Some(error),
            },
        }
    }

    /// `common.kv`: a bold title and indented `key: value` lines. A pair whose value is `None`
    /// is left out (Python also leaves out empty lists and dicts; map those to `None`).
    #[must_use]
    pub fn kv(&self, pairs: &[(&str, Option<&str>)], title: Option<&str>) -> Printed {
        let mut lines = Vec::new();
        if let Some(title) = title.filter(|t| !t.is_empty()) {
            lines.push(format!("[bold]{title}[/bold]"));
        }
        for (key, value) in pairs {
            if let Some(value) = value {
                lines.push(format!("  [cyan]{key}[/cyan]: {value}"));
            }
        }
        self.print_all(&lines)
    }

    /// `common.fail` in human mode: the line written to standard error.
    #[must_use]
    pub fn fail(&self, message: &str) -> Printed {
        self.print(&format!("[red]error:[/red] {message}"))
    }

    /// `common.handle_errors` for a domain error in human mode: the report for standard error.
    /// Detail values are the Python `str()` of each value; the `errors` key is skipped.
    #[must_use]
    pub fn error_report(&self, code: &str, message: &str, details: &[(&str, &str)]) -> Printed {
        let mut lines = vec![format!("[red]error[/red] ({code}): {message}")];
        for (key, value) in details {
            if *key != "errors" {
                lines.push(format!("  {key}: {value}"));
            }
        }
        self.print_all(&lines)
    }

    /// Prints each markup line in turn, stopping at the first error and keeping what came before.
    fn print_all(&self, lines: &[String]) -> Printed {
        let mut text = String::new();
        for line in lines {
            match self.print_lines(line) {
                Ok(printed) => text.push_str(&printed),
                Err(error) => {
                    return Printed {
                        text,
                        error: Some(error),
                    };
                }
            }
        }
        Printed::ok(text)
    }
}

/// Joins lines with newlines after cropping each to the console width.
fn lines_to_string(lines: &[Vec<char>], width: usize) -> String {
    let mut out = String::new();
    for line in lines {
        out.extend(crop(line, width));
        out.push('\n');
    }
    out
}
