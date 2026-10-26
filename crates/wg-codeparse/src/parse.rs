//! Tree-sitter parsing: turns source text into the syntax-node facts the pure crate reads.

use tree_sitter::{Language as Grammar, LanguageError, Node, Parser};
use wg_codeanalysis::{ALL_LANGUAGES, Language, NodeFacts, Symbol, symbols_from_facts};

/// The symbols of `source` in pre-order, or `None` when the file does not parse. Errors only when
/// the grammar cannot be loaded.
pub fn extract_symbols(
    source: &[u8],
    language: Language,
) -> Result<Option<Vec<Symbol>>, LanguageError> {
    let grammar: Grammar = match language {
        Language::Python => tree_sitter_python::LANGUAGE.into(),
        Language::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Language::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Language::Go => tree_sitter_go::LANGUAGE.into(),
        Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        Language::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
    };
    let mut parser = Parser::new();
    parser.set_language(&grammar)?;
    let Some(tree) = parser.parse(source, None) else {
        return Ok(None);
    };
    let root = tree.root_node();
    let mut facts = Vec::new();
    collect(root, 0, source, &mut facts);
    Ok(symbols_from_facts(language, &facts, root.has_error()))
}

/// The names of the grammars that load, in [`ALL_LANGUAGES`] order.
pub fn available_languages() -> Vec<&'static str> {
    ALL_LANGUAGES
        .into_iter()
        .filter(|language| extract_symbols(b"", *language).is_ok())
        .map(Language::name)
        .collect()
}

/// Records the facts of `node` and its descendants in pre-order.
fn collect(node: Node, depth: usize, source: &[u8], facts: &mut Vec<NodeFacts>) {
    let field = |name: &str| {
        node.child_by_field_name(name)
            .map(|child| String::from_utf8_lossy(&source[child.byte_range()]).into_owned())
    };
    facts.push(NodeFacts {
        depth,
        kind: node.kind().to_owned(),
        start_row: node.start_position().row,
        end_row: node.end_position().row,
        name: field("name"),
        type_field: field("type"),
        trait_field: field("trait"),
    });
    for index in 0..node.child_count() {
        if let Some(child) = node.child(index) {
            collect(child, depth + 1, source, facts);
        }
    }
}
