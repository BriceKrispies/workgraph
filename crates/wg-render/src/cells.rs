//! Terminal cell widths, ported from `rich.cells` (rich 15.0.0, Unicode 17 table).
//!
//! Text is handled as slices of `char`, so every offset is a code point offset, as in Python.

use crate::rich_data::{CELL_WIDTHS, NARROW_TO_WIDE};

const ZWJ: char = '\u{200d}';
const VS16: char = '\u{fe0f}';

/// Ranges that rich treats as one cell wide without consulting the table.
const SINGLE_CELL_RANGES: [(u32, u32); 6] = [
    (0x20, 0x7e),
    (0xa0, 0xac),
    (0xae, 0x2ff),
    (0x370, 0x482),
    (0x2500, 0x25fc),
    (0x2800, 0x28ff),
];

fn is_single_cell(c: char) -> bool {
    let cp = c as u32;
    SINGLE_CELL_RANGES
        .iter()
        .any(|&(lo, hi)| cp >= lo && cp <= hi)
}

fn all_single_cell(text: &[char]) -> bool {
    text.iter().all(|&c| is_single_cell(c))
}

/// Number of cells (0, 1 or 2) one character occupies.
#[must_use]
pub fn char_cells(c: char) -> usize {
    let cp = c as u32;
    if (cp != 0 && cp < 32) || (0x7f..0xa0).contains(&cp) {
        return 0;
    }
    let Some(last) = CELL_WIDTHS.last() else {
        return 1;
    };
    if cp > last.1 {
        return 1;
    }
    let index = CELL_WIDTHS.partition_point(|&(_, end, _)| end < cp);
    match CELL_WIDTHS.get(index) {
        Some(&(start, end, width)) if cp >= start && cp <= end => usize::from(width),
        _ => 1,
    }
}

fn is_narrow_to_wide(c: char) -> bool {
    NARROW_TO_WIDE.binary_search(&(c as u32)).is_ok()
}

/// Number of terminal cells a string needs (`rich.cells.cell_len`).
#[must_use]
pub fn cell_len(text: &[char]) -> usize {
    if all_single_cell(text) {
        return text.len();
    }
    if !text.iter().any(|&c| c == ZWJ || c == VS16) {
        return text.iter().map(|&c| char_cells(c)).sum();
    }
    let mut total = 0;
    let mut last: Option<char> = None;
    let mut index = 0;
    while index < text.len() {
        let c = text[index];
        if c == ZWJ {
            index += 1;
        } else if c == VS16 {
            if let Some(prev) = last {
                total += usize::from(is_narrow_to_wide(prev));
                last = None;
            }
        } else {
            let width = char_cells(c);
            if width > 0 {
                last = Some(c);
                total += width;
            }
        }
        index += 1;
    }
    total
}

/// `(start, end, cells)` of one grapheme, as offsets into the text.
type Span = (usize, usize, usize);

/// Splits text into graphemes with their cell widths (`rich.cells.split_graphemes`).
fn split_graphemes(text: &[char]) -> (Vec<Span>, usize) {
    let count = text.len();
    let mut index = 0;
    let mut last: Option<char> = None;
    let mut total = 0;
    let mut spans: Vec<Span> = Vec::new();
    while index < count {
        let c = text[index];
        if c == ZWJ || c == VS16 {
            let Some(&(start, _, cells)) = spans.last() else {
                spans.push((index, index + 1, 0));
                index += 1;
                continue;
            };
            if c == ZWJ {
                index += if index + 1 < count { 2 } else { 1 };
                set_last(&mut spans, (start, index, cells));
            } else {
                index += 1;
                if let Some(prev) = last {
                    let mut cells = cells;
                    if is_narrow_to_wide(prev) {
                        last = None;
                        cells += 1;
                        total += 1;
                    }
                    set_last(&mut spans, (start, index, cells));
                } else {
                    set_last(&mut spans, (start, index, cells));
                }
            }
            continue;
        }
        let width = char_cells(c);
        if width > 0 {
            last = Some(c);
            spans.push((index, index + 1, width));
            total += width;
            index += 1;
        } else if let Some(&(start, _, cells)) = spans.last() {
            index += 1;
            set_last(&mut spans, (start, index, cells));
        } else {
            spans.push((index, index + 1, 0));
            index += 1;
        }
    }
    (spans, total)
}

fn set_last(spans: &mut [Span], value: Span) {
    if let Some(slot) = spans.last_mut() {
        *slot = value;
    }
}

/// Splits at a cell position; a wide character cut in two becomes two spaces
/// (`rich.cells._split_text`).
fn split_text_slow(text: &[char], cell_position: usize) -> (Vec<char>, Vec<char>) {
    if cell_position == 0 {
        return (Vec::new(), text.to_vec());
    }
    let (spans, cell_length) = split_graphemes(text);
    if cell_length == 0 || spans.is_empty() {
        return (text.to_vec(), Vec::new());
    }
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let guess = ((cell_position as f64 / cell_length as f64) * spans.len() as f64) as usize;
    let mut offset = guess.min(spans.len());
    let mut left_size: usize = spans[..offset].iter().map(|s| s.2).sum();
    loop {
        if left_size == cell_position {
            if offset >= spans.len() {
                return (text.to_vec(), Vec::new());
            }
            let at = spans[offset].0;
            return (text[..at].to_vec(), text[at..].to_vec());
        }
        if left_size < cell_position {
            let Some(&(start, end, size)) = spans.get(offset) else {
                return (text.to_vec(), Vec::new());
            };
            if left_size + size > cell_position {
                return (
                    with_space_after(&text[..start]),
                    with_space_before(&text[end..]),
                );
            }
            offset += 1;
            left_size += size;
        } else {
            if offset == 0 {
                return (Vec::new(), text.to_vec());
            }
            let (start, end, size) = spans[offset - 1];
            if left_size - size < cell_position {
                return (
                    with_space_after(&text[..start]),
                    with_space_before(&text[end..]),
                );
            }
            offset -= 1;
            left_size -= size;
        }
    }
}

fn with_space_after(text: &[char]) -> Vec<char> {
    let mut out = text.to_vec();
    out.push(' ');
    out
}

fn with_space_before(text: &[char]) -> Vec<char> {
    let mut out = vec![' '];
    out.extend_from_slice(text);
    out
}

/// Pads or crops a string to exactly `total` cells (`rich.cells.set_cell_size`).
#[must_use]
pub fn set_cell_size(text: &[char], total: usize) -> Vec<char> {
    if all_single_cell(text) {
        let size = text.len();
        if size < total {
            let mut out = text.to_vec();
            out.resize(total, ' ');
            return out;
        }
        return text[..total].to_vec();
    }
    if total == 0 {
        return Vec::new();
    }
    let size = cell_len(text);
    if size == total {
        return text.to_vec();
    }
    if size < total {
        let mut out = text.to_vec();
        out.resize(out.len() + (total - size), ' ');
        return out;
    }
    split_text_slow(text, total).0
}

/// Splits text into pieces that each fit `width` cells (`rich.cells.chop_cells`).
#[must_use]
pub fn chop_cells(text: &[char], width: usize) -> Vec<Vec<char>> {
    if all_single_cell(text) {
        return text.chunks(width.max(1)).map(<[char]>::to_vec).collect();
    }
    let (spans, _) = split_graphemes(text);
    let mut line_size = 0;
    let mut lines: Vec<Vec<char>> = Vec::new();
    let mut line_offset = 0;
    for (start, _end, cells) in spans {
        if line_size + cells > width {
            lines.push(text[line_offset..start].to_vec());
            line_offset = start;
            line_size = 0;
        }
        line_size += cells;
    }
    if line_size > 0 {
        lines.push(text[line_offset..].to_vec());
    }
    lines
}
