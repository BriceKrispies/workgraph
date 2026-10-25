//! UTC timestamps at microsecond precision and their four text formats.
//!
//! * [`Timestamp::format_z`]: Pydantic's JSON form of a model datetime (contract O01): `Z`
//!   suffix, fraction only when non-zero.
//! * [`Timestamp::format_isoformat`]: Python `datetime.isoformat()` of a UTC value: `+00:00`
//!   suffix, fraction only when non-zero.
//! * [`Timestamp::format_db`]: database column form (contract O11): always six fraction digits.
//! * [`Timestamp::format_display`]: `when()` for human output (contract O03).

use time::{Date, Month, OffsetDateTime, PrimitiveDateTime, Time, UtcOffset};

use crate::{ErrorCode, WgError};

/// A UTC instant truncated to microseconds. Naive values and other offsets are converted to UTC
/// on construction, so every formatter prints the UTC wall clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(OffsetDateTime);

fn invalid(message: impl Into<String>) -> WgError {
    WgError::new(ErrorCode::ValidationFailed, message)
}

fn component<E: core::fmt::Display, T>(result: Result<T, E>) -> Result<T, WgError> {
    result.map_err(|err| invalid(err.to_string()))
}

impl Timestamp {
    /// Converts an offset datetime to UTC and truncates it to whole microseconds.
    pub fn from_offset_datetime(value: OffsetDateTime) -> Result<Self, WgError> {
        let utc = value.to_offset(UtcOffset::UTC);
        let micros = utc.nanosecond() / 1_000;
        Ok(Self(component(utc.replace_nanosecond(micros * 1_000))?))
    }

    /// Builds a UTC timestamp from its calendar components. Years must be 1 to 9999, as in
    /// Python's `datetime`.
    pub fn from_components(
        year: i32,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        microsecond: u32,
    ) -> Result<Self, WgError> {
        if !(1..=9999).contains(&year) {
            return Err(invalid(format!("year {year} is outside 1..=9999")));
        }
        let month = component(Month::try_from(month))?;
        let date = component(Date::from_calendar_date(year, month, day))?;
        let time = component(Time::from_hms_micro(hour, minute, second, microsecond))?;
        Ok(Self(PrimitiveDateTime::new(date, time).assume_utc()))
    }

    /// Parses `YYYY-MM-DD` then `T` or a space, `HH:MM:SS`, an optional fraction of one to six
    /// digits, and a mandatory offset (`Z`, `+00:00`, or any `±HH:MM`, converted to UTC).
    /// Naive values are rejected: a stored timestamp always carries its offset.
    pub fn parse_iso(text: &str) -> Result<Self, WgError> {
        let malformed = || invalid(format!("invalid timestamp {text:?}"));
        let (date_part, rest) = text.split_once(['T', ' ']).ok_or_else(malformed)?;
        let date_fields: Vec<&str> = date_part.split('-').collect();
        let [year, month, day] = date_fields.as_slice() else {
            return Err(malformed());
        };
        let year = fixed_digits(year, 4).ok_or_else(malformed)?;
        let month = fixed_digits(month, 2).ok_or_else(malformed)?;
        let day = fixed_digits(day, 2).ok_or_else(malformed)?;

        let (clock, offset) = split_offset(rest).ok_or_else(malformed)?;
        let (hms, fraction) = match clock.split_once('.') {
            Some((hms, fraction)) => (hms, Some(fraction)),
            None => (clock, None),
        };
        let clock_fields: Vec<&str> = hms.split(':').collect();
        let [hour, minute, second] = clock_fields.as_slice() else {
            return Err(malformed());
        };
        let hour = fixed_digits(hour, 2).ok_or_else(malformed)?;
        let minute = fixed_digits(minute, 2).ok_or_else(malformed)?;
        let second = fixed_digits(second, 2).ok_or_else(malformed)?;
        let micros = match fraction {
            None => 0,
            Some(digits) => {
                if digits.is_empty()
                    || digits.len() > 6
                    || !digits.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(malformed());
                }
                let value: u32 = digits.parse().map_err(|_| malformed())?;
                value * 10u32.pow(6 - digits.len() as u32)
            }
        };

        let date = component(Month::try_from(month as u8))
            .and_then(|month| component(Date::from_calendar_date(year as i32, month, day as u8)))
            .map_err(|_| malformed())?;
        let time = component(Time::from_hms_micro(
            hour as u8,
            minute as u8,
            second as u8,
            micros,
        ))
        .map_err(|_| malformed())?;
        let local = PrimitiveDateTime::new(date, time);

        Self::from_offset_datetime(local.assume_offset(offset))
    }

    /// The same instant as an [`OffsetDateTime`] in UTC.
    pub fn to_offset_datetime(self) -> OffsetDateTime {
        self.0
    }

    /// Pydantic JSON form: `YYYY-MM-DDTHH:MM:SS[.ffffff]Z`.
    pub fn format_z(self) -> String {
        format!("{}{}Z", self.iso_body(), self.fraction_if_nonzero())
    }

    /// Python `isoformat()` of a UTC datetime: `YYYY-MM-DDTHH:MM:SS[.ffffff]+00:00`.
    pub fn format_isoformat(self) -> String {
        format!("{}{}+00:00", self.iso_body(), self.fraction_if_nonzero())
    }

    /// Database form (O11): `YYYY-MM-DDTHH:MM:SS.ffffff+00:00`, always six fraction digits.
    pub fn format_db(self) -> String {
        format!("{}.{:06}+00:00", self.iso_body(), self.microseconds())
    }

    /// The `when()` form for human output: `YYYY-MM-DD HH:MM`.
    pub fn format_display(self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}",
            self.0.year(),
            u8::from(self.0.month()),
            self.0.day(),
            self.0.hour(),
            self.0.minute()
        )
    }

    fn iso_body(self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            self.0.year(),
            u8::from(self.0.month()),
            self.0.day(),
            self.0.hour(),
            self.0.minute(),
            self.0.second()
        )
    }

    fn microseconds(self) -> u32 {
        self.0.nanosecond() / 1_000
    }

    /// `.ffffff` when the microsecond part is non-zero, otherwise nothing.
    fn fraction_if_nonzero(self) -> String {
        match self.microseconds() {
            0 => String::new(),
            micros => format!(".{micros:06}"),
        }
    }
}

/// Exactly `width` ASCII digits, as a number.
fn fixed_digits(text: &str, width: usize) -> Option<u32> {
    if text.len() == width && text.bytes().all(|b| b.is_ascii_digit()) {
        text.parse().ok()
    } else {
        None
    }
}

/// Splits the clock part from a trailing offset: `Z`, or `±HH:MM` (converted to a `UtcOffset`).
fn split_offset(rest: &str) -> Option<(&str, UtcOffset)> {
    if let Some(clock) = rest.strip_suffix('Z') {
        return Some((clock, UtcOffset::UTC));
    }
    let sign_at = rest.len().checked_sub(6)?;
    let (clock, tail) = rest.split_at(sign_at);
    let sign: i8 = match tail.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hours = fixed_digits(tail.get(1..3)?, 2)?;
    if tail.as_bytes().get(3)? != &b':' {
        return None;
    }
    let minutes = fixed_digits(tail.get(4..6)?, 2)?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    let offset = UtcOffset::from_hms(sign * hours as i8, sign * minutes as i8, 0).ok()?;
    Some((clock, offset))
}
