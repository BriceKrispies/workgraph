//! Pure: the statement text, row decoding and output shapes of the classification and search
//! families, and the arguments and human views of the `classify` and `search` commands.

pub mod args;
pub mod classify_out;
pub mod classify_sql;
pub mod llm;
pub mod payload;
pub mod plan;
pub mod requests;
pub mod search_out;
pub mod search_sql;
pub mod semantic_out;
pub mod similar_sql;
pub mod view;
pub mod write_out;
pub mod write_view;
