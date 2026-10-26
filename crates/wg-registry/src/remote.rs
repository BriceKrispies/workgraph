//! Canonical form of a git remote URL, for recognising the same repository under another URL.
//!
//! The steps of `normalize_remote_url` in `src/workgraph/integrations/git/cli.py`, applied in the
//! same order, each written out by hand instead of with the regular expressions Python uses. The
//! `golden/gitparse/remote_urls.json` vectors pin the results, including the odd ones (a port in
//! the host position becomes a path segment, a scheme in upper case is not stripped).

/// The URL's scheme prefix, `^[a-z+]+://`, removed if present.
fn strip_scheme(value: &str) -> Option<&str> {
    let at = value.find("://")?;
    let scheme = &value[..at];
    let plain = !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_lowercase() || c == '+');
    plain.then(|| &value[at + 3..])
}

/// The credentials prefix, `^[^@/]+@`, removed if present: everything up to the first `@` when
/// that text is non-empty and holds no `/`.
fn strip_credentials(value: &str) -> &str {
    match value.find('@') {
        Some(at) if at > 0 && !value[..at].contains('/') => &value[at + 1..],
        _ => value,
    }
}

/// `host:path` (scp-like) becomes `host/path`, unless the path is all digits (a port) or the colon
/// comes after the first slash. Python's `isdigit` is taken as ASCII digits here.
fn scp_colon_to_slash(value: &str) -> String {
    let (Some(colon), Some(slash)) = (value.find(':'), value.find('/')) else {
        return value.to_owned();
    };
    if colon > slash {
        return value.to_owned();
    }
    let host = &value[..colon];
    let path = &value[colon + 1..];
    let all_digits = !path.is_empty() && path.chars().all(|c| c.is_ascii_digit());
    if all_digits {
        value.to_owned()
    } else {
        format!("{host}/{path}")
    }
}

/// Removes every `:digits/` (the `re.sub(r":\d+/", "/")` of Python), scanning left to right.
fn drop_port_segments(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b':' {
            let digits_end = (i + 1..bytes.len())
                .find(|&j| !bytes[j].is_ascii_digit())
                .unwrap_or(bytes.len());
            if digits_end > i + 1 && bytes.get(digits_end) == Some(&b'/') {
                out.push('/');
                i = digits_end + 1;
                continue;
            }
        }
        // `value` is valid UTF-8 and ':' is ASCII, so byte-wise copying of the rest is sound.
        let next = value[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&value[i..i + next]);
        i += next;
    }
    out
}

/// The canonical `host/path` form of a remote URL: surrounding whitespace, the scheme, the
/// credentials and a trailing `.git` removed, and the host lower-cased.
pub fn normalize_remote_url(url: &str) -> String {
    let mut value = url.trim();
    if let Some(rest) = strip_scheme(value) {
        value = rest;
    }
    value = strip_credentials(value);
    let value = scp_colon_to_slash(value);
    let value = drop_port_segments(&value);
    let value = value.trim_end_matches('/');
    let value = value.strip_suffix(".git").unwrap_or(value);
    match value.split_once('/') {
        Some((host, rest)) if !rest.is_empty() => format!("{}/{rest}", host.to_lowercase()),
        Some((host, _)) => host.to_lowercase(),
        None => value.to_lowercase(),
    }
}
