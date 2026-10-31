//! Text helpers: the Python-shaped snippets the messages quote, and percent-encoding for query
//! strings.

/// A single-quoted string as Python's `repr` shows plain text (`'name'`).
pub fn py_repr(text: &str) -> String {
    format!("'{text}'")
}

/// Percent-encodes every byte outside the RFC 3986 unreserved set, as `urllib.parse.quote` with
/// `safe=''` does.
pub fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// A transport failure that carries an operating-system error, as Python's `httpx` prints it:
/// `[WinError 10061] No connection ...` on Windows, `[Errno 111] Connection refused` elsewhere.
/// `shown` is the error's own text, which may end in Rust's ` (os error N)` suffix; that suffix
/// and any trailing full stop are dropped.
pub fn os_transport_message(windows: bool, code: i32, shown: &str) -> String {
    let suffix = format!(" (os error {code})");
    let reason = shown
        .strip_suffix(&suffix)
        .unwrap_or(shown)
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    let tag = if windows { "WinError" } else { "Errno" };
    format!("[{tag} {code}] {reason}")
}
