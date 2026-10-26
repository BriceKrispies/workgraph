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
        legacy_windows: legacy_windows(),
    })
}

/// rich's `detect_legacy_windows`: on Windows, standard output is not a console that accepts
/// virtual-terminal sequences. `GetConsoleMode` fails for a pipe or file, and a legacy console
/// lacks `ENABLE_VIRTUAL_TERMINAL_PROCESSING`. Off Windows rich never takes the legacy path.
fn legacy_windows() -> bool {
    #[cfg(not(windows))]
    {
        false
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        /// `STD_OUTPUT_HANDLE` is `(DWORD)-11`.
        const STD_OUTPUT_HANDLE: u32 = u32::MAX - 10;
        const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
        unsafe extern "system" {
            fn GetStdHandle(id: u32) -> *mut c_void;
            fn GetConsoleMode(console: *mut c_void, mode: *mut u32) -> i32;
        }
        let mut mode = 0_u32;
        // SAFETY: `GetStdHandle` only reads its argument; `GetConsoleMode` writes one `u32`
        // through a pointer to a live local. A bad handle makes it fail, the legacy answer.
        let vt = unsafe {
            GetConsoleMode(GetStdHandle(STD_OUTPUT_HANDLE), &raw mut mode) != 0
                && mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0
        };
        !vt
    }
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
