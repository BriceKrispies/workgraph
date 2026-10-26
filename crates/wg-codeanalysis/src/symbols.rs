//! Symbols: which syntax nodes are functions or types, how they are named, and how they nest.

use crate::lang::Language;

/// What a symbol is. Methods, constructors and local functions count as functions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Function,
    Type,
}

impl SymbolKind {
    /// The name the Python analyser and the golden vectors use.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Type => "type",
        }
    }
}

/// A function or type with its 1-based inclusive line range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub kind: SymbolKind,
    pub name: String,
    pub start_line: usize,
    pub end_line: usize,
}

impl Symbol {
    /// Whether `line` (1-based) lies inside this symbol.
    pub fn contains(&self, line: usize) -> bool {
        self.start_line <= line && line <= self.end_line
    }

    /// The line span minus one, as the Python analyser measures it.
    pub fn size(&self) -> usize {
        self.end_line - self.start_line
    }
}

/// What the shell records about one syntax node, in pre-order.
///
/// `depth` is the node's distance from the root. `start_row` and `end_row` are 0-based. The three
/// field texts are the source text of the node's `name`, `type` and `trait` fields, when present.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeFacts {
    pub depth: usize,
    pub kind: String,
    pub start_row: usize,
    pub end_row: usize,
    pub name: Option<String>,
    pub type_field: Option<String>,
    pub trait_field: Option<String>,
}

/// The symbol kind of a syntax node type, per grammar. `None` for every other node.
fn symbol_kind(language: Language, node_type: &str) -> Option<SymbolKind> {
    use Language::{CSharp, Go, JavaScript, Python, Rust, Tsx, TypeScript};
    use SymbolKind::{Function, Type};
    match (language, node_type) {
        (Python, "function_definition") => Some(Function),
        (Python, "class_definition") => Some(Type),
        (
            JavaScript,
            "function_declaration" | "generator_function_declaration" | "method_definition",
        ) => Some(Function),
        (JavaScript, "class_declaration") => Some(Type),
        (
            TypeScript | Tsx,
            "function_declaration"
            | "generator_function_declaration"
            | "method_definition"
            | "method_signature",
        ) => Some(Function),
        (
            TypeScript | Tsx,
            "class_declaration"
            | "abstract_class_declaration"
            | "interface_declaration"
            | "type_alias_declaration"
            | "enum_declaration",
        ) => Some(Type),
        (Go, "function_declaration" | "method_declaration") => Some(Function),
        (Go, "type_spec") => Some(Type),
        (Rust, "function_item") => Some(Function),
        (Rust, "struct_item" | "enum_item" | "trait_item" | "impl_item" | "union_item") => {
            Some(Type)
        }
        (CSharp, "method_declaration" | "constructor_declaration" | "local_function_statement") => {
            Some(Function)
        }
        (
            CSharp,
            "class_declaration"
            | "struct_declaration"
            | "interface_declaration"
            | "record_declaration"
            | "enum_declaration",
        ) => Some(Type),
        _ => None,
    }
}

/// The bare name of a symbol node (not scope-qualified).
fn bare_name(language: Language, node: &NodeFacts) -> String {
    if let Some(name) = &node.name {
        return name.clone();
    }
    if language == Language::Rust
        && node.kind == "impl_item"
        && let Some(target) = &node.type_field
    {
        return match &node.trait_field {
            Some(trait_name) => format!("impl {trait_name} for {target}"),
            None => format!("impl {target}"),
        };
    }
    format!("<anonymous {}@{}>", node.kind, node.start_row + 1)
}

/// The symbols of a file from its node facts, in pre-order.
///
/// A symbol node is named by its enclosing symbols' bare names joined with `.`. Facts are in
/// pre-order, so the enclosing symbols of a node are the symbols above it whose depth is lower.
/// Returns `None` when the tree has errors and no symbols were found (the file does not parse).
pub fn symbols_from_facts(
    language: Language,
    facts: &[NodeFacts],
    root_has_error: bool,
) -> Option<Vec<Symbol>> {
    let mut symbols = Vec::new();
    let mut scopes: Vec<(usize, String)> = Vec::new();
    for node in facts {
        while scopes.last().is_some_and(|(depth, _)| *depth >= node.depth) {
            scopes.pop();
        }
        let Some(kind) = symbol_kind(language, &node.kind) else {
            continue;
        };
        let own = bare_name(language, node);
        let name = scopes
            .iter()
            .map(|(_, scope)| scope.as_str())
            .chain(std::iter::once(own.as_str()))
            .collect::<Vec<_>>()
            .join(".");
        symbols.push(Symbol {
            kind,
            name,
            start_line: node.start_row + 1,
            end_line: node.end_row + 1,
        });
        scopes.push((node.depth, own));
    }
    if root_has_error && symbols.is_empty() {
        return None;
    }
    Some(symbols)
}
