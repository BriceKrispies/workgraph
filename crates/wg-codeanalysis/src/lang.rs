//! The grammars the analyser knows, named as the Python analyser names them.

/// A supported grammar. `Tsx` is the TypeScript grammar with JSX.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    Rust,
    CSharp,
}

/// Every supported grammar, in the order the Python loader table lists them.
pub const ALL_LANGUAGES: [Language; 7] = [
    Language::Python,
    Language::JavaScript,
    Language::TypeScript,
    Language::Tsx,
    Language::Go,
    Language::Rust,
    Language::CSharp,
];

impl Language {
    /// The language for a Python-style name (`"c_sharp"`, `"tsx"`, ...), or `None` if unsupported.
    pub fn from_name(name: &str) -> Option<Self> {
        ALL_LANGUAGES
            .into_iter()
            .find(|language| language.name() == name)
    }

    /// The Python-style name of this language.
    pub fn name(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::Tsx => "tsx",
            Self::Go => "go",
            Self::Rust => "rust",
            Self::CSharp => "c_sharp",
        }
    }
}
