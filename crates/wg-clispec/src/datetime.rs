//! Click's `DateTime` parameter type, as typer builds it for a `datetime` option: each of the
//! three formats is tried in order with `datetime.strptime`, and the first that parses wins.
//!
//! `strptime` matches a format as a regular expression anchored at the start (CPython's
//! `_strptime.TimeRE`), keeps the first match found, and then requires that match to end at the
//! end of the value. The [`regex`] crate uses the same leftmost-first alternation, so the same
//! texts are accepted and the same ones rejected.

use regex::Regex;
use wg_json::LocalDateTime;
use wg_model::py_repr_str;

/// The formats of a `datetime` option, in the order typer gives them to Click.
pub const FORMATS: [&str; 3] = ["%Y-%m-%d", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"];

/// One piece of a format: a directive, a literal character, or the blank of a space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Token {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Literal(char),
    /// A space in the format: `_strptime` matches one or more whitespace characters for it.
    Blank,
}

const DATE: [Token; 5] = [
    Token::Year,
    Token::Literal('-'),
    Token::Month,
    Token::Literal('-'),
    Token::Day,
];

/// The tokens of each format in [`FORMATS`].
const TOKENS: [&[Token]; 3] = [
    &DATE,
    &[
        Token::Year,
        Token::Literal('-'),
        Token::Month,
        Token::Literal('-'),
        Token::Day,
        Token::Literal('T'),
        Token::Hour,
        Token::Literal(':'),
        Token::Minute,
        Token::Literal(':'),
        Token::Second,
    ],
    &[
        Token::Year,
        Token::Literal('-'),
        Token::Month,
        Token::Literal('-'),
        Token::Day,
        Token::Blank,
        Token::Hour,
        Token::Literal(':'),
        Token::Minute,
        Token::Literal(':'),
        Token::Second,
    ],
];

impl Token {
    /// The alternatives `_strptime.TimeRE` gives a directive, in its priority order. `None` for
    /// literals and blanks.
    fn alternatives(self) -> Option<&'static str> {
        Some(match self {
            Self::Year => r"\d\d\d\d",
            Self::Month => "1[0-2]|0[1-9]|[1-9]",
            Self::Day => "3[01]|[12]\\d|0[1-9]|[1-9]| [1-9]",
            Self::Hour => r"2[0-3]|[0-1]\d|\d",
            Self::Minute => r"[0-5]\d|\d",
            Self::Second => r"6[0-1]|[0-5]\d|\d",
            Self::Literal(_) | Self::Blank => return None,
        })
    }

    /// The pattern of this token: a capture group for a directive, the literal (matched without
    /// regard to case, as `_strptime` does) or the whitespace run of a blank.
    fn pattern(self) -> String {
        match (self.alternatives(), self) {
            (Some(alternatives), _) => format!("({alternatives})"),
            (None, Token::Literal(c)) => regex::escape(&c.to_string()),
            (None, _) => r"[\s\x1c-\x1f]+".to_owned(),
        }
    }
}

/// The regular expression of a token list, anchored at the start of the value.
fn pattern(tokens: &[Token]) -> String {
    let body: String = tokens.iter().map(|token| token.pattern()).collect();
    format!("(?i)^{body}")
}

/// The value of a digit the pattern matched. `\d` matches any Unicode decimal digit (Nd), which
/// Python's `int` reads too. Those digits sit in runs of ten that count up from zero, so the value
/// is the position in the run: the count of decimal digits just before it, modulo ten.
fn digit_value(decimal: &Regex, c: char) -> Option<u32> {
    if let Some(value) = c.to_digit(10) {
        return Some(value);
    }
    let mut run = 0_u32;
    let mut code = u32::from(c);
    while let Some(prev) = code.checked_sub(1).and_then(char::from_u32) {
        if !decimal.is_match(prev.encode_utf8(&mut [0; 4])) {
            break;
        }
        run += 1;
        code -= 1;
    }
    Some(run % 10)
}

/// The number that a run of matched digits spells.
fn number(text: &str) -> Option<u32> {
    let decimal = Regex::new(r"^\d$").ok()?;
    text.chars().try_fold(0_u32, |acc, c| {
        digit_value(&decimal, c).map(|digit| acc.saturating_mul(10).saturating_add(digit))
    })
}

/// Days in a month of the proleptic Gregorian calendar.
fn days_in(year: u32, month: u32) -> u32 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `datetime.strptime(value, format)` for the tokens of one format. `None` where Python raises
/// `ValueError`: no match, unconverted data, or a field outside its calendar range.
fn strptime(value: &str, tokens: &[Token]) -> Option<LocalDateTime> {
    let re = Regex::new(&pattern(tokens)).ok()?;
    let caps = re.captures(value)?;
    let whole = caps.get(0)?;
    if whole.end() != value.len() {
        return None;
    }
    // The capture groups are the directives, in the order the format names them; a blank or a
    // literal captures nothing.
    let values = tokens
        .iter()
        .filter(|token| token.alternatives().is_some())
        .enumerate()
        .map(|(index, token)| {
            let text = caps.get(index + 1)?.as_str();
            Some((*token, number(text.trim_start())?))
        })
        .collect::<Option<Vec<_>>>()?;
    let field = |wanted: Token| {
        values
            .iter()
            .find(|(token, _)| *token == wanted)
            .map(|(_, value)| *value)
    };
    let year = field(Token::Year)?;
    let month = field(Token::Month)?;
    let day = field(Token::Day)?;
    let hour = field(Token::Hour).unwrap_or(0);
    let minute = field(Token::Minute).unwrap_or(0);
    let second = field(Token::Second).unwrap_or(0);
    // `datetime` has years 1 to 9999 and seconds 0 to 59; the regular expression already bounds
    // the other fields.
    if !(1..=9999).contains(&year) || second > 59 || day > days_in(year, month) {
        return None;
    }
    Some(LocalDateTime {
        year: i32::try_from(year).ok()?,
        month: u8::try_from(month).ok()?,
        day: u8::try_from(day).ok()?,
        hour: u8::try_from(hour).ok()?,
        minute: u8::try_from(minute).ok()?,
        second: u8::try_from(second).ok()?,
        microsecond: 0,
        offset_seconds: None,
    })
}

/// Click's `DateTime.convert` for the [`FORMATS`]: the naive datetime of the first format that
/// parses `value`, or Click's message for the value that none does (without the option name).
pub fn parse_click_datetime(value: &str) -> Result<LocalDateTime, String> {
    if let Some(parsed) = TOKENS.iter().find_map(|tokens| strptime(value, tokens)) {
        return Ok(parsed);
    }
    let formats = FORMATS.map(py_repr_str).join(", ");
    Err(format!(
        "{} does not match the formats {formats}.",
        py_repr_str(value)
    ))
}
