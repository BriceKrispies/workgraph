//! Word wrapping and justification, ported from `rich._wrap` and `rich.text.Text.wrap`.
//!
//! Lines are `Vec<char>`; widths are terminal cells.

use crate::cells::{cell_len, chop_cells, set_cell_size};
use crate::markup::py_is_space;

/// How wrapped lines are aligned inside their width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Justify {
    /// rich's `"default"`: lines are left as wrapped, with no padding.
    Default,
    /// Lines are cropped and padded on the right to the width.
    Left,
    /// Lines are stripped, cropped and padded on both sides.
    Center,
}

/// `str.splitlines()` terminators.
fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\u{b}'
            | '\u{c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// `str.splitlines()`.
fn split_lines(text: &[char]) -> Vec<&[char]> {
    let mut lines = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < text.len() {
        if is_line_break(text[i]) {
            lines.push(&text[start..i]);
            if text[i] == '\r' && text.get(i + 1) == Some(&'\n') {
                i += 1;
            }
            start = i + 1;
        }
        i += 1;
    }
    if start < text.len() {
        lines.push(&text[start..]);
    }
    lines
}

/// `str.split()` with no argument.
fn split_words(text: &[char]) -> Vec<&[char]> {
    text.split(|&c| py_is_space(c))
        .filter(|w| !w.is_empty())
        .collect()
}

/// `Text.__rich_measure__`: the narrowest and widest the text can be drawn.
pub(crate) fn measure_text(text: &[char]) -> (usize, usize) {
    let max_width = split_lines(text)
        .iter()
        .map(|line| cell_len(line))
        .max()
        .unwrap_or(0);
    let min_width = split_words(text)
        .iter()
        .map(|word| cell_len(word))
        .max()
        .unwrap_or(max_width);
    (min_width, max_width)
}

/// Trims trailing whitespace the way `str.rstrip()` does.
fn rstrip(text: &[char]) -> &[char] {
    let end = text
        .iter()
        .rposition(|&c| !py_is_space(c))
        .map_or(0, |i| i + 1);
    &text[..end]
}

/// Words of `\s*\S+\s*` from the start of `text`, as `(start, end)` offsets.
fn words(text: &[char]) -> Vec<(usize, usize)> {
    let n = text.len();
    let mut out = Vec::new();
    let mut position = 0;
    loop {
        let mut i = position;
        while i < n && py_is_space(text[i]) {
            i += 1;
        }
        let word_start = i;
        while i < n && !py_is_space(text[i]) {
            i += 1;
        }
        if i == word_start {
            return out;
        }
        while i < n && py_is_space(text[i]) {
            i += 1;
        }
        out.push((position, i));
        position = i;
    }
}

/// `rich._wrap.divide_line` with folding: offsets where the line must break.
fn divide_line(text: &[char], width: usize) -> Vec<usize> {
    let mut breaks = Vec::new();
    let mut cell_offset: usize = 0;
    for (start, end) in words(text) {
        let word = &text[start..end];
        let word_length = cell_len(rstrip(word));
        let fits = width >= cell_offset && width - cell_offset >= word_length;
        if fits {
            cell_offset += cell_len(word);
        } else if word_length > width {
            let pieces = chop_cells(word, width);
            let count = pieces.len();
            let mut position = start;
            for (index, piece) in pieces.iter().enumerate() {
                if position > 0 {
                    breaks.push(position);
                }
                if index + 1 == count {
                    cell_offset = cell_len(piece);
                } else {
                    position += piece.len();
                }
            }
        } else if cell_offset > 0 && start > 0 {
            breaks.push(start);
            cell_offset = cell_len(word);
        }
    }
    breaks
}

/// `Text.divide`: cuts the line at the given offsets.
fn divide(text: &[char], offsets: &[usize]) -> Vec<Vec<char>> {
    let mut bounds = vec![0];
    bounds.extend_from_slice(offsets);
    bounds.push(text.len());
    bounds
        .windows(2)
        .map(|pair| {
            let (from, to) = (pair[0].min(text.len()), pair[1].min(text.len()));
            if from < to {
                text[from..to].to_vec()
            } else {
                Vec::new()
            }
        })
        .collect()
}

/// `Text.rstrip_end`: removes whitespace past `size` characters from the end.
fn rstrip_end(line: &[char], size: usize) -> Vec<char> {
    if line.len() > size {
        let excess = line.len() - size;
        let trailing = line.len() - rstrip(line).len();
        if trailing > 0 {
            return line[..line.len() - trailing.min(excess)].to_vec();
        }
    }
    line.to_vec()
}

/// `Text.truncate` with the `"fold"` overflow, which crops.
fn truncate(line: Vec<char>, width: usize, pad: bool) -> Vec<char> {
    let length = cell_len(&line);
    let mut line = if length > width {
        set_cell_size(&line, width)
    } else {
        line
    };
    if pad && length < width {
        line.resize(line.len() + (width - length), ' ');
    }
    line
}

/// `Text.expand_tabs` for one line with tab stops every `tab_size` cells.
fn expand_tabs(line: &[char], tab_size: usize) -> Vec<char> {
    let mut out: Vec<char> = Vec::with_capacity(line.len());
    let mut cell_position = 0;
    let mut part_start = 0;
    for (i, &c) in line.iter().enumerate() {
        if c != '\t' {
            continue;
        }
        let mut part: Vec<char> = line[part_start..i].to_vec();
        part.push(' ');
        cell_position += cell_len(&part);
        let remainder = cell_position % tab_size;
        if remainder != 0 {
            let spaces = tab_size - remainder;
            part.resize(part.len() + spaces, ' ');
            cell_position += spaces;
        }
        out.extend(part);
        part_start = i + 1;
    }
    out.extend_from_slice(&line[part_start..]);
    out
}

fn justify_line(line: Vec<char>, width: usize, justify: Justify) -> Vec<char> {
    match justify {
        Justify::Default => line,
        Justify::Left => truncate(line, width, true),
        Justify::Center => {
            let stripped = rstrip(&line).to_vec();
            let mut line = truncate(stripped, width, false);
            let left = width.saturating_sub(cell_len(&line)) / 2;
            let mut padded = vec![' '; left];
            padded.append(&mut line);
            let right = width.saturating_sub(cell_len(&padded));
            padded.resize(padded.len() + right, ' ');
            padded
        }
    }
}

/// `Text.wrap` for plain text with the `"fold"` overflow: wraps to `width` cells.
pub(crate) fn wrap(text: &[char], width: usize, justify: Justify) -> Vec<Vec<char>> {
    let mut lines = Vec::new();
    for raw in text.split(|&c| c == '\n') {
        let line = if raw.contains(&'\t') {
            expand_tabs(raw, 8)
        } else {
            raw.to_vec()
        };
        let offsets = divide_line(&line, width);
        for piece in divide(&line, &offsets) {
            let piece = rstrip_end(&piece, width);
            let piece = justify_line(piece, width, justify);
            lines.push(truncate(piece, width, false));
        }
    }
    lines
}

/// Crops a line to `width` cells (`Segment.adjust_line_length` without padding).
pub(crate) fn crop(line: &[char], width: usize) -> Vec<char> {
    if cell_len(line) > width {
        set_cell_size(line, width)
    } else {
        line.to_vec()
    }
}

/// Crops or pads a line to exactly `width` cells (`Segment.adjust_line_length`).
pub(crate) fn fit(line: &[char], width: usize) -> Vec<char> {
    let mut out = crop(line, width);
    let length = cell_len(&out);
    if length < width {
        out.resize(out.len() + (width - length), ' ');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    fn wrapped(s: &str, width: usize) -> Vec<String> {
        wrap(&chars(s), width, Justify::Default)
            .into_iter()
            .map(|l| l.into_iter().collect())
            .collect()
    }

    #[test]
    fn wraps_on_words_and_keeps_trailing_space() {
        assert_eq!(
            wrapped("title must not be empty", 12),
            vec!["title must ", "not be empty"]
        );
    }

    #[test]
    fn folds_long_words() {
        assert_eq!(wrapped("xxxxxxxxxx", 4), vec!["xxxx", "xxxx", "xx"]);
    }

    #[test]
    fn blank_lines_survive() {
        assert_eq!(wrapped("a\n\nb", 10), vec!["a", "", "b"]);
    }

    #[test]
    fn measure_counts_tabs_as_zero_cells() {
        assert_eq!(measure_text(&chars("tab\there")), (4, 7));
    }
}
