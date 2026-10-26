//! Path-based classification of changed files: language, class (`test`, `docs`, `config`,
//! `production`) and the directory "module" a file belongs to.
//!
//! Paths are treated the way Python's `PurePosixPath` reads them: empty and `.` segments are
//! dropped, and a leading `/` is a part of its own.

const DOC_EXTENSIONS: &[&str] = &[".md", ".rst", ".txt", ".adoc", ".mdx"];
const CONFIG_EXTENSIONS: &[&str] = &[
    ".toml",
    ".yaml",
    ".yml",
    ".json",
    ".ini",
    ".cfg",
    ".conf",
    ".properties",
    ".xml",
    ".lock",
    ".csproj",
    ".sln",
    ".props",
    ".targets",
    ".editorconfig",
    ".env",
];
const CONFIG_NAMES: &[&str] = &[
    "dockerfile",
    "makefile",
    "justfile",
    ".gitignore",
    ".gitattributes",
    "package.json",
    "pyproject.toml",
    "cargo.toml",
    "go.mod",
    "go.sum",
    "tsconfig.json",
    "pnpm-lock.yaml",
    "bun.lock",
    "uv.lock",
    ".dockerignore",
];
const TEST_DIRS: &[&str] = &[
    "test",
    "tests",
    "__tests__",
    "spec",
    "specs",
    "testing",
    "e2e",
];
const TEST_SUFFIXES: &[&str] = &["_test.py", "_test.go", "_spec.rb", "tests.cs", "test.cs"];
const DOC_DIRS: &[&str] = &["docs", "doc", "documentation"];
const MODULE_ROOTS: &[&str] = &["src", "lib", "pkg", "packages", "apps", "internal", "cmd"];

/// The language of a file, from its extension (case-insensitive), or `None`.
pub fn language_for(path: &str) -> Option<&'static str> {
    let name = posix_parts(path).last().copied().unwrap_or_default();
    let suffix = py_suffix(name).to_lowercase();
    match suffix.as_str() {
        ".py" | ".pyi" => Some("python"),
        ".cs" => Some("c_sharp"),
        ".ts" | ".mts" | ".cts" => Some("typescript"),
        ".tsx" => Some("tsx"),
        ".js" | ".jsx" | ".mjs" | ".cjs" => Some("javascript"),
        ".go" => Some("go"),
        ".rs" => Some("rust"),
        ".java" => Some("java"),
        ".kt" => Some("kotlin"),
        ".rb" => Some("ruby"),
        ".c" | ".h" => Some("c"),
        ".cpp" | ".hpp" => Some("cpp"),
        ".swift" => Some("swift"),
        _ => None,
    }
}

/// `test`, `docs`, `config` or `production`, from the lower-cased path.
pub fn file_class(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    let parts = posix_parts(&lower);
    let (name, dirs) = match parts.split_last() {
        Some((last, dirs)) if *last != "/" => (*last, dirs),
        _ => ("", &parts[..0]),
    };
    let suffix = py_suffix(name);
    let in_dirs = |names: &[&str]| dirs.iter().any(|dir| names.contains(dir));

    if in_dirs(TEST_DIRS)
        || name.starts_with("test_")
        || TEST_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
        || name.contains(".test.")
        || name.contains(".spec.")
    {
        return "test";
    }
    if DOC_EXTENSIONS.contains(&suffix)
        || in_dirs(DOC_DIRS)
        || matches!(name, "license" | "changelog")
    {
        return "docs";
    }
    if CONFIG_EXTENSIONS.contains(&suffix)
        || CONFIG_NAMES.contains(&name)
        || dirs.contains(&".github")
        || name.starts_with(".env")
    {
        return "config";
    }
    "production"
}

/// The directory "module" a file belongs to: its first `depth` directories. A file under one of
/// the source roots (`src`, `lib`, `packages`, ...) counts one more directory. A file with no
/// directory is in `/`.
pub fn module_for(path: &str, depth: usize) -> String {
    let parts = posix_parts(path);
    let dirs = &parts[..parts.len().saturating_sub(1)];
    let Some(first) = dirs.first() else {
        return "/".to_owned();
    };
    let depth = if MODULE_ROOTS.contains(first) && dirs.len() > 1 {
        depth + 1
    } else {
        depth
    };
    dirs[..depth.min(dirs.len())].join("/")
}

/// The parts of a POSIX path as `PurePosixPath(path).parts` gives them.
fn posix_parts(path: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    if path.starts_with('/') {
        parts.push("/");
    }
    parts.extend(
        path.split('/')
            .filter(|part| !part.is_empty() && *part != "."),
    );
    parts
}

/// `PurePath.suffix` of a file name: the text from its last dot, unless the dot is the first
/// character or the last one.
fn py_suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if dot > 0 && dot + 1 < name.len() => &name[dot..],
        _ => "",
    }
}
