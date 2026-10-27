//! `AnalyticsFilters`: the filter block of an analytics query. Its echo in a query result prints
//! the filters as `model_dump(mode="json")` does.

use wg_json::Value;
use wg_types::Timestamp;

use crate::convention::z_text;
use crate::dto::Dto;
use crate::wire_dto;

wire_dto! {
    /// The filter values of one query. An absent filter is `None`. The echo is a dict whose values
    /// Python has already turned into strings with `model_dump(mode="json")`, so the bounds are
    /// `Z` text here, not datetime values.
    #[derive(Default, Clone, Copy)]
    pub struct Filters<'a>("AnalyticsFilters") {
        pub project: Option<&'a str>,
        pub repository: Option<&'a str>,
        pub category: Option<&'a str>,
        pub agent: Option<&'a str>,
        pub provider: Option<&'a str>,
        pub model: Option<&'a str>,
        pub priority: Option<&'a str>,
        pub status: Option<&'a str>,
        pub kind: Option<&'a str>,
        #[serde(serialize_with = "z_text")]
        pub since: Option<Timestamp>,
        #[serde(serialize_with = "z_text")]
        pub until: Option<Timestamp>,
    }
}

/// The echo of the filters inside a query result: the datetimes print in the model's `Z` form.
pub fn echo(filters: &Filters<'_>) -> Value {
    filters.dict()
}
