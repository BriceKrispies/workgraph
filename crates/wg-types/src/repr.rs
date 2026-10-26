//! Python's `repr()` of a `str`, for error text that quotes a user's value the way Python does.

/// Python's `repr()` of a `str`: single quotes unless the text holds a single quote and no
/// double quote; backslashes, the chosen quote and control characters escaped.
pub fn py_repr_str(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let body: String = text
        .chars()
        .map(|c| match c {
            '\\' => "\\\\".to_owned(),
            '\n' => "\\n".to_owned(),
            '\r' => "\\r".to_owned(),
            '\t' => "\\t".to_owned(),
            c if c == quote => format!("\\{c}"),
            c if u32::from(c) < 0x20 || (0x7f..=0xa0).contains(&u32::from(c)) => {
                format!("\\x{:02x}", u32::from(c))
            }
            c => c.to_string(),
        })
        .collect();
    format!("{quote}{body}{quote}")
}

#[cfg(test)]
mod tests {
    use super::py_repr_str;

    #[test]
    fn single_quotes_unless_the_text_holds_one() {
        assert_eq!(py_repr_str("root:everything"), "'root:everything'");
        assert_eq!(py_repr_str("it's"), "\"it's\"");
        assert_eq!(py_repr_str("say \"hi\" it's"), "'say \"hi\" it\\'s'");
    }

    #[test]
    fn escapes_backslashes_and_control_characters() {
        assert_eq!(py_repr_str("a\\b\nc\td"), "'a\\\\b\\nc\\td'");
        assert_eq!(py_repr_str("\u{1b}"), "'\\x1b'");
    }
}
