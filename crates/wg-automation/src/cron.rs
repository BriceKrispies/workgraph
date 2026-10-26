//! Minimal 5-field cron expressions (minute hour day-of-month month day-of-week), UTC
//! (`automation/cron.py`).
//!
//! Matching and stepping use the wall-clock fields of the moment they are given, as the Python
//! `datetime` does. Error texts are the Python `ValidationFailed` messages, since the golden
//! vectors record them.

use std::collections::BTreeSet;

use time::{Duration, OffsetDateTime};
use wg_types::{ErrorCode, WgError};

use crate::py::str_repr;

/// Inclusive bounds of minute, hour, day-of-month, month and day-of-week (Sunday = 0).
const RANGES: [(u64, u64); 5] = [(0, 59), (0, 23), (1, 31), (1, 12), (0, 6)];

/// How far `next_after` looks ahead: one leap-year cycle plus a day, in minutes.
const SEARCH_LIMIT_MINUTES: u64 = 366 * 24 * 60;

/// A parsed cron expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    expression: String,
    minutes: BTreeSet<u64>,
    hours: BTreeSet<u64>,
    days: BTreeSet<u64>,
    months: BTreeSet<u64>,
    weekdays: BTreeSet<u64>,
    dom_any: bool,
    dow_any: bool,
}

fn invalid(message: String) -> WgError {
    WgError::new(ErrorCode::ValidationFailed, message)
}

/// Python `str.isdigit()` for the ASCII inputs that cron fields hold.
fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Parses an unsigned integer, saturating at `u64::MAX` so that an out-of-range value is
/// reported by the range check rather than by overflow.
fn number(text: &str) -> u64 {
    text.parse::<u64>().unwrap_or(u64::MAX)
}

fn parse_field(spec: &str, low: u64, high: u64) -> Result<BTreeSet<u64>, WgError> {
    let mut values = BTreeSet::new();
    for item in spec.split(',') {
        let mut part = item;
        let mut step = 1;
        if let Some((head, step_text)) = part.split_once('/') {
            if !is_digits(step_text) || number(step_text) == 0 {
                return Err(invalid(format!(
                    "invalid cron step {}",
                    str_repr(step_text)
                )));
            }
            step = number(step_text);
            part = head;
        }
        let (start, end) = if part == "*" {
            (low, high)
        } else if let Some((first, last)) = part.split_once('-') {
            if !(is_digits(first) && is_digits(last)) {
                return Err(invalid(format!("invalid cron range {}", str_repr(part))));
            }
            (number(first), number(last))
        } else if is_digits(part) {
            (number(part), number(part))
        } else {
            return Err(invalid(format!("invalid cron field {}", str_repr(part))));
        };
        if start < low || end > high || start > end {
            return Err(invalid(format!(
                "cron value {} outside {low}-{high}",
                str_repr(part)
            )));
        }
        let mut value = start;
        while value <= end {
            values.insert(value);
            match value.checked_add(step) {
                Some(next) => value = next,
                None => break,
            }
        }
    }
    Ok(values)
}

impl Cron {
    /// Parses a five-field expression. Fields are split on whitespace.
    pub fn parse(expression: &str) -> Result<Self, WgError> {
        let fields: Vec<&str> = expression.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields.as_slice() else {
            return Err(invalid(
                "cron expressions need 5 fields: minute hour day month weekday".to_owned(),
            ));
        };
        let [min_range, hour_range, day_range, month_range, weekday_range] = RANGES;
        Ok(Cron {
            expression: expression.to_owned(),
            minutes: parse_field(minute, min_range.0, min_range.1)?,
            hours: parse_field(hour, hour_range.0, hour_range.1)?,
            days: parse_field(day, day_range.0, day_range.1)?,
            months: parse_field(month, month_range.0, month_range.1)?,
            weekdays: parse_field(weekday, weekday_range.0, weekday_range.1)?,
            dom_any: *day == "*",
            dow_any: *weekday == "*",
        })
    }

    /// Whether the minute of `moment` fires.
    pub fn matches(&self, moment: OffsetDateTime) -> bool {
        let weekday = u64::from(moment.weekday().number_days_from_sunday());
        let day = u64::from(moment.day());
        let day_ok = if self.dom_any || self.dow_any {
            self.days.contains(&day) && self.weekdays.contains(&weekday)
        } else {
            // Classic cron: either day-of-month or day-of-week may match.
            self.days.contains(&day) || self.weekdays.contains(&weekday)
        };
        self.minutes.contains(&u64::from(moment.minute()))
            && self.hours.contains(&u64::from(moment.hour()))
            && self.months.contains(&u64::from(u8::from(moment.month())))
            && day_ok
    }

    /// The first firing minute strictly after `moment`, searched minute by minute.
    pub fn next_after(&self, moment: OffsetDateTime) -> Result<OffsetDateTime, WgError> {
        let truncated = moment.replace_nanosecond(0).unwrap_or(moment);
        let truncated = truncated.replace_second(0).unwrap_or(truncated);
        let mut candidate = truncated.checked_add(Duration::minutes(1));
        for _ in 0..SEARCH_LIMIT_MINUTES {
            let Some(current) = candidate else { break };
            if self.matches(current) {
                return Ok(current);
            }
            candidate = current.checked_add(Duration::minutes(1));
        }
        Err(invalid(format!(
            "cron expression {} never fires",
            str_repr(&self.expression)
        )))
    }
}
