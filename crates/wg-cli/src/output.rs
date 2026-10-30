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
    let no_color = var("NO_COLOR");
    let is_terminal = terminal_detected(
        isatty,
        var("TTY_COMPATIBLE").as_deref(),
        var("FORCE_COLOR").as_deref(),
    );
    Console::from_env(&ConsoleEnv {
        columns: columns.as_deref(),
        term: term.as_deref(),
        no_color: no_color.as_deref(),
        is_terminal,
        terminal_width: terminal_columns(),
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

/// The width of the console rich reads when `COLUMNS` is not set (`Console.size`). On Windows
/// rich asks standard output, then standard error, for the window of the console behind it, so a
/// redirected standard output still takes the width of a console on standard error. `None` when
/// neither is a console; off Windows the terminal size is not read here, so the default applies.
#[must_use]
pub fn terminal_columns() -> Option<usize> {
    #[cfg(not(windows))]
    {
        None
    }
    #[cfg(windows)]
    {
        /// `STD_OUTPUT_HANDLE` is `(DWORD)-11` and `STD_ERROR_HANDLE` is `(DWORD)-12`.
        const HANDLES: [u32; 2] = [u32::MAX - 10, u32::MAX - 11];
        HANDLES
            .iter()
            .find_map(|&id| console_window(id))
            .and_then(|[left, right]| wg_render::window_columns(left, right))
    }
}

/// The left and right columns of the console window behind standard handle `id`, when it is a
/// console (`GetConsoleScreenBufferInfo`).
#[cfg(windows)]
fn console_window(id: u32) -> Option<[i16; 2]> {
    use std::ffi::c_void;

    /// `CONSOLE_SCREEN_BUFFER_INFO`, as the console API lays it out.
    #[repr(C)]
    #[derive(Default)]
    struct ScreenBufferInfo {
        size: [i16; 2],
        cursor: [i16; 2],
        attributes: u16,
        window: [i16; 4],
        maximum: [i16; 2],
    }
    unsafe extern "system" {
        fn GetStdHandle(id: u32) -> *mut c_void;
        fn GetConsoleScreenBufferInfo(console: *mut c_void, info: *mut ScreenBufferInfo) -> i32;
    }
    let mut info = ScreenBufferInfo::default();
    // SAFETY: `GetStdHandle` only reads its argument; `GetConsoleScreenBufferInfo` writes one
    // buffer-info structure through a pointer to a live local. A handle that is not a console
    // makes it fail, which is the `None` answer.
    let ok = unsafe { GetConsoleScreenBufferInfo(GetStdHandle(id), &raw mut info) != 0 };
    ok.then_some([info.window[0], info.window[2]])
}
