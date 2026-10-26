//! The metric periods and their `date_trunc` buckets. Buckets are computed from the UTC day
//! number with integer arithmetic (the proleptic Gregorian calendar), so no date type is needed.

use wg_types::Timestamp;

/// A bucket size of the `period` dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl Period {
    /// Every period, in the order the catalog lists them.
    pub const ALL: [Period; 5] = [
        Period::Day,
        Period::Month,
        Period::Quarter,
        Period::Week,
        Period::Year,
    ];

    /// The name the query language uses.
    pub fn name(self) -> &'static str {
        match self {
            Period::Day => "day",
            Period::Week => "week",
            Period::Month => "month",
            Period::Quarter => "quarter",
            Period::Year => "year",
        }
    }

    /// The period with this name, if any.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|period| period.name() == text)
    }
}

/// The start of the `period` bucket that holds `at`, as `YYYY-MM-DD`. Weeks start on Monday.
pub fn bucket(period: Period, at: Timestamp) -> String {
    let day = at.to_offset_datetime().unix_timestamp().div_euclid(86_400);
    let monday = day - (day + 3).rem_euclid(7);
    let (year, month, date) = civil_from_days(match period {
        Period::Week => monday,
        _ => day,
    });
    let (month, date) = match period {
        Period::Day | Period::Week => (month, date),
        Period::Month => (month, 1),
        Period::Quarter => ((month - 1) / 3 * 3 + 1, 1),
        Period::Year => (1, 1),
    };
    format!("{year:04}-{month:02}-{date:02}")
}

/// The calendar date of a day number counted from 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let date = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, date)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_match_the_calendar() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(20_454), (2026, 1, 1));
    }
}
