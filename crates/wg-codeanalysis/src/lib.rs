//! wg-codeanalysis: the pure half of symbol extraction and change analysis.
//!
//! * [`symbols_from_facts`] turns a pre-order list of syntax-node facts into the functions and
//!   types of a file, named with dotted scopes as the Python analyser names them.
//! * [`parse_hunks`] turns a `git diff -U0` text into deleted and added line numbers.
//! * [`analyze_file`] extracts both sides of a file with a caller-supplied extractor and reports
//!   which symbols were added, modified or deleted ([`attribute_changes`] does the attribution).
//!
//! The tree-sitter parsing that produces the facts lives in `wg-codeparse`.

mod changes;
mod lang;
mod symbols;

pub use changes::{
    ANALYZER_VERSION, ChangeKind, FileStructure, Mode, SymbolChange, analyze_file,
    attribute_changes, parse_hunks,
};
pub use lang::{ALL_LANGUAGES, Language};
pub use symbols::{NodeFacts, Symbol, SymbolKind, symbols_from_facts};
