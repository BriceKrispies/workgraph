//! wg-render: rich-compatible tables, key-value blocks and colour rules.
//!
//! Reimplements the part of Python `rich` that `workgraph.cli.common` uses (`table`, `kv`,
//! `fail`, the error report): markup parsing, word wrapping with cell widths, and the heavy-head
//! table layout. Every function returns the text the Python code would have written, so callers
//! decide where it goes. Markup styles (bold, colours) are parsed so tags match and vanish exactly
//! as in rich; they are not drawn, so markup output is plain text. The one exception is
//! [`Console::print_json`], which draws its JSON theme with SGR sequences on a colour console
//! (see [`render_spans`]). Emoji shortcodes such as `:rocket:` are replaced before the markup is
//! parsed, as rich does.

mod cells;
mod emoji;
mod emoji_data;
mod json_highlight;
mod markup;
mod rich_data;
mod sgr;
mod table;
mod text;

pub use cells::{cell_len, char_cells};
pub use markup::{MarkupError, escape};
pub use sgr::{Span, render_spans};

use json_highlight::highlight_json;
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
    /// The `NO_COLOR` variable. rich turns colour off when it is set to anything but `""`.
    pub no_color: Option<&'a str>,
    /// Whether output goes to a terminal (see [`terminal_detected`]).
    pub is_terminal: bool,
    /// Width reported by the operating system for the terminal, if there is one.
    pub terminal_width: Option<usize>,
    /// rich's `legacy_windows`: Windows, and the console does not take virtual-terminal output.
    pub legacy_windows: bool,
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
    /// The width of rich's plain `Console`, which typer's usage panels are drawn at.
    plain_width: usize,
    color: bool,
    /// `NO_COLOR` is set: attributes such as bold still draw, colours do not.
    no_color: bool,
}

impl Console {
    /// A plain console `width` cells wide.
    #[must_use]
    pub fn new(width: usize) -> Self {
        Self {
            width,
            plain_width: width,
            color: false,
            no_color: false,
        }
    }

    /// A console sized and coloured the way rich would for this environment: `COLUMNS` (digits
    /// only) beats the terminal's own width, and zero or no width means 80. A dumb terminal is 80
    /// wide with no colour; colour is on only for a terminal.
    ///
    /// On a legacy Windows console rich gives up one column (`Console.legacy_windows`). It takes
    /// that column off `COLUMNS` when the console is built, always. It takes it off the terminal's
    /// width when `size` is read, but only for a terminal, which is what `CliConsole` does; typer's
    /// plain console takes it off every width, which is [`Console::plain_width`].
    #[must_use]
    pub fn from_env(env: &ConsoleEnv<'_>) -> Self {
        let dumb = env.is_terminal
            && env
                .term
                .is_some_and(|t| matches!(t.to_lowercase().as_str(), "dumb" | "unknown"));
        if dumb {
            return Self::new(DEFAULT_WIDTH);
        }
        let legacy = usize::from(env.legacy_windows);
        let from_columns = env
            .columns
            .filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_digit()))
            .map(|c| c.parse::<usize>().unwrap_or(usize::MAX));
        let (width, plain_width) = match from_columns {
            Some(columns) => {
                let width = columns.saturating_sub(legacy).max(1);
                (width, width)
            }
            None => {
                let base = env
                    .terminal_width
                    .filter(|width| *width > 0)
                    .unwrap_or(DEFAULT_WIDTH);
                (
                    base.saturating_sub(usize::from(env.legacy_windows && env.is_terminal)),
                    base.saturating_sub(legacy),
                )
            }
        };
        Self {
            width,
            plain_width,
            color: env.is_terminal,
            no_color: env.no_color.is_some_and(|value| !value.is_empty()),
        }
    }

    /// Width in cells.
    #[must_use]
    pub fn width(&self) -> usize {
        self.width
    }

    /// The width typer's usage panels are drawn at: rich's plain console, which takes the legacy
    /// Windows column off every width, a terminal or not.
    #[must_use]
    pub fn plain_width(&self) -> usize {
        self.plain_width
    }

    /// Whether the console would colour output: only when writing to a terminal.
    #[must_use]
    pub fn color(&self) -> bool {
        self.color
    }

    /// `console.print_json(text)`: the JSON text as it is, or with rich's JSON theme drawn in SGR
    /// sequences when this console colours. Plain output is the input unchanged, so callers add
    /// the trailing newline themselves.
    #[must_use]
    pub fn print_json(&self, json: &str) -> String {
        if self.color {
            render_spans(&highlight_json(json), self.no_color)
        } else {
            json.to_owned()
        }
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

/// The columns of a console window whose left and right edges are `left` and `right` (inclusive),
/// as rich reads them from the console buffer. `None` when the window is empty or inverted.
#[must_use]
pub fn window_columns(left: i16, right: i16) -> Option<usize> {
    let columns = i32::from(right) - i32::from(left) + 1;
    usize::try_from(columns).ok().filter(|columns| *columns > 0)
}
