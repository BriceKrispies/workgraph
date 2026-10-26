//! wg-codeparse: tree-sitter symbol extraction for six languages.
//!
//! This shell crate parses source with tree-sitter and hands the syntax-node facts to the pure
//! crate `wg-codeanalysis`, which decides what is a symbol and how changes are attributed.
//!
//! * [`extract_symbols`] lists the functions and types of one source file.
//! * [`analyze_file`] (in `wg-codeanalysis`, with [`extract_symbols`] as its extractor) reports
//!   which of them a diff added, modified or deleted.
//!
//! Each call creates its own tree-sitter parser, so the functions here share no state and may be
//! called from any thread.

mod parse;

pub use parse::{available_languages, extract_symbols};
pub use wg_codeanalysis::{
    ALL_LANGUAGES, ANALYZER_VERSION, ChangeKind, FileStructure, Language, Mode, NodeFacts, Symbol,
    SymbolChange, SymbolKind, analyze_file, parse_hunks,
};
