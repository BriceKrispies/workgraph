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
