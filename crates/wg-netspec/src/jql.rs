//! JQL text for the Jira search (`integrations/trackers/jira/client.py`): quoted values, the
//! project list, the `updated` clause and the minute stamp it compares against.

/// A JQL string literal: the value in double quotes, with `\` and `"` escaped.
pub fn jql_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `project in ("A","B")`.
pub fn project_jql(projects: &[String]) -> String {
    let quoted: Vec<String> = projects.iter().map(|project| jql_string(project)).collect();
    format!("project in ({})", quoted.join(","))
}

/// The query of a search for updated issues. `since` is the minute stamp of
/// [`jql_minute`], or `None` for every issue in the projects.
pub fn updated_jql(projects: &[String], since: Option<&str>) -> String {
    let mut jql = project_jql(projects);
    if let Some(minute) = since {
        jql.push_str(&format!(" AND updated >= \"{minute}\""));
    }
    jql.push_str(" ORDER BY updated ASC");
    jql
}

/// The minute stamp JQL compares `updated` against: `YYYY-MM-DD HH:MM` in the zone whose UTC
/// offset is `offset_seconds` at the instant `epoch_seconds` (seconds are dropped, as
/// `strftime` drops them).
pub fn jql_minute(epoch_seconds: i64, offset_seconds: i64) -> String {
    let local = epoch_seconds + offset_seconds;
    let days = local.div_euclid(86_400);
    let second_of_day = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// The proleptic Gregorian date of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}
