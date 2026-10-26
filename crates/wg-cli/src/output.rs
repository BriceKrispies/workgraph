//! Streams and the rich-compatible console.

use std::io::{IsTerminal, Write};

use wg_render::{Console, ConsoleEnv, terminal_detected};

/// The console `rich` would build for standard output (`stderr` false) or standard error: width
/// from `COLUMNS`, colour only for a terminal (and `FORCE_COLOR`, `TTY_COMPATIBLE`).
#[must_use]
pub fn console(stderr: bool) -> Console {
    let var = |key: &str| std::env::var(key).ok();
    let isatty = if stderr {
        std::io::stderr().is_terminal()
    } else {
        std::io::stdout().is_terminal()
    };
    let columns = var("COLUMNS");
    let term = var("TERM");
    let is_terminal = terminal_detected(
        isatty,
        var("TTY_COMPATIBLE").as_deref(),
        var("FORCE_COLOR").as_deref(),
    );
    Console::from_env(&ConsoleEnv {
        columns: columns.as_deref(),
        term: term.as_deref(),
        is_terminal,
        terminal_width: None,
    })
}

/// Writes `text` and flushes. A closed pipe (`workgraph ... | head`) is not an error.
fn write_all(stream: &mut dyn Write, text: &str) {
    if text.is_empty() {
        return;
    }
    let _ = stream.write_all(text.as_bytes());
    let _ = stream.flush();
}

/// Writes `text` to standard output.
pub fn write_stdout(text: &str) {
    write_all(&mut std::io::stdout().lock(), text);
}

/// Writes `text` to standard error.
pub fn write_stderr(text: &str) {
    write_all(&mut std::io::stderr().lock(), text);
}

/// Writes `text` to `stream`, ignoring a closed pipe.
pub(crate) fn write_to(stream: &mut dyn Write, text: &str) {
    write_all(stream, text);
}
