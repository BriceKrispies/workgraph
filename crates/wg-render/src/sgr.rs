//! SGR styling: the escape sequences rich writes for a styled run of text on a colour terminal.
//!
//! Used only where Python colours its output (`config show`, rich's `print_json`). Everywhere
//! else the crate's output stays plain text. A style is a rich style definition such as
//! `"bold blue"`; it is parsed with the markup parser's style rules, so the words accepted here
//! are the ones rich accepts.

use crate::markup;

/// One run of text and the rich style it is drawn in. An empty style draws it plain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span<'a> {
    /// The text as written.
    pub text: &'a str,
    /// A rich style definition, or `""` for none.
    pub style: &'a str,
}

/// SGR parameter for each attribute, in rich's order (`Style._style_map`).
const ATTRIBUTE_CODES: [u8; 13] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 21, 51, 52, 53];

/// The eight standard colour names, in ANSI order.
const BASIC_COLORS: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// The SGR parameter for a named ANSI colour (`red`, `bright_red`), foreground or background.
/// Other colour forms (numbered, hex, `default`) have no code here and draw in the default colour.
fn colour_code(word: &str, background: bool) -> Option<u8> {
    let (bright, base) = match word.strip_prefix("bright_") {
        Some(rest) => (true, rest),
        None => (false, word),
    };
    let index = BASIC_COLORS.iter().position(|name| *name == base)?;
    let index = u8::try_from(index).ok()?;
    let offset = match (bright, background) {
        (false, false) => 30,
        (false, true) => 40,
        (true, false) => 90,
        (true, true) => 100,
    };
    Some(offset + index)
}

/// The SGR parameters (`"1;34"`) for a style definition, or `""` when it draws plain.
///
/// Attributes come first in rich's order, then the foreground, then the background. With
/// `no_color`, colour codes are dropped and attributes kept, as rich's `Segment.remove_color`
/// does. Words this layer cannot express (`not`, `link`, `default`, numbered colours) are skipped.
fn sgr_parameters(definition: &str, no_color: bool) -> String {
    let Some(normalised) = markup::parse_style(definition) else {
        return String::new();
    };
    let mut codes: Vec<String> = Vec::new();
    let mut words = normalised.split(' ').filter(|word| !word.is_empty());
    while let Some(word) = words.next() {
        if let Some(index) = markup::attribute_index(word) {
            codes.push(ATTRIBUTE_CODES[index].to_string());
        } else if word == "not" || word == "link" {
            // Attribute-off codes and hyperlinks are not written here: skip the operand.
            let _ = words.next();
        } else if word == "on" {
            let colour = words.next().and_then(|name| colour_code(name, true));
            if let (false, Some(code)) = (no_color, colour) {
                codes.push(code.to_string());
            }
        } else if let (false, Some(code)) = (no_color, colour_code(word, false)) {
            codes.push(code.to_string());
        }
    }
    codes.join(";")
}

/// `Console` output of styled spans on a colour terminal: each span with codes is wrapped in
/// `ESC[<codes>m` and `ESC[0m` (rich's `Style.render`); a span without codes is written as is.
/// `no_color` keeps the attributes and drops the colours (`NO_COLOR`).
#[must_use]
pub fn render_spans(spans: &[Span<'_>], no_color: bool) -> String {
    let mut out = String::new();
    for span in spans {
        let codes = sgr_parameters(span.style, no_color);
        if span.text.is_empty() || codes.is_empty() {
            out.push_str(span.text);
        } else {
            out.push_str("\x1b[");
            out.push_str(&codes);
            out.push('m');
            out.push_str(span.text);
            out.push_str("\x1b[0m");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str, style: &str) -> String {
        render_spans(&[Span { text, style }], false)
    }

    #[test]
    fn attributes_then_colours_in_rich_order() {
        assert_eq!(sgr_parameters("bold blue", false), "1;34");
        assert_eq!(sgr_parameters("blue bold", false), "1;34");
        assert_eq!(sgr_parameters("italic bright_green", false), "3;92");
        assert_eq!(sgr_parameters("bold cyan", false), "1;36");
        assert_eq!(sgr_parameters("red on white", false), "31;47");
        assert_eq!(sgr_parameters("", false), "");
        assert_eq!(sgr_parameters("not a style", false), "");
    }

    #[test]
    fn spans_wrap_only_when_styled() {
        assert_eq!(one("x", "green"), "\x1b[32mx\x1b[0m");
        assert_eq!(one("x", ""), "x");
        assert_eq!(one("", "green"), "");
    }

    #[test]
    fn no_color_keeps_attributes_only() {
        let spans = [
            Span {
                text: "\"lan\"",
                style: "bold blue",
            },
            Span {
                text: "true",
                style: "italic bright_green",
            },
            Span {
                text: "\"x\"",
                style: "green",
            },
        ];
        assert_eq!(
            render_spans(&spans, true),
            "\x1b[1m\"lan\"\x1b[0m\x1b[3mtrue\x1b[0m\"x\""
        );
    }
}
