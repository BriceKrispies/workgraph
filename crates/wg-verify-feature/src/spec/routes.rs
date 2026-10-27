//! Pure: the verification routes (H085 to H090, `api/routes/ops.py`): the query parameters read
//! into the arguments of the services. The bodies and the path parameters are read by the route
//! table in [`crate::http`].

use serde_json::{Map, Value as Json};
use time::{Date, Duration, Month, PrimitiveDateTime, Time};
use wg_types::{Timestamp, WgError};

/// The `limit` of `GET /ws/{ws}/verification/runs` when the client sends none.
pub const RUNS_LIMIT: i64 = 50;

/// The `action` of `POST /verification/configs/{config_id}/{action}`: `true` to enable, `false` to
/// disable. The snapshot's enum rejects any other word first; this is the same answer for a
/// route that reads it anyway.
pub fn enabled_of(action: &str) -> Result<bool, WgError> {
    match action {
        "enable" => Ok(true),
        "disable" => Ok(false),
        other => Err(WgError::validation(format!(
            "unknown verification action {other}"
        ))),
    }
}

/// The `check`, `repository` and `since` of `GET /ws/{ws}/verification/failures`. An empty text is
/// no filter, as the Python service tests them (`if check_name:`).
#[must_use]
pub fn failure_args(params: &Map<String, Json>) -> (Option<&str>, Option<&str>, Option<&str>) {
    let text = |key: &str| {
        params
            .get(key)
            .and_then(Json::as_str)
            .filter(|found| !found.is_empty())
    };
    (text("check"), text("repository"), text("since"))
}

/// The `status` and `limit` of `GET /ws/{ws}/verification/runs`. An empty status is no filter,
/// as the Python service reads it.
#[must_use]
pub fn run_args(params: &Map<String, Json>) -> (Option<&str>, i64) {
    let status = params
        .get("status")
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty());
    let limit = params
        .get("limit")
        .and_then(Json::as_i64)
        .unwrap_or(RUNS_LIMIT);
    (status, limit)
}

/// The `since` of the failure history, read as Python's `datetime.fromisoformat` reads it
/// (Python 3.11 and later): a date (`YYYY-MM-DD` or `YYYYMMDD`), then optionally one separator
/// character and a time (`HH`, `HH:MM`, `HH:MM:SS` or the basic `HH`, `HHMM`, `HHMMSS`, each with
/// an optional `.fraction`), then optionally an offset (`Z`, or `+`/`-` with `HH[:MM[:SS]]` or
/// `HH[MM[SS]]`). A value without an offset is UTC, as the `UTCDateTime` column binds it.
pub fn parse_since(text: &str) -> Result<Timestamp, WgError> {
    let bad = || WgError::validation(format!("invalid datetime {text:?}"));
    if !text.is_ascii() {
        return Err(bad());
    }
    let bytes = text.as_bytes();
    let (year, month, day, rest) = if bytes.get(4) == Some(&b'-') {
        if bytes.len() < 10 || bytes.get(7) != Some(&b'-') {
            return Err(bad());
        }
        (&text[0..4], &text[5..7], &text[8..10], &text[10..])
    } else {
        if bytes.len() < 8 {
            return Err(bad());
        }
        (&text[0..4], &text[4..6], &text[6..8], &text[8..])
    };
    let year = digits(year).ok_or_else(bad)?;
    if year < 1 {
        return Err(bad());
    }
    let month = Month::try_from(digits(month).ok_or_else(bad)? as u8).map_err(|_| bad())?;
    let date = Date::from_calendar_date(year as i32, month, digits(day).ok_or_else(bad)? as u8)
        .map_err(|_| bad())?;

    let (clock, zone) = if rest.is_empty() {
        (None, None)
    } else {
        // The one separator character is skipped; any character will do, as in Python.
        let body = &rest[1..];
        let split = body.find(['Z', '+', '-']).unwrap_or(body.len());
        let (clock, zone) = body.split_at(split);
        if clock.is_empty() {
            return Err(bad());
        }
        (Some(clock), (!zone.is_empty()).then_some(zone))
    };
    let (hour, minute, second, micro) = match clock {
        Some(clock) => clock_of(clock).ok_or_else(bad)?,
        None => (0, 0, 0, 0),
    };
    let offset = match zone {
        Some(zone) => offset_of(zone).ok_or_else(bad)?,
        None => 0,
    };

    let time = Time::from_hms_micro(hour, minute, second, micro).map_err(|_| bad())?;
    let utc = PrimitiveDateTime::new(date, time)
        .checked_sub(Duration::seconds(offset))
        .ok_or_else(bad)?;
    Timestamp::parse_iso(&format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:06}+00:00",
        utc.year(),
        utc.month() as u8,
        utc.day(),
        utc.hour(),
        utc.minute(),
        utc.second(),
        utc.microsecond(),
    ))
}

/// A run of ASCII digits as a number; `None` for anything else, or an empty run.
fn digits(text: &str) -> Option<u32> {
    (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// The fields of `HH:MM:SS` (or the basic `HHMMSS`) as two-digit numbers: one to three of them.
fn fields_of(text: &str) -> Option<Vec<u32>> {
    let parts: Vec<&str> = if text.contains(':') {
        text.split(':').collect()
    } else {
        text.as_bytes()
            .chunks(2)
            .map(|pair| std::str::from_utf8(pair).unwrap_or(""))
            .collect()
    };
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|part| part.len() != 2) {
        return None;
    }
    parts.iter().map(|part| digits(part)).collect()
}

/// `HH[:MM[:SS]][.fraction]` as hour, minute, second and microsecond. A fraction needs all three
/// fields, as Python has no fractional hours or minutes.
fn clock_of(text: &str) -> Option<(u8, u8, u8, u32)> {
    let (main, fraction) = match text.split_once('.') {
        Some((main, fraction)) => (main, Some(fraction)),
        None => (text, None),
    };
    let fields = fields_of(main)?;
    if fraction.is_some() && fields.len() != 3 {
        return None;
    }
    let micro = match fraction {
        Some(fraction) => {
            let kept: String = fraction.chars().take(6).collect();
            digits(fraction)?;
            digits(&format!("{kept:0<6}"))?
        }
        None => 0,
    };
    let at = |index: usize| fields.get(index).copied().unwrap_or(0);
    let (hour, minute, second) = (at(0), at(1), at(2));
    (hour <= 23 && minute <= 59 && second <= 59).then_some((
        hour as u8,
        minute as u8,
        second as u8,
        micro,
    ))
}

/// An offset in seconds east of UTC: `Z`, or a sign and `HH[:MM[:SS]]` (or the basic form), under
/// 24 hours.
fn offset_of(text: &str) -> Option<i64> {
    if text == "Z" {
        return Some(0);
    }
    let (sign, body) = match text.as_bytes().first()? {
        b'+' => (1_i64, &text[1..]),
        b'-' => (-1_i64, &text[1..]),
        _ => return None,
    };
    let (main, fraction) = match body.split_once('.') {
        Some((main, fraction)) => (main, Some(fraction)),
        None => (body, None),
    };
    let fields = fields_of(main)?;
    if let Some(fraction) = fraction {
        digits(fraction)?;
    }
    let at = |index: usize| fields.get(index).copied().unwrap_or(0) as i64;
    let seconds = at(0) * 3600 + at(1) * 60 + at(2);
    (seconds < 86_400).then_some(sign * seconds)
}
