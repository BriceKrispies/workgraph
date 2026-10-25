//! Table layout, ported from `rich.table.Table` with the options `workgraph.cli.common.table`
//! uses: heavy-head box, one space of padding either side of a cell, wrapping columns that fold
//! overlong words, no expansion, no footer.
//!
//! Widths are `i64` because rich computes with signed integers and a narrow console drives
//! several intermediate values below zero.

use crate::markup::{MarkupError, render_plain};
use crate::text::{Justify, fit, measure_text, wrap};

/// `(minimum, maximum)` width, rich's `Measurement`.
#[derive(Clone, Copy)]
struct Measure {
    min: i64,
    max: i64,
}

impl Measure {
    fn normalize(self) -> Self {
        let min = self.min.max(0).min(self.max);
        Self {
            min: min.max(0),
            max: min.max(self.max).max(0),
        }
    }

    fn with_maximum(self, width: i64) -> Self {
        Self {
            min: self.min.min(width),
            max: self.max.min(width),
        }
    }
}

fn to_i64(n: usize) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn to_usize(n: i64) -> usize {
    usize::try_from(n.max(0)).unwrap_or(0)
}

/// `Measurement.get` for a markup string.
fn measure_str(markup: &str, max_width: i64) -> Result<Measure, MarkupError> {
    if max_width < 1 {
        return Ok(Measure { min: 0, max: 0 });
    }
    let text = render_plain(markup)?;
    let (min, max) = measure_text(&text);
    let measured = Measure {
        min: to_i64(min),
        max: to_i64(max),
    }
    .normalize()
    .with_maximum(max_width);
    if measured.max < 1 {
        return Ok(Measure { min: 0, max: 0 });
    }
    Ok(measured.normalize())
}

/// `Measurement.get` for a cell wrapped in `Padding(cell, (0, 1, 0, 1))`.
fn measure_cell(markup: &str, max_width: i64) -> Result<Measure, MarkupError> {
    if max_width < 1 {
        return Ok(Measure { min: 0, max: 0 });
    }
    let padded = if max_width - 2 < 1 {
        Measure {
            min: max_width,
            max: max_width,
        }
    } else {
        let inner = measure_str(markup, max_width)?;
        Measure {
            min: inner.min + 2,
            max: inner.max + 2,
        }
        .with_maximum(max_width)
    };
    let measured = padded.normalize().with_maximum(max_width);
    if measured.max < 1 {
        return Ok(Measure { min: 0, max: 0 });
    }
    Ok(measured.normalize())
}

/// `Table._measure_column`: the header and every cell of one column.
fn measure_column(cells: &[&str], max_width: i64) -> Result<Measure, MarkupError> {
    if max_width < 1 {
        return Ok(Measure { min: 0, max: 0 });
    }
    let mut min = 1;
    let mut max = max_width;
    let mut first = true;
    for cell in cells {
        let m = measure_cell(cell, max_width)?;
        if first {
            min = m.min;
            max = m.max;
            first = false;
        } else {
            min = min.max(m.min);
            max = max.max(m.max);
        }
    }
    Ok(Measure { min, max }.with_maximum(max_width))
}

/// Python's `round()` of `numerator / denominator`: halves go to the even neighbour.
fn round_div(numerator: i64, denominator: i64) -> i64 {
    let quotient = numerator.div_euclid(denominator);
    let remainder = numerator.rem_euclid(denominator);
    match (remainder * 2).cmp(&denominator) {
        std::cmp::Ordering::Greater => quotient + 1,
        std::cmp::Ordering::Equal if quotient % 2 != 0 => quotient + 1,
        _ => quotient,
    }
}

/// `rich._ratio.ratio_reduce`.
fn ratio_reduce(total: i64, ratios: &[i64], maximums: &[i64], values: &[i64]) -> Vec<i64> {
    let ratios: Vec<i64> = ratios
        .iter()
        .zip(maximums)
        .map(|(&ratio, &maximum)| if maximum != 0 { ratio } else { 0 })
        .collect();
    let mut total_ratio: i64 = ratios.iter().sum();
    if total_ratio == 0 {
        return values.to_vec();
    }
    let mut total_remaining = total;
    let mut result = Vec::with_capacity(values.len());
    for ((&ratio, &maximum), &value) in ratios.iter().zip(maximums).zip(values) {
        if ratio != 0 && total_ratio > 0 {
            let distributed = maximum.min(round_div(ratio * total_remaining, total_ratio));
            result.push(value - distributed);
            total_remaining -= distributed;
            total_ratio -= ratio;
        } else {
            result.push(value);
        }
    }
    result
}

/// `Table._collapse_widths` with every column allowed to shrink.
fn collapse_widths(widths: Vec<i64>, max_width: i64) -> Vec<i64> {
    let mut widths = widths;
    let mut total_width: i64 = widths.iter().sum();
    let mut excess_width = total_width - max_width;
    while total_width != 0 && excess_width > 0 {
        let max_column = widths.iter().copied().max().unwrap_or(0);
        let second_max_column = widths
            .iter()
            .map(|&w| if w == max_column { 0 } else { w })
            .max()
            .unwrap_or(0);
        let column_difference = max_column - second_max_column;
        let ratios: Vec<i64> = widths.iter().map(|&w| i64::from(w == max_column)).collect();
        if ratios.iter().all(|&r| r == 0) || column_difference == 0 {
            break;
        }
        let max_reduce = vec![excess_width.min(column_difference); widths.len()];
        widths = ratio_reduce(excess_width, &ratios, &max_reduce, &widths);
        total_width = widths.iter().sum();
        excess_width = total_width - max_width;
    }
    widths
}

/// `Table._calculate_column_widths`; `max_width` is the console width less the borders.
fn column_widths(columns: &[Vec<&str>], max_width: i64) -> Result<Vec<i64>, MarkupError> {
    let mut ranges = Vec::with_capacity(columns.len());
    for column in columns {
        ranges.push(measure_column(column, max_width)?);
    }
    let mut widths: Vec<i64> = ranges
        .iter()
        .map(|r| if r.max == 0 { 1 } else { r.max })
        .collect();
    let mut table_width: i64 = widths.iter().sum();
    if table_width > max_width {
        widths = collapse_widths(widths, max_width);
        table_width = widths.iter().sum();
        if table_width > max_width {
            let excess = table_width - max_width;
            let ones = vec![1; widths.len()];
            widths = ratio_reduce(excess, &ones, &widths, &widths);
        }
        let mut remeasured = Vec::with_capacity(columns.len());
        for (column, &width) in columns.iter().zip(&widths) {
            remeasured.push(measure_column(column, width)?.max);
        }
        widths = remeasured;
    }
    Ok(widths)
}

/// One cell drawn into lines exactly `column_width` cells wide: `Padding(cell, (0, 1, 0, 1))`
/// (which expands to the column) around the text wrapped to the width less the padding.
fn render_cell(markup: &str, column_width: i64) -> Result<Vec<Vec<char>>, MarkupError> {
    let inner = to_usize(column_width - 2);
    let text = render_plain(markup)?;
    let mut lines = Vec::new();
    for line in wrap(&text, inner, Justify::Left) {
        let mut padded = vec![' '];
        padded.extend(fit(&line, inner));
        padded.push(' ');
        lines.push(fit(&padded, to_usize(column_width)));
    }
    Ok(lines)
}

fn rule(widths: &[i64], left: char, fill: char, cross: char, right: char) -> Vec<char> {
    let mut line = vec![left];
    for (index, &width) in widths.iter().enumerate() {
        if index > 0 {
            line.push(cross);
        }
        line.extend(std::iter::repeat_n(fill, to_usize(width)));
    }
    line.push(right);
    line
}

/// Lays out a table as `rich.table.Table` would draw it at `console_width` cells.
///
/// `headers` and every row are markup strings. A row shorter than the headers is padded with
/// empty cells; a longer one adds columns with an empty header. The title is rendered after the
/// widths are fixed, so a markup error in a cell wins over one in the title.
pub(crate) fn render_table(
    rows: &[Vec<String>],
    headers: &[&str],
    title: Option<&str>,
    console_width: usize,
) -> Result<Vec<Vec<char>>, MarkupError> {
    let ncols = rows
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0)
        .max(headers.len());
    if ncols == 0 {
        return Ok(vec![Vec::new()]);
    }
    let cell = |row: &Vec<String>, c: usize| -> String { row.get(c).cloned().unwrap_or_default() };
    let columns: Vec<Vec<String>> = (0..ncols)
        .map(|c| {
            let mut column = vec![headers.get(c).map(|h| (*h).to_owned()).unwrap_or_default()];
            column.extend(rows.iter().map(|row| cell(row, c)));
            column
        })
        .collect();
    let column_refs: Vec<Vec<&str>> = columns
        .iter()
        .map(|column| column.iter().map(String::as_str).collect())
        .collect();

    let extra_width = to_i64(ncols) + 1;
    let widths = column_widths(&column_refs, to_i64(console_width) - extra_width)?;
    let table_width = widths.iter().sum::<i64>() + extra_width;

    let mut out: Vec<Vec<char>> = Vec::new();
    if let Some(title) = title.filter(|t| !t.is_empty()) {
        let text = render_plain(title)?;
        out.extend(wrap(&text, to_usize(table_width), Justify::Center));
    }

    out.push(rule(&widths, '┏', '━', '┳', '┓'));
    for row_index in 0..=rows.len() {
        let header_row = row_index == 0;
        let mut rendered: Vec<Vec<Vec<char>>> = Vec::with_capacity(ncols);
        for (column, &width) in column_refs.iter().zip(&widths) {
            rendered.push(render_cell(column[row_index], width)?);
        }
        let height = rendered.iter().map(Vec::len).max().unwrap_or(1);
        let aligned: Vec<Vec<Vec<char>>> = rendered
            .into_iter()
            .zip(&widths)
            .map(|(lines, &width)| {
                let blank = vec![' '; to_usize(width)];
                let missing = vec![blank; height - lines.len()];
                if header_row {
                    missing.into_iter().chain(lines).collect()
                } else {
                    lines.into_iter().chain(missing).collect()
                }
            })
            .collect();
        let bar = if header_row { '┃' } else { '│' };
        for line_no in 0..height {
            let mut line = vec![bar];
            for (index, cell_lines) in aligned.iter().enumerate() {
                if index > 0 {
                    line.push(bar);
                }
                line.extend(cell_lines[line_no].iter());
            }
            line.push(bar);
            out.push(line);
        }
        if header_row {
            out.push(rule(&widths, '┡', '━', '╇', '┩'));
        }
    }
    out.push(rule(&widths, '└', '─', '┴', '┘'));
    Ok(out)
}
