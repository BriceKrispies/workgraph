//! wg-intel: the decisions of the intelligence layer, as pure functions (Core, StrictPure).
//!
//! Ports `workgraph.classification.taxonomy`, the offline heuristic provider in
//! `workgraph.integrations.llm.heuristic`, the classifier evaluation metrics of
//! `workgraph.classification.service` (`compute_metrics`), and the search arithmetic of
//! `workgraph.search` (FTS5 query quoting, cosine similarity, reciprocal rank fusion). Every
//! function is a function of its arguments: model calls, the database and the clock stay in the
//! shell crates. JSON values mirror the Python dictionaries they replace.

mod classification;
mod embedding;
mod evaluation;
mod fts;
mod heuristic;
mod order;
mod pyrepr;
mod search;
mod similar;
mod taxonomy;

pub use similar::{Candidates, candidates, round4};

pub use classification::{
    ConfirmPlan, Confirmation, CurrentPlan, PromotePlan, RunRow, RunStatus, Superseded, TaskText,
    apply_current, confirm, evaluation_result, evaluation_row, gold_label, model_row,
    output_schema, promote, with_current,
};
pub use embedding::{
    content_digest, embedding_text, pack, repository_overlap, same_category, similar_score, unpack,
};
pub use evaluation::compute_metrics;
pub use fts::{FtsOptions, to_fts_query};
pub use heuristic::{
    CATEGORY_KEYWORDS, DOMAINS, EMBED_DIM, HeuristicError, MODEL, check_model, classify_text,
    decompose_text, generate_json, hash_embedding, list_models, narrative_from_facts,
};
pub use search::{FusedHit, RRF_K, cosine, fuse};
pub use taxonomy::{
    CATEGORIES, CATEGORY_DESCRIPTIONS, COMPLEXITY, PROMPT_VERSION, SYSTEM_PROMPT, SchemaError,
    TAXONOMY_VERSION, build_prompt, input_hash, schema_error_text, validate_classification,
    validate_classification_errors, validation_error_text,
};
