//! Python text helpers the server's messages and pages use: `repr(str)`, `html.escape` and
//! `urllib.parse.unquote`.

/// `repr(text)` for a `str`: single quotes unless the text holds a `'` and no `"`; backslash,
/// the quote, `\t`, `\n`, `\r` escaped, other control characters as `\xNN`.
pub fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let body: String = text
        .chars()
        .map(|ch| match ch {
            '\\' => "\\\\".to_owned(),
            '\t' => "\\t".to_owned(),
            '\n' => "\\n".to_owned(),
            '\r' => "\\r".to_owned(),
            c if c == quote => format!("\\{c}"),
            c if (c as u32) < 0x20 || (0x7f..=0xa0).contains(&(c as u32)) => {
                format!("\\x{:02x}", c as u32)
            }
            c => c.to_string(),
        })
        .collect();
    format!("{quote}{body}{quote}")
}

/// `html.escape(text, quote=True)`.
pub fn html_escape(text: &str) -> String {
    text.chars()
        .map(|ch| match ch {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&#x27;".to_owned(),
            c => c.to_string(),
        })
        .collect()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// `urllib.parse.unquote(text)`: `%XX` sequences decoded as UTF-8 with replacement characters for
/// invalid bytes; a `%` not followed by two hex digits stays as it is.
pub fn unquote(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| {
                let high = hex_value(*bytes.get(index + 1)?)?;
                let low = hex_value(*bytes.get(index + 2)?)?;
                Some(high * 16 + low)
            })
            .flatten();
        match escaped {
            Some(byte) => {
                out.push(byte);
                index += 3;
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repr_like_python() {
        assert_eq!(py_repr("evil.example"), "'evil.example'");
        assert_eq!(py_repr(""), "''");
        assert_eq!(py_repr("it's"), "\"it's\"");
        assert_eq!(py_repr("a'b\"c"), "'a\\'b\"c'");
        assert_eq!(py_repr("tab\there\u{1}"), "'tab\\there\\x01'");
    }

    #[test]
    fn escapes_html() {
        assert_eq!(
            html_escape("<a href='x'>&\"</a>"),
            "&lt;a href=&#x27;x&#x27;&gt;&amp;&quot;&lt;/a&gt;"
        );
    }

    #[test]
    fn unquotes() {
        assert_eq!(unquote("/%61pi/v1"), "/api/v1");
        assert_eq!(unquote("100%"), "100%");
        assert_eq!(unquote("%zz%e2%9c%93"), "%zz\u{2713}");
        assert_eq!(unquote("%ff"), "\u{fffd}");
    }
}
