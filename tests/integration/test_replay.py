from __future__ import annotations

import json
import sys
from collections.abc import Callable
from dataclasses import replace
from pathlib import Path

import pytest

from tests.conftest import GitRepo
from workgraph.experiments.replay import (
    PYTHON_TEST_GLOBS,
    RUST_TEST_GLOBS,
    ReplayConfig,
    analyze_commit,
    create_base,
    match_glob,
    resolve_oracle,
    scan,
    validate,
    worktree_changes,
)

MOD_BASE = "def f():\n    return 1\n"
MOD_WITH_G = "def f():\n    return 1\n\n\ndef g():\n    return 2\n"
TEST_G = "from pkg.mod import g\n\n\ndef test_g():\n    assert g() == 2\n"


def _py_repo(make_repo: Callable[..., GitRepo], name: str) -> GitRepo:
    repo = make_repo(name)
    repo.commit(
        "add project",
        {
            "pyproject.toml": '[project]\nname = "demo"\nversion = "0.1.0"\n',
            "pkg/__init__.py": "",
            "pkg/mod.py": MOD_BASE,
        },
    )
    return repo


def _pytest_config(repo: GitRepo) -> ReplayConfig:
    return replace(
        ReplayConfig.for_repository(repo.path),
        oracle_template=(sys.executable, "-m", "pytest", "-q", "{test_files}"),
    )


def _branch(sha: str) -> str:
    return f"wg/replay/{sha[:12]}"


def _worktrees(repo: GitRepo) -> list[str]:
    return [
        line
        for line in repo.git("worktree", "list", "--porcelain").splitlines()
        if line.startswith("worktree ")
    ]


# ---------------------------------------------------------------- configuration and glob helper


@pytest.mark.parametrize(
    ("path", "pattern", "expected"),
    [
        ("tests/test_a.py", "tests/**/*.py", True),
        ("tests/a/b/test_a.py", "tests/**/*.py", True),
        ("pkg/tests/x.py", "tests/**/*.py", False),
        ("test_x.py", "**/test_*.py", True),
        ("a/b/test_x.py", "**/test_*.py", True),
        ("a/b/x.py", "**/test_*.py", False),
        ("pkg/sub/mod_test.py", "**/*_test.py", True),
        ("crates/foo/src/x_tests.rs", "**/*_tests.rs", True),
        ("tests/bar.rs", "**/tests/**/*.rs", True),
        ("crates/foo/tests/common/mod.rs", "**/tests/**/*.rs", True),
        ("crates/foo/src/lib.rs", "**/tests/**/*.rs", False),
        ("a/b/c.rs", "*.rs", False),
        ("a/b/c.rs", "a/*.rs", False),
        ("a/b.rs", "a/*.rs", True),
        ("generated/x.py", "**/generated/**", True),
        ("a/generated/b/c.py", "**/generated/**", True),
        ("Cargo.lock", "Cargo.lock", True),
        ("sub/Cargo.lock", "Cargo.lock", False),
    ],
)
def test_match_glob(path: str, pattern: str, expected: bool) -> None:
    assert match_glob(path, pattern) is expected


def test_for_repository_defaults(make_repo: Callable[..., GitRepo]) -> None:
    py = _py_repo(make_repo, "defaults-py")
    config = ReplayConfig.for_repository(py.path)
    assert config.test_globs == PYTHON_TEST_GLOBS
    assert config.source_suffixes == (".py",)
    assert "Cargo.lock" in config.ignore_globs and "**/generated/**" in config.ignore_globs
    assert config.oracle_template is None

    rust = make_repo("defaults-rust")
    rust.commit("cargo", {"Cargo.toml": "[workspace]\nmembers = []\n"})
    rust_config = ReplayConfig.for_repository(rust.path)
    assert rust_config.test_globs == RUST_TEST_GLOBS
    assert rust_config.source_suffixes == (".rs", ".rho")


# ---------------------------------------------------------------- Python repository


def test_python_feature_commit_is_eligible(make_repo: Callable[..., GitRepo]) -> None:
    repo = _py_repo(make_repo, "py-feature")
    sha = repo.commit("Add g", {"pkg/mod.py": MOD_WITH_G, "tests/test_mod.py": TEST_G})

    candidate = analyze_commit(repo.path, sha)

    assert candidate.excluded == ()
    assert candidate.parent == repo.git("rev-parse", f"{sha}^")
    assert candidate.subject == "Add g"
    assert candidate.test_paths == ("tests/test_mod.py",)
    assert candidate.source_paths == ("pkg/mod.py",)
    assert candidate.features.src_lines == 4
    assert candidate.features.test_lines == 5
    assert candidate.features.src_files == 1
    assert candidate.features.test_files == 1
    assert candidate.features.packages == ("",)
    assert candidate.features.band == 2
    assert candidate.oracle == (("python", "-m", "pytest", "-q", "tests/test_mod.py"),)
    assert json.loads(json.dumps(candidate.as_dict()))["features"]["band"] == 2


def test_python_exclusions(make_repo: Callable[..., GitRepo]) -> None:
    repo = _py_repo(make_repo, "py-exclusions")

    tests_only = repo.commit("tests only", {"tests/test_only.py": "def test_only():\n    assert True\n"})
    source_only = repo.commit("source only", {"pkg/extra.py": "X = 1\n"})
    formatting = repo.commit(
        "whitespace",
        {
            "pkg/mod.py": "def f():\n    return 1  \n",
            "tests/test_fmt.py": "def test_fmt():\n    assert True\n",
        },
    )
    inline = repo.commit(
        "inline test",
        {
            "pkg/mod.py": "def f():\n    return 1  \n\n\ndef test_f():\n    assert f() == 1\n",
            "tests/test_inline.py": "def test_inline():\n    assert True\n",
        },
    )
    repo.git("mv", "pkg/extra.py", "pkg/extra2.py")
    renamed = repo.commit("rename", {})

    assert "no_source" in analyze_commit(repo.path, tests_only).excluded
    assert "no_tests" in analyze_commit(repo.path, source_only).excluded
    assert "formatting_only" in analyze_commit(repo.path, formatting).excluded
    assert "inline_tests" in analyze_commit(repo.path, inline).excluded
    rename = analyze_commit(repo.path, renamed)
    assert "rename_only" in rename.excluded
    assert "no_source" in rename.excluded

    root = analyze_commit(repo.path, repo.git("rev-list", "--max-parents=0", "HEAD"))
    assert "root" in root.excluded
    assert root.parent is None

    repo.git("checkout", "-q", "-b", "side")
    repo.commit("side work", {"pkg/side.py": "S = 1\n"})
    repo.git("checkout", "-q", "main")
    repo.commit("main work", {"pkg/main.py": "M = 1\n"})
    repo.git("merge", "-q", "--no-ff", "-m", "merge side", "side")
    merge = analyze_commit(repo.path, repo.head())
    assert "merge" in merge.excluded


# ---------------------------------------------------------------- Rust-shaped repository


def _rust_repo(make_repo: Callable[..., GitRepo]) -> GitRepo:
    repo = make_repo("rust-shape")
    repo.commit(
        "crate",
        {
            "Cargo.toml": '[workspace]\nmembers = ["crates/foo"]\n',
            "crates/foo/Cargo.toml": '[package]\nname = "foo-core"\nversion = "0.1.0"\n',
            "crates/foo/src/lib.rs": "pub fn one() -> u32 {\n    1\n}\n",
            "crates/foo/tests/common/mod.rs": "pub fn helper() {}\n",
        },
    )
    return repo


RUST_FEATURE_FILES = {
    "crates/foo/src/lib.rs": "pub fn one() -> u32 {\n    1\n}\n\npub fn two() -> u32 {\n    2\n}\n",
    "crates/foo/src/x_tests.rs": "use crate::two;\n\n#[test]\nfn two_is_two() { assert_eq!(two(), 2); }\n",
    "crates/foo/tests/bar.rs": "use foo_core::two;\n\n#[test]\nfn bar() { assert_eq!(two(), 2); }\n",
}


def test_rust_oracle_resolution(make_repo: Callable[..., GitRepo]) -> None:
    repo = _rust_repo(make_repo)
    sha = repo.commit("Add two", RUST_FEATURE_FILES)
    config = ReplayConfig.for_repository(repo.path)

    candidate = analyze_commit(repo.path, sha, config)

    assert candidate.excluded == ()
    assert candidate.features.packages == ("crates/foo",)
    assert candidate.features.src_lines == 4
    assert candidate.features.test_lines == 8
    assert candidate.test_paths == ("crates/foo/src/x_tests.rs", "crates/foo/tests/bar.rs")
    assert candidate.oracle == (
        ("cargo", "test", "-p", "foo-core", "--lib"),
        ("cargo", "test", "-p", "foo-core", "--test", "bar"),
    )
    resolved = resolve_oracle(
        repo.path,
        sha,
        ("crates/foo/tests/bar.rs", "crates/foo/tests/gone.rs", "crates/foo/tests/common/mod.rs"),
        config,
    )
    assert resolved == (("cargo", "test", "-p", "foo-core", "--test", "bar"),)


def test_rust_helper_only_commit_has_no_oracle(make_repo: Callable[..., GitRepo]) -> None:
    repo = _rust_repo(make_repo)
    sha = repo.commit(
        "helper only",
        {
            "crates/foo/tests/common/mod.rs": "pub fn helper() -> u32 {\n    3\n}\n",
            "crates/foo/src/lib.rs": "pub fn one() -> u32 {\n    1\n}\n\npub fn three() -> u32 {\n    3\n}\n",
        },
    )

    candidate = analyze_commit(repo.path, sha)

    assert candidate.oracle == ()
    assert "no_oracle" in candidate.excluded


# ---------------------------------------------------------------- scan


def test_scan_respects_range_limit_and_writes_nothing(make_repo: Callable[..., GitRepo]) -> None:
    repo = _py_repo(make_repo, "py-scan")
    shas: dict[str, str] = {}
    for name, size in (("a", 1), ("b", 4), ("c", 10)):
        content = "".join(f"V{i} = {i}\n" for i in range(size))
        shas[name] = repo.commit(
            f"add {name}", {f"pkg/{name}.py": content, f"tests/test_{name}.py": "def test_x():\n    pass\n"}
        )
    tests_only = repo.commit("tests only", {"tests/test_d.py": "def test_d():\n    pass\n"})

    before = (repo.head(), repo.git("for-each-ref"), repo.git("status", "--porcelain"))

    assert [c.sha for c in scan(repo.path)] == [shas["c"], shas["b"], shas["a"]]
    assert [c.sha for c in scan(repo.path, min_lines=2, max_lines=8)] == [shas["b"]]
    assert [c.sha for c in scan(repo.path, limit=2)] == [shas["c"], shas["b"]]
    assert [c.sha for c in scan(repo.path, min_lines=0, include_excluded=True, limit=1)] == [tests_only]

    assert (repo.head(), repo.git("for-each-ref"), repo.git("status", "--porcelain")) == before


# ---------------------------------------------------------------- create_base


def test_create_base_contents(make_repo: Callable[..., GitRepo]) -> None:
    repo = _py_repo(make_repo, "py-base")
    sha = repo.commit("Add g", {"pkg/mod.py": MOD_WITH_G, "tests/test_mod.py": TEST_G})
    candidate = analyze_commit(repo.path, sha)
    parent = candidate.parent
    assert parent is not None

    (repo.path / "pkg" / "mod.py").write_text("LOCAL EDIT\n", encoding="utf-8")
    (repo.path / "scratch.txt").write_text("untracked\n", encoding="utf-8")
    status_before = repo.git("status", "--porcelain")
    index_before = repo.git("ls-files", "-s")

    branch = create_base(repo.path, candidate)

    assert branch == _branch(sha)
    assert repo.git("rev-parse", f"refs/heads/{branch}")
    assert repo.git("rev-parse", f"{branch}^") == parent
    assert repo.git("show", f"{branch}:tests/test_mod.py") == repo.git("show", f"{sha}:tests/test_mod.py")
    assert repo.git("show", f"{branch}:pkg/mod.py") == repo.git("show", f"{parent}:pkg/mod.py")

    assert repo.git("status", "--porcelain") == status_before
    assert repo.git("ls-files", "-s") == index_before
    assert (repo.path / "pkg" / "mod.py").read_text(encoding="utf-8") == "LOCAL EDIT\n"

    ref_before = repo.git("rev-parse", f"refs/heads/{branch}")
    assert create_base(repo.path, candidate) == branch
    assert repo.git("rev-parse", f"refs/heads/{branch}") == ref_before


def test_create_base_removes_deleted_test_file(make_repo: Callable[..., GitRepo]) -> None:
    repo = _py_repo(make_repo, "py-base-delete")
    repo.commit("old test", {"tests/test_old.py": "def test_old():\n    assert True\n"})
    (repo.path / "tests" / "test_old.py").unlink()
    sha = repo.commit("replace test", {"pkg/mod.py": MOD_WITH_G, "tests/test_mod.py": TEST_G})
    candidate = analyze_commit(repo.path, sha)
    assert "tests/test_old.py" in candidate.test_paths

    branch = create_base(repo.path, candidate)

    files = repo.git("ls-tree", "-r", "--name-only", branch).splitlines()
    assert "tests/test_old.py" not in files
    assert repo.git("show", f"{branch}:tests/test_mod.py") == repo.git("show", f"{sha}:tests/test_mod.py")


def test_create_base_conflict_raises(make_repo: Callable[..., GitRepo]) -> None:
    repo = _py_repo(make_repo, "py-base-conflict")
    sha = repo.commit("Add g", {"pkg/mod.py": MOD_WITH_G, "tests/test_mod.py": TEST_G})
    candidate = analyze_commit(repo.path, sha)
    repo.git("update-ref", f"refs/heads/{_branch(sha)}", repo.git("rev-list", "--max-parents=0", "HEAD"))

    with pytest.raises(ValueError, match="different tree"):
        create_base(repo.path, candidate)


# ---------------------------------------------------------------- validate


def test_validate_accepts_red_base_and_green_solution(
    make_repo: Callable[..., GitRepo], tmp_path: Path
) -> None:
    repo = _py_repo(make_repo, "py-validate")
    sha = repo.commit("Add g", {"pkg/mod.py": MOD_WITH_G, "tests/test_mod.py": TEST_G})
    candidate = analyze_commit(repo.path, sha, _pytest_config(repo))
    branch = create_base(repo.path, candidate)
    workdir = tmp_path / "work"
    workdir.mkdir()

    result = validate(repo.path, candidate, branch, workdir=workdir)

    assert result.ok is True
    assert result.reason is None
    assert all(code != 0 for code in result.base)
    assert result.solution == (0,)
    assert len(_worktrees(repo)) == 1
    assert list(workdir.iterdir()) == []


def test_validate_rejects_green_base(make_repo: Callable[..., GitRepo], tmp_path: Path) -> None:
    repo = _py_repo(make_repo, "py-green")
    sha = repo.commit(
        "Add g without a red test",
        {
            "pkg/mod.py": MOD_WITH_G,
            "tests/test_mod.py": "from pkg.mod import f\n\n\ndef test_f():\n    assert f() == 1\n",
        },
    )
    candidate = analyze_commit(repo.path, sha, _pytest_config(repo))
    branch = create_base(repo.path, candidate)
    workdir = tmp_path / "work"
    workdir.mkdir()

    result = validate(repo.path, candidate, branch, workdir=workdir)

    assert result.ok is False
    assert result.reason == "oracle_green_at_base"
    assert result.solution and all(code == 0 for code in result.solution)
    assert len(_worktrees(repo)) == 1
    assert list(workdir.iterdir()) == []


def test_validate_rejects_red_solution(make_repo: Callable[..., GitRepo], tmp_path: Path) -> None:
    repo = _py_repo(make_repo, "py-red")
    sha = repo.commit(
        "Add g with a wrong test",
        {
            "pkg/mod.py": MOD_WITH_G,
            "tests/test_mod.py": "from pkg.mod import f\n\n\ndef test_f():\n    assert f() == 5\n",
        },
    )
    candidate = analyze_commit(repo.path, sha, _pytest_config(repo))
    branch = create_base(repo.path, candidate)
    workdir = tmp_path / "work"
    workdir.mkdir()

    result = validate(repo.path, candidate, branch, workdir=workdir)

    assert result.ok is False
    assert result.reason == "oracle_red_at_solution"
    assert all(code != 0 for code in result.solution)
    assert result.base == ()  # the solution runs first, so a red solution skips the base run
    assert len(_worktrees(repo)) == 1


# ---------------------------------------------------------------- worktree_changes


def _worktree_repo(make_repo: Callable[..., GitRepo], name: str) -> tuple[GitRepo, str]:
    repo = _py_repo(make_repo, name)
    base = repo.commit(
        "tests and ignores",
        {
            "tests/test_mod.py": "def test_f():\n    assert True\n",
            ".gitignore": "__pycache__/\n.pytest_cache/\n",
        },
    )
    return repo, base


def test_worktree_changes_counts_committed_uncommitted_and_untracked(
    make_repo: Callable[..., GitRepo],
) -> None:
    repo, base = _worktree_repo(make_repo, "py-worktree-counts")
    repo.commit("Add g", {"pkg/mod.py": MOD_WITH_G})
    repo.write("pkg/mod.py", MOD_WITH_G + "X = 3\n")
    repo.write("pkg/extra.py", "E = 1\nF = 2\n")
    repo.write(".pytest_cache/junk.py", "J = 1\n" * 50)

    changes = worktree_changes(repo.path, base)

    assert changes.features.src_lines == 7
    assert changes.features.src_files == 2
    assert changes.features.test_files == 0
    assert changes.features.band == 2
    assert changes.features.packages == ("",)
    assert changes.added_source_lines["pkg/extra.py"] == ("E = 1", "F = 2")
    assert "X = 3" in changes.added_source_lines["pkg/mod.py"]
    assert ".pytest_cache/junk.py" not in changes.added_source_lines
    assert changes.tests_modified == ()


def test_worktree_changes_lists_added_stubs(make_repo: Callable[..., GitRepo]) -> None:
    repo, base = _worktree_repo(make_repo, "py-worktree-stubs")
    repo.write("pkg/stub.py", 'def a():\n    raise NotImplementedError\n\n\nX = todo!("later")\n')

    changes = worktree_changes(repo.path, base)

    assert changes.stubs == (
        ("pkg/stub.py", "    raise NotImplementedError"),
        ("pkg/stub.py", 'X = todo!("later")'),
    )


def test_worktree_changes_lists_modified_test_file(make_repo: Callable[..., GitRepo]) -> None:
    repo, base = _worktree_repo(make_repo, "py-worktree-modified")
    repo.write("tests/test_mod.py", TEST_G)

    assert worktree_changes(repo.path, base).tests_modified == ("tests/test_mod.py",)


def test_worktree_changes_lists_deleted_test_file(make_repo: Callable[..., GitRepo]) -> None:
    repo, base = _worktree_repo(make_repo, "py-worktree-deleted")
    (repo.path / "tests" / "test_mod.py").unlink()

    assert worktree_changes(repo.path, base).tests_modified == ("tests/test_mod.py",)


def test_worktree_changes_ignores_new_test_file(make_repo: Callable[..., GitRepo]) -> None:
    repo, base = _worktree_repo(make_repo, "py-worktree-new")
    repo.write("tests/test_new.py", TEST_G)

    assert worktree_changes(repo.path, base).tests_modified == ()


def test_worktree_changes_lists_old_path_of_renamed_test_file(make_repo: Callable[..., GitRepo]) -> None:
    repo, base = _worktree_repo(make_repo, "py-worktree-renamed")
    repo.git("mv", "tests/test_mod.py", "tests/test_renamed.py")

    assert worktree_changes(repo.path, base).tests_modified == ("tests/test_mod.py",)


def test_worktree_changes_leaves_worktree_and_index_untouched(make_repo: Callable[..., GitRepo]) -> None:
    repo, base = _worktree_repo(make_repo, "py-worktree-untouched")
    repo.commit("Add g", {"pkg/mod.py": MOD_WITH_G})
    repo.write("pkg/mod.py", MOD_WITH_G + "X = 3\n")
    repo.write("pkg/extra.py", "E = 1\n")

    def snapshot() -> tuple[str, bytes, bytes]:
        status = repo.git("status", "--porcelain")
        index = (repo.path / ".git" / "index").read_bytes()
        return status, index, (repo.path / "pkg" / "extra.py").read_bytes()

    before = snapshot()
    worktree_changes(repo.path, base)

    assert snapshot() == before
