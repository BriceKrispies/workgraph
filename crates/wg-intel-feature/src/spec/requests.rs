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
