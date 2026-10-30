//! The arguments of the classification writes: a classification run, a human confirmation and a
//! bulk reclassification, as the commands and the jobs give them.

/// The options of one classification run (`run_classification`'s keyword arguments).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunOptions {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub promote: bool,
    pub batch_id: Option<wg_types::Id>,
}

/// The arguments of `ClassificationService.confirm`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmRequest {
    pub category: String,
    pub subtype: Option<String>,
    pub domain: Option<String>,
    pub complexity: Option<String>,
    pub tags: Vec<String>,
    pub note: String,
    pub add_to_gold: bool,
}

/// The arguments of `SearchService.search`: the text, the mode, the limit and the filters.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchRequest {
    pub query: String,
    pub mode: String,
    pub limit: i64,
    pub filters: crate::spec::search_out::SearchFilters,
}

/// The arguments of `ClassificationService.reclassify`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReclassifyRequest {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub promote: bool,
    pub category: Option<String>,
}

/// A date-time request parameter as a UTC instant, absent when not given. Naive values read as UTC,
/// as the analytics readers do.
pub fn instant_param(
    params: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<wg_types::Timestamp>, wg_types::WgError> {
    let Some(text) = params.get(key).and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let at = wg_clispec::datetime::parse_click_datetime(text).map_err(|message| {
        wg_types::WgError::validation(format!("invalid datetime '{text}': {message}"))
    })?;
    wg_types::Timestamp::from_components(
        at.year,
        at.month,
        at.day,
        at.hour,
        at.minute,
        at.second,
        at.microsecond,
    )
    .map(Some)
}
