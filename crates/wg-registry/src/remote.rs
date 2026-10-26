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
    // Each piece after a colon starts a candidate: a run of digits followed by `/` is a port.
    value
        .split(':')
        .enumerate()
        .map(|(index, piece)| {
            if index == 0 {
                return piece.to_owned();
            }
            let digits = piece.bytes().take_while(u8::is_ascii_digit).count();
            match piece.as_bytes().get(digits) {
                Some(b'/') if digits > 0 => format!("/{}", &piece[digits + 1..]),
                _ => format!(":{piece}"),
            }
        })
        .collect()
}

/// The canonical `host/path` form of a remote URL: surrounding whitespace, the scheme, the
/// credentials and a trailing `.git` removed, and the host lower-cased.
pub fn normalize_remote_url(url: &str) -> String {
    let trimmed = url.trim();
    let value = strip_scheme(trimmed).unwrap_or(trimmed);
    let value = drop_port_segments(&scp_colon_to_slash(strip_credentials(value)));
    let value = value.trim_end_matches('/');
    let value = value.strip_suffix(".git").unwrap_or(value);
    match value.split_once('/') {
        Some((host, rest)) if !rest.is_empty() => format!("{}/{rest}", host.to_lowercase()),
        Some((host, _)) => host.to_lowercase(),
        None => value.to_lowercase(),
    }
}
