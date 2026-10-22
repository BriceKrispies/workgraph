"""Path-based classification of changed files (language, role, module)."""

from __future__ import annotations

from pathlib import PurePosixPath

EXTENSION_LANGUAGES = {
    ".py": "python",
    ".pyi": "python",
    ".cs": "c_sharp",
    ".ts": "typescript",
    ".mts": "typescript",
    ".cts": "typescript",
    ".tsx": "tsx",
    ".js": "javascript",
    ".jsx": "javascript",
    ".mjs": "javascript",
    ".cjs": "javascript",
    ".go": "go",
    ".rs": "rust",
    ".java": "java",
    ".kt": "kotlin",
    ".rb": "ruby",
    ".c": "c",
    ".h": "c",
    ".cpp": "cpp",
    ".hpp": "cpp",
    ".swift": "swift",
}

DOC_EXTENSIONS = {".md", ".rst", ".txt", ".adoc", ".mdx"}
CONFIG_EXTENSIONS = {
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
}
CONFIG_NAMES = {
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
}


def language_for(path: str) -> str | None:
    return EXTENSION_LANGUAGES.get(PurePosixPath(path).suffix.lower())


def file_class(path: str) -> str:
    """One of ``test``, ``docs``, ``config`` or ``production``."""
    p = PurePosixPath(path.lower())
    name = p.name
    parts = set(p.parts[:-1])
    if (
        parts & {"test", "tests", "__tests__", "spec", "specs", "testing", "e2e"}
        or name.startswith("test_")
        or name.endswith(("_test.py", "_test.go", "_spec.rb", "tests.cs", "test.cs"))
        or ".test." in name
        or ".spec." in name
    ):
        return "test"
    if (
        p.suffix in DOC_EXTENSIONS
        or parts & {"docs", "doc", "documentation"}
        or name in {"license", "changelog"}
    ):
        return "docs"
    if p.suffix in CONFIG_EXTENSIONS or name in CONFIG_NAMES or ".github" in parts or name.startswith(".env"):
        return "config"
    return "production"


def module_for(path: str, depth: int = 2) -> str:
    """The directory "module" a file belongs to (first ``depth`` directories)."""
    parts = PurePosixPath(path).parts[:-1]
    if not parts:
        return "/"
    if parts[0] in {"src", "lib", "pkg", "packages", "apps", "internal", "cmd"} and len(parts) > 1:
        depth += 1
    return "/".join(parts[:depth])
