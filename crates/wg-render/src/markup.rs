//! Console markup, ported from `rich.markup`.
//!
//! Only the plain text matters: nothing here emits styles, so a tag is parsed, matched with its
//! closing tag and dropped, exactly as rich drops tags it cannot style. What the Python code
//! reports as `MarkupError` is reported as [`MarkupError`].

use std::fmt;

use crate::rich_data::COLOR_NAMES;

/// Malformed markup: a closing tag with nothing to close.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkupError {
    message: String,
}

impl MarkupError {
    /// The message exactly as Python's `str(MarkupError(...))` gives it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for MarkupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for MarkupError {}

/// Python `str.isspace` for one character.
#[must_use]
pub(crate) fn py_is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

fn is_tag_start(c: char) -> bool {
    c.is_ascii_lowercase() || c == '#' || c == '/' || c == '@'
}

/// One tag found by the scanner: the backslashes before it and the text between the brackets.
struct Found {
    start: usize,
    end: usize,
    escapes: usize,
    /// Range of the text between `[` and `]`.
    tag: (usize, usize),
}

/// Leftmost match of `((\\*)\[([a-z#/@][^[]*?)])` at or after `from`.
fn find_tag(markup: &[char], from: usize) -> Option<Found> {
    let n = markup.len();
    let mut i = from;
    while i < n {
        let mut j = i;
        while j < n && markup[j] == '\\' {
            j += 1;
        }
        if j + 1 < n && markup[j] == '[' && is_tag_start(markup[j + 1]) {
            let mut k = j + 2;
            while k < n && markup[k] != '[' {
                if markup[k] == ']' {
                    return Some(Found {
                        start: i,
                        end: k + 1,
                        escapes: j - i,
                        tag: (j + 1, k),
                    });
                }
                k += 1;
            }
        }
        i += 1;
    }
    None
}

enum Piece {
    Text(String),
    Tag {
        name: String,
        parameters: Option<String>,
    },
}

/// `rich.markup._parse`: pieces with the position each starts at.
fn parse(markup: &[char]) -> Vec<(usize, Piece)> {
    let mut out = Vec::new();
    let mut position = 0;
    let mut from = 0;
    while let Some(found) = find_tag(markup, from) {
        let Found {
            mut start,
            end,
            escapes,
            tag,
        } = found;
        from = end;
        if start > position {
            out.push((start, Piece::Text(markup[position..start].iter().collect())));
        }
        if escapes > 0 {
            let backslashes = escapes / 2;
            let escaped = escapes % 2 == 1;
            if backslashes > 0 {
                out.push((start, Piece::Text("\\".repeat(backslashes))));
                start += backslashes * 2;
            }
            if escaped {
                // The escaped tag keeps its brackets: the text from `[` to `]`.
                out.push((start, Piece::Text(markup[tag.0 - 1..end].iter().collect())));
                position = end;
                continue;
            }
        }
        let tag_text: String = markup[tag.0..tag.1].iter().collect();
        let (name, parameters) = match tag_text.split_once('=') {
            Some((name, parameters)) => (name.to_owned(), Some(parameters.to_owned())),
            None => (tag_text, None),
        };
        out.push((start, Piece::Tag { name, parameters }));
        position = end;
    }
    if position < markup.len() {
        out.push((position, Piece::Text(markup[position..].iter().collect())));
    }
    out
}

/// `rich.markup.escape`: backslash-escapes anything that would read as a tag.
#[must_use]
pub fn escape(markup: &str) -> String {
    let chars: Vec<char> = markup.chars().collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        // `(\\*)(\[[a-z#/@][^[]*?])`
        let mut j = i;
        while j < n && chars[j] == '\\' {
            j += 1;
        }
        let mut matched_end = None;
        if j + 1 < n && chars[j] == '[' && is_tag_start(chars[j + 1]) {
            let mut k = j + 2;
            while k < n && chars[k] != '[' {
                if chars[k] == ']' {
                    matched_end = Some(k + 1);
                    break;
                }
                k += 1;
            }
        }
        if let Some(end) = matched_end {
            let backslashes: String = chars[i..j].iter().collect();
            out.push_str(&backslashes);
            out.push_str(&backslashes);
            out.push('\\');
            out.extend(chars[j..end].iter());
            i = end;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    if out.ends_with('\\') && !out.ends_with("\\\\") {
        out.push('\\');
    }
    out
}

/// Renders markup to its plain text, or fails as `rich.markup.render` does.
///
/// Emoji codes (`:smile:`) are left as they are; rich would replace them.
///
/// # Errors
///
/// A closing tag that matches no open tag, or `[/]` with nothing open.
pub fn render_plain(markup: &str) -> Result<Vec<char>, MarkupError> {
    let chars: Vec<char> = markup.chars().collect();
    if !chars.contains(&'[') {
        return Ok(chars);
    }
    let mut text: Vec<char> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    for (position, piece) in parse(&chars) {
        match piece {
            Piece::Text(plain) => text.extend(plain.replace("\\[", "[").chars()),
            Piece::Tag { name, parameters } => {
                if let Some(closing) = name.strip_prefix('/') {
                    let style_name = closing.trim_matches(py_is_space);
                    if style_name.is_empty() {
                        if stack.pop().is_none() {
                            return Err(MarkupError {
                                message: format!(
                                    "closing tag '[/]' at position {position} has nothing to close"
                                ),
                            });
                        }
                    } else {
                        let wanted = normalize_style(style_name);
                        let Some(index) = stack.iter().rposition(|open| *open == wanted) else {
                            let shown = match &parameters {
                                None => format!("[{name}]"),
                                Some(parameters) => format!("[{name}={parameters}]"),
                            };
                            return Err(MarkupError {
                                message: format!(
                                    "closing tag '{shown}' at position {position} doesn't match any open tag"
                                ),
                            });
                        };
                        stack.remove(index);
                    }
                } else {
                    stack.push(normalize_style(&name));
                }
            }
        }
    }
    Ok(text)
}

/// `Style.normalize`: the canonical spelling of a style definition, so that equal styles compare
/// equal. An unparsable definition normalises to itself, stripped and lower-cased.
fn normalize_style(definition: &str) -> String {
    parse_style(definition).unwrap_or_else(|| definition.trim_matches(py_is_space).to_lowercase())
}

/// Attribute names in `Style.__str__` order, with the words that select each.
const ATTRIBUTES: [(&str, &[&str]); 13] = [
    ("bold", &["bold", "b"]),
    ("dim", &["dim", "d"]),
    ("italic", &["italic", "i"]),
    ("underline", &["underline", "u"]),
    ("blink", &["blink"]),
    ("blink2", &["blink2"]),
    ("reverse", &["reverse", "r"]),
    ("conceal", &["conceal", "c"]),
    ("strike", &["strike", "s"]),
    ("underline2", &["underline2", "uu"]),
    ("frame", &["frame"]),
    ("encircle", &["encircle"]),
    ("overline", &["overline", "o"]),
];

fn attribute_index(word: &str) -> Option<usize> {
    ATTRIBUTES
        .iter()
        .position(|(_, words)| words.contains(&word))
}

fn is_color(word: &str) -> bool {
    if word == "default" || COLOR_NAMES.binary_search(&word).is_ok() {
        return true;
    }
    if let Some(hex) = word.strip_prefix('#') {
        return hex.len() == 6 && hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'));
    }
    if let Some(number) = word
        .strip_prefix("color(")
        .and_then(|r| r.strip_suffix(')'))
    {
        return (1..=3).contains(&number.len())
            && number.chars().all(|c| c.is_ascii_digit())
            && number.parse::<u32>().is_ok_and(|n| n <= 255);
    }
    if let Some(inner) = word.strip_prefix("rgb(").and_then(|r| r.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').collect();
        return parts.len() == 3
            && parts
                .iter()
                .all(|p| p.parse::<u32>().is_ok_and(|n| n <= 255));
    }
    false
}

/// `str(Style.parse(definition))`, or `None` where rich raises `StyleSyntaxError`.
fn parse_style(definition: &str) -> Option<String> {
    if definition.trim_matches(py_is_space) == "none" || definition.is_empty() {
        return Some("none".to_owned());
    }
    let mut color: Option<String> = None;
    let mut bgcolor: Option<String> = None;
    let mut link: Option<String> = None;
    let mut attributes: [Option<bool>; 13] = [None; 13];
    let lowered = definition.to_lowercase();
    let mut words = lowered.split(py_is_space).filter(|w| !w.is_empty());
    while let Some(word) = words.next() {
        match word {
            "on" => {
                let next = words.next()?;
                if !is_color(next) {
                    return None;
                }
                bgcolor = Some(next.to_owned());
            }
            "not" => {
                let index = attribute_index(words.next()?)?;
                attributes[index] = Some(false);
            }
            "link" => link = Some(words.next()?.to_owned()),
            _ => {
                if let Some(index) = attribute_index(word) {
                    attributes[index] = Some(true);
                } else if is_color(word) {
                    color = Some(word.to_owned());
                } else {
                    return None;
                }
            }
        }
    }
    let mut parts: Vec<String> = Vec::new();
    for ((name, _), value) in ATTRIBUTES.iter().zip(attributes) {
        match value {
            Some(true) => parts.push((*name).to_owned()),
            Some(false) => parts.push(format!("not {name}")),
            None => {}
        }
    }
    parts.extend(color);
    if let Some(bg) = bgcolor {
        parts.push("on".to_owned());
        parts.push(bg);
    }
    if let Some(url) = link {
        parts.push("link".to_owned());
        parts.push(url);
    }
    if parts.is_empty() {
        Some("none".to_owned())
    } else {
        Some(parts.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(markup: &str) -> String {
        render_plain(markup)
            .map(|c| c.into_iter().collect())
            .unwrap_or_default()
    }

    #[test]
    fn tags_are_dropped() {
        assert_eq!(plain("[bold]hi[/bold] [red]there"), "hi there");
        assert_eq!(plain("\\[escaped]"), "[escaped]");
        assert_eq!(plain("[[double]]"), "[]");
        assert_eq!(plain("[b]short[/b]"), "short");
    }

    #[test]
    fn unmatched_close_reports_position() {
        let err = render_plain("x [/bold] y").unwrap_err();
        assert_eq!(
            err.message(),
            "closing tag '[/bold]' at position 2 doesn't match any open tag"
        );
        let err = render_plain("[/]").unwrap_err();
        assert_eq!(
            err.message(),
            "closing tag '[/]' at position 0 has nothing to close"
        );
    }

    #[test]
    fn escape_round_trips() {
        assert_eq!(escape("[red]x"), "\\[red]x");
        assert_eq!(plain(&escape("[red]x")), "[red]x");
        assert_eq!(escape("a\\"), "a\\\\");
    }

    #[test]
    fn style_normalisation() {
        assert_eq!(normalize_style("b"), "bold");
        assert_eq!(normalize_style("red on blue"), "red on blue");
        assert_eq!(normalize_style("Not A Style"), "not a style");
    }
}
