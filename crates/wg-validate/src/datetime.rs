//! Datetime parsing the way Pydantic's lax mode does it (speedate for strings, Unix time for
//! numbers) and the ISO form `model_dump(mode="json")` writes.

/// A datetime with an optional fixed offset in seconds; `None` is naive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DateTime {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    micro: i64,
    offset: Option<i64>,
}

impl DateTime {
    /// The JSON form: ISO 8601, `Z` for UTC, microseconds only when not zero.
    pub(crate) fn to_iso(self) -> String {
        let mut out = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        );
        if self.micro != 0 {
            out.push_str(&format!(".{:06}", self.micro));
        }
        match self.offset {
            None => {}
            Some(0) => out.push('Z'),
            Some(seconds) => {
                let sign = if seconds < 0 { '-' } else { '+' };
                let abs = seconds.abs();
                out.push_str(&format!("{sign}{:02}:{:02}", abs / 3600, abs % 3600 / 60));
            }
        }
        out
    }
}

fn is_leap(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A Unix timestamp (seconds, with microseconds) as a UTC datetime. Pydantic reads magnitudes
/// above 2e10 as milliseconds.
pub(crate) fn from_timestamp(seconds: f64) -> Option<DateTime> {
    if !seconds.is_finite() {
        return None;
    }
    let seconds = if seconds.abs() > 2e10 {
        seconds / 1000.0
    } else {
        seconds
    };
    let micros_total = (seconds * 1e6).round();
    if micros_total.abs() > 2.5e17 {
        return None;
    }
    let micros_total = micros_total as i64;
    let secs = micros_total.div_euclid(1_000_000);
    let micro = micros_total.rem_euclid(1_000_000);
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    if !(1..=9999).contains(&year) {
        return None;
    }
    let in_day = secs.rem_euclid(86_400);
    Some(DateTime {
        year,
        month,
        day,
        hour: in_day / 3600,
        minute: in_day % 3600 / 60,
        second: in_day % 60,
        micro,
        offset: Some(0),
    })
}

fn digits(bytes: &[u8], start: usize, count: usize) -> Option<i64> {
    let part = bytes.get(start..start + count)?;
    if !part.iter().all(u8::is_ascii_digit) {
        return None;
    }
    part.iter()
        .try_fold(0_i64, |acc, byte| Some(acc * 10 + i64::from(byte - b'0')))
}

/// Parses an RFC 3339 style datetime or a bare date (midnight, naive). The error is speedate's
/// message.
pub(crate) fn parse(text: &str) -> Result<DateTime, &'static str> {
    let bytes = text.as_bytes();
    if bytes.len() < 10 {
        return Err("input is too short");
    }
    let year = digits(bytes, 0, 4).ok_or("invalid character in year")?;
    if bytes[4] != b'-' {
        return Err("invalid character in date separator, expected `-`");
    }
    let month = digits(bytes, 5, 2).ok_or("invalid character in month")?;
    if bytes[7] != b'-' {
        return Err("invalid character in date separator, expected `-`");
    }
    let day = digits(bytes, 8, 2).ok_or("invalid character in day")?;
    if !(1..=12).contains(&month) {
        return Err("month value is outside expected range of 1-12");
    }
    if day < 1 || day > days_in_month(year, month) {
        return Err("day value is outside expected range");
    }
    let date = DateTime {
        year,
        month,
        day,
        hour: 0,
        minute: 0,
        second: 0,
        micro: 0,
        offset: None,
    };
    if bytes.len() == 10 {
        return Ok(date);
    }
    if !matches!(bytes[10], b'T' | b't' | b' ' | b'_') {
        return Err("invalid datetime separator, expected `T`, `t`, `_` or space");
    }
    let hour = digits(bytes, 11, 2).ok_or("invalid character in hour")?;
    if bytes.get(13) != Some(&b':') {
        return Err("invalid time separator, expected `:`");
    }
    let minute = digits(bytes, 14, 2).ok_or("invalid character in minute")?;
    let mut pos = 16;
    let mut second = 0;
    let mut micro = 0;
    if bytes.get(pos) == Some(&b':') {
        second = digits(bytes, pos + 1, 2).ok_or("invalid character in second")?;
        pos += 3;
        if matches!(bytes.get(pos), Some(b'.' | b',')) {
            pos += 1;
            let start = pos;
            while bytes.get(pos).is_some_and(u8::is_ascii_digit) {
                pos += 1;
            }
            if pos == start {
                return Err("invalid character in second fraction");
            }
            let frac = &bytes[start..pos.min(start + 6)];
            micro = frac
                .iter()
                .fold(0_i64, |acc, byte| acc * 10 + i64::from(byte - b'0'));
            micro *= 10_i64.pow(6 - frac.len() as u32);
        }
    }
    if hour > 23 {
        return Err("hour value is outside expected range of 0-23");
    }
    if minute > 59 {
        return Err("minute value is outside expected range of 0-59");
    }
    if second > 59 {
        return Err("second value is outside expected range of 0-59");
    }
    let offset = match bytes.get(pos..) {
        Some([]) | None => None,
        Some([b'Z' | b'z']) => Some(0),
        Some(rest @ [sign @ (b'+' | b'-'), ..]) => {
            let hours = digits(rest, 1, 2).ok_or("invalid timezone offset")?;
            let (minutes, used) = match rest.get(3) {
                None => (0, 3),
                Some(b':') => (digits(rest, 4, 2).ok_or("invalid timezone offset")?, 6),
                Some(_) => (digits(rest, 3, 2).ok_or("invalid timezone offset")?, 5),
            };
            if rest.len() != used || hours > 23 || minutes > 59 {
                return Err("invalid timezone offset");
            }
            let total = hours * 3600 + minutes * 60;
            Some(if *sign == b'-' { -total } else { total })
        }
        Some(_) => return Err("extra characters found after the datetime"),
    };
    Ok(DateTime {
        hour,
        minute,
        second,
        micro,
        offset,
        ..date
    })
}
