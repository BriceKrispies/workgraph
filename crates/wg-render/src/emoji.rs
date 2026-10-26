//! Emoji shortcodes, ported from `rich._emoji_replace`.
//!
//! rich runs `re.sub(r":(\S*?)(?:-(emoji|text))?:")` over the markup before it parses the tags, so
//! `:rocket:` becomes the rocket and `:thumbs_up-text:` gets the text-presentation selector. A
//! name is replaced only when rich knows it (after lower-casing); anything else is left as typed.

use crate::emoji_data::EMOJI;
use crate::markup::py_is_space;

/// U+FE0F, rich's `emoji` variant selector.
const VARIANT_EMOJI: &str = "\u{fe0f}";
/// U+FE0E, rich's `text` variant selector.
const VARIANT_TEXT: &str = "\u{fe0e}";

/// One shortcode match: where it ends (exclusive), its name, and the variant suffix if any.
struct Shortcode<'a> {
    end: usize,
    name: &'a [char],
    variant: Option<&'a str>,
}

/// Matches `:(\S*?)(?:-(emoji|text))?:` at `start`, which must be a colon. The name is the
/// shortest run of non-space characters that is followed by a closing colon, optionally after
/// a `-emoji` or `-text` suffix, exactly as the lazy regex finds it.
fn match_at(chars: &[char], start: usize) -> Option<Shortcode<'_>> {
    let name_start = start + 1;
    let mut q = name_start;
    loop {
        let rest = &chars[q..];
        if let Some(variant) = suffix_variant(rest) {
            let end = q + variant.1;
            return Some(Shortcode {
                end,
                name: &chars[name_start..q],
                variant: Some(variant.0),
            });
        }
        if rest.first() == Some(&':') {
            return Some(Shortcode {
                end: q + 1,
                name: &chars[name_start..q],
                variant: None,
            });
        }
        match rest.first() {
            Some(&c) if !py_is_space(c) => q += 1,
            _ => return None,
        }
    }
}

/// `-emoji:` or `-text:` at the start of `rest`: the variant name and the length consumed.
fn suffix_variant(rest: &[char]) -> Option<(&'static str, usize)> {
    let head: String = rest.iter().take(7).collect();
    if head.starts_with("-emoji:") {
        Some(("emoji", 7))
    } else if head.starts_with("-text:") {
        Some(("text", 6))
    } else {
        None
    }
}

/// The replacement for a shortcode name, or `None` when rich does not know it.
fn lookup(name: &[char]) -> Option<&'static str> {
    let lower: String = name.iter().collect::<String>().to_lowercase();
    EMOJI
        .binary_search_by_key(&lower.as_str(), |(code, _)| *code)
        .ok()
        .map(|index| EMOJI[index].1)
}

/// `rich._emoji_replace._emoji_replace` with no default variant: every known shortcode becomes
/// its emoji, and every other colon-delimited run is copied unchanged.
#[must_use]
pub(crate) fn replace(text: &str) -> String {
    if !text.contains(':') {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let shortcode = if chars[i] == ':' {
            match_at(&chars, i)
        } else {
            None
        };
        if let Some(shortcode) = shortcode {
            match lookup(shortcode.name) {
                Some(emoji) => {
                    out.push_str(emoji);
                    out.push_str(match shortcode.variant {
                        Some("emoji") => VARIANT_EMOJI,
                        Some(_) => VARIANT_TEXT,
                        None => "",
                    });
                }
                None => out.extend(&chars[i..shortcode.end]),
            }
            i = shortcode.end;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}
