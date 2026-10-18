"""Replay tasks: turn a repository's own commits into red-test tasks with a known solution size.

A commit that changes source and tests together yields a base branch (the parent plus only the
commit's test changes) and an oracle (the commit's tests). The commit itself is the reference
solution, so its non-test source size is known.
"""

from __future__ import annotations

import re
import shutil
import subprocess
import tempfile
import tomllib
from collections.abc import Generator, Iterator, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
from fnmatch import fnmatchcase
from pathlib import Path

from workgraph.integrations.git.cli import Git, validate_ref

RUST_TEST_GLOBS = ("**/tests/**/*.rs", "**/*_tests.rs")
RUST_SOURCE_SUFFIXES = (".rs", ".rho")
PYTHON_TEST_GLOBS = ("tests/**/*.py", "**/test_*.py", "**/*_test.py")
PYTHON_SOURCE_SUFFIXES = (".py",)
IGNORE_GLOBS = (
    "Cargo.lock",
    "uv.lock",
    "poetry.lock",
    "package-lock.json",
    "pnpm-lock.yaml",
    "**/generated/**",
)
MANIFESTS = ("Cargo.toml", "pyproject.toml", "package.json")


@dataclass(frozen=True)
class ReplayConfig:
    """Per-repository classification of changed files and the oracle command template."""

    test_globs: tuple[str, ...]
    source_suffixes: tuple[str, ...]
    ignore_globs: tuple[str, ...] = IGNORE_GLOBS
    oracle_template: tuple[str, ...] | None = None

    @classmethod
    def for_repository(cls, path: Path) -> ReplayConfig:
        """Defaults for a Rust (``Cargo.toml``) and/or Python (``pyproject.toml``) repository root."""
        globs: list[str] = []
        suffixes: list[str] = []
        if (path / "Cargo.toml").is_file():
            globs.extend(RUST_TEST_GLOBS)
            suffixes.extend(RUST_SOURCE_SUFFIXES)
        if (path / "pyproject.toml").is_file():
            globs.extend(PYTHON_TEST_GLOBS)
            suffixes.extend(PYTHON_SOURCE_SUFFIXES)
        return cls(
            test_globs=tuple(dict.fromkeys(globs)),
            source_suffixes=tuple(dict.fromkeys(suffixes)),
            ignore_globs=IGNORE_GLOBS,
        )


def match_glob(path: str, pattern: str) -> bool:
    """Match a forward-slash path against a root-anchored glob.

    ``*`` and ``?`` never cross a ``/``; ``**`` as a whole segment matches any number of segments,
    zero included, so ``**/test_*.py`` matches ``test_x.py`` and ``a/b/test_x.py``.
    """
    return _match_segments(path.split("/"), pattern.split("/"))


def _match_segments(path: list[str], pattern: list[str]) -> bool:
    if not pattern:
        return not path
    head, rest = pattern[0], pattern[1:]
    if head == "**":
        return any(_match_segments(path[i:], rest) for i in range(len(path) + 1))
    return bool(path) and fnmatchcase(path[0], head) and _match_segments(path[1:], rest)


@dataclass(frozen=True)
class ReplayFeatures:
    """Size and shape of a commit's change. ``src_lines`` is the primary, log2-banded size."""

    src_lines: int
    test_lines: int
    src_files: int
    test_files: int
    packages: tuple[str, ...]
    band: int | None

    def as_dict(self) -> dict[str, object]:
        return {
            "src_lines": self.src_lines,
            "test_lines": self.test_lines,
            "src_files": self.src_files,
            "test_files": self.test_files,
            "packages": list(self.packages),
            "band": self.band,
        }


@dataclass(frozen=True)
class ReplayCandidate:
    """A commit analysed as a replay task. ``excluded`` lists the reasons; empty means eligible."""

    sha: str
    parent: str | None
    subject: str
    body: str
    test_paths: tuple[str, ...]
    source_paths: tuple[str, ...]
    features: ReplayFeatures
    oracle: tuple[tuple[str, ...], ...]
    excluded: tuple[str, ...]

    def as_dict(self) -> dict[str, object]:
        return {
            "sha": self.sha,
            "parent": self.parent,
            "subject": self.subject,
            "body": self.body,
            "test_paths": list(self.test_paths),
            "source_paths": list(self.source_paths),
            "features": self.features.as_dict(),
            "oracle": [list(command) for command in self.oracle],
            "excluded": list(self.excluded),
        }


@dataclass(frozen=True)
class Validation:
    """Exit codes of each oracle command at the base and at the real commit."""

    ok: bool
    reason: str | None
    base: tuple[int, ...]
    solution: tuple[int, ...]


@dataclass(frozen=True)
class _Change:
    """One non-binary file in a numstat listing (``path`` is the post-image path)."""

    path: str
    added: int
    removed: int
    old_path: str | None = None

    @property
    def lines(self) -> int:
        return self.added + self.removed


def _kind(path: str, config: ReplayConfig) -> str:
    if any(match_glob(path, pattern) for pattern in config.ignore_globs):
        return "ignored"
    if any(match_glob(path, pattern) for pattern in config.test_globs):
        return "test"
    if path.endswith(config.source_suffixes):
        return "source"
    return "other"


def _band(lines: int) -> int | None:
    return lines.bit_length() - 1 if lines > 0 else None


def _changes(git: Git, sha: str, parent: str | None, *flags: str) -> str:
    """Diff of ``sha`` against its first parent; a root commit is shown against the empty tree."""
    if parent is None:
        return git.out("show", "--format=", *flags, sha)
    return git.out("diff", *flags, parent, sha)


def _parse_numstat(output: str) -> list[_Change]:
    """Parse ``--numstat -z``: ``added TAB removed TAB path NUL``, or for renames ``... TAB NUL old NUL new NUL``."""
    tokens = output.split("\0")
    changes: list[_Change] = []
    i = 0
    while i < len(tokens):
        token = tokens[i].lstrip("\n")
        i += 1
        if not token:
            continue
        added, removed, path = token.split("\t", 2)
        old_path: str | None = None
        if not path:
            old_path, path = tokens[i], tokens[i + 1]
            i += 2
        if added == "-" or removed == "-":
            continue  # binary files carry no line counts
        changes.append(_Change(path, int(added), int(removed), old_path))
    return changes


def _added_lines(patch: str) -> dict[str, list[str]]:
    """Added line contents (without the ``+``) per post-image path, from a zero-context patch."""
    added: dict[str, list[str]] = {}
    current: str | None = None
    in_hunk = False
    for line in patch.split("\n"):
        if line.startswith("diff --git "):
            current, in_hunk = None, False
        elif line.startswith("@@"):
            in_hunk = True
        elif in_hunk:
            if line.startswith("+") and current is not None:
                added.setdefault(current, []).append(line[1:])
        elif line.startswith("+++ b/"):
            current = line[len("+++ b/") :]
    return added


def _has_inline_tests(added: dict[str, list[str]], sources: Sequence[_Change]) -> bool:
    for change in sources:
        is_python = change.path.endswith(".py")
        for line in added.get(change.path, []):
            if "#[test]" in line or "#[cfg(test)]" in line:
                return True
            if is_python and line.lstrip().startswith("def test_"):
                return True
    return False


def _tree_files(git: Git, sha: str) -> frozenset[str]:
    return frozenset(path for path in git.out("ls-tree", "-r", "-z", "--name-only", sha).split("\0") if path)


def _manifest_dirs(files: frozenset[str], names: Sequence[str]) -> frozenset[str]:
    return frozenset(path.rpartition("/")[0] for path in files if path.rpartition("/")[2] in names)


def _nearest(path: str, dirs: frozenset[str]) -> str | None:
    """The nearest directory at or above the file's directory that is in ``dirs`` ("" is the root)."""
    current = path.rpartition("/")[0]
    while True:
        if current in dirs:
            return current
        if not current:
            return None
        current = current.rpartition("/")[0]


def _package_name(git: Git, sha: str, package_dir: str) -> str | None:
    data = git.show_file(sha, f"{package_dir}/Cargo.toml" if package_dir else "Cargo.toml")
    if data is None:
        return None
    try:
        manifest = tomllib.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, tomllib.TOMLDecodeError):
        return None
    package = manifest.get("package", {})
    name = package.get("name") if isinstance(package, dict) else None
    return name if isinstance(name, str) else None


def _resolve_oracle(
    git: Git,
    sha: str,
    test_paths: Sequence[str],
    config: ReplayConfig,
    files: frozenset[str],
) -> tuple[tuple[str, ...], ...]:
    existing = tuple(path for path in test_paths if path in files)
    if config.oracle_template is not None:
        if not existing:
            return ()
        manifests = _manifest_dirs(files, MANIFESTS)
        packages = tuple(dict.fromkeys(_nearest(path, manifests) or "." for path in existing))
        argv: list[str] = []
        for item in config.oracle_template:
            if item == "{test_files}":
                argv.extend(existing)
            elif item == "{packages}":
                argv.extend(packages)
            else:
                argv.append(item)
        return (tuple(argv),)

    commands: list[tuple[str, ...]] = []
    cargo_dirs = _manifest_dirs(files, ("Cargo.toml",))
    names: dict[str, str | None] = {}
    for path in existing:
        if not path.endswith(".rs"):
            continue
        package_dir = _nearest(path, cargo_dirs)
        if package_dir is None:
            continue
        if package_dir not in names:
            names[package_dir] = _package_name(git, sha, package_dir)
        name = names[package_dir]
        if name is None:
            continue
        rel = path[len(package_dir) + 1 :] if package_dir else path
        if rel.startswith("tests/") and rel.count("/") == 1:
            commands.append(("cargo", "test", "-p", name, "--test", rel[len("tests/") : -len(".rs")]))
        elif rel.startswith("src/") and path.endswith("_tests.rs"):
            commands.append(("cargo", "test", "-p", name, "--lib"))
    python_tests = tuple(path for path in existing if path.endswith(".py"))
    if python_tests:
        commands.append(("python", "-m", "pytest", "-q", *python_tests))
    return tuple(dict.fromkeys(commands))


def resolve_oracle(
    repo: Path, sha: str, test_paths: Sequence[str], config: ReplayConfig
) -> tuple[tuple[str, ...], ...]:
    """Commands that run the given test files at ``sha``. Test files absent at ``sha`` are dropped."""
    git = Git(repo)
    commit = git.resolve_commit(sha)
    return _resolve_oracle(git, commit, test_paths, config, _tree_files(git, commit))


def analyze_commit(repo: Path, sha: str, config: ReplayConfig | None = None) -> ReplayCandidate:
    git = Git(repo)
    config = config if config is not None else ReplayConfig.for_repository(repo)
    commit = git.resolve_commit(sha)
    header = git.out("show", "-s", "--format=%P%x00%s%x00%b", commit)
    parent_field, subject, body = header.split("\0", 2)
    parents = parent_field.split()
    parent = parents[0] if parents else None

    changes = _parse_numstat(_changes(git, commit, parent, "--numstat", "-z", "-M"))
    tests = [change for change in changes if _kind(change.path, config) == "test"]
    sources = [change for change in changes if _kind(change.path, config) == "source"]
    src_lines = sum(change.lines for change in sources)
    test_lines = sum(change.lines for change in tests)

    files = _tree_files(git, commit)
    manifests = _manifest_dirs(files, MANIFESTS)
    packages = tuple(sorted({_nearest(change.path, manifests) or "" for change in sources}))
    features = ReplayFeatures(
        src_lines=src_lines,
        test_lines=test_lines,
        src_files=len(sources),
        test_files=len(tests),
        packages=packages,
        band=_band(src_lines),
    )

    excluded: list[str] = []
    if len(parents) > 1:
        excluded.append("merge")
    if parent is None:
        excluded.append("root")
    if not tests:
        excluded.append("no_tests")
    if src_lines == 0:
        excluded.append("no_source")
    if sources and _has_inline_tests(_added_lines(_changes(git, commit, parent, "-M", "-U0")), sources):
        excluded.append("inline_tests")
    if parent is not None and src_lines > 0:
        whitespace_blind = _parse_numstat(_changes(git, commit, parent, "--numstat", "-z", "-M", "-w"))
        if not any(_kind(change.path, config) == "source" for change in whitespace_blind):
            excluded.append("formatting_only")
    if sources and all(change.old_path is not None and change.lines == 0 for change in sources):
        excluded.append("rename_only")
    oracle = _resolve_oracle(git, commit, [change.path for change in tests], config, files)
    if not oracle:
        excluded.append("no_oracle")

    return ReplayCandidate(
        sha=commit,
        parent=parent,
        subject=subject,
        body=body.rstrip("\n"),
        test_paths=tuple(change.path for change in tests),
        source_paths=tuple(change.path for change in sources),
        features=features,
        oracle=oracle,
        excluded=tuple(excluded),
    )


def scan(
    repo: Path,
    *,
    rev: str = "HEAD",
    limit: int | None = None,
    min_lines: int = 1,
    max_lines: int | None = None,
    include_excluded: bool = False,
    config: ReplayConfig | None = None,
) -> list[ReplayCandidate]:
    """Candidates from ``rev`` (newest first) whose source size is in range. Read-only."""
    validate_ref(rev)
    config = config if config is not None else ReplayConfig.for_repository(repo)
    shas = Git(repo).out("rev-list", "--no-merges", rev).split()
    candidates: list[ReplayCandidate] = []
    for sha in shas:
        if limit is not None and len(candidates) >= limit:
            break
        candidate = analyze_commit(repo, sha, config)
        lines = candidate.features.src_lines
        if lines < min_lines or (max_lines is not None and lines > max_lines):
            continue
        if candidate.excluded and not include_excluded:
            continue
        candidates.append(candidate)
    return candidates


def _name_status(output: str) -> Iterator[tuple[str, str, str | None]]:
    """``(status, path, old_path)`` from ``--name-status -z``; renames and copies carry two paths."""
    tokens = output.split("\0")
    i = 0
    while i < len(tokens):
        status = tokens[i]
        i += 1
        if not status:
            continue
        if status[0] in "RC":
            old, new = tokens[i], tokens[i + 1]
            i += 2
            yield status, new, old
        else:
            path = tokens[i]
            i += 1
            yield status, path, None


def _sync_path(git: Git, env: dict[str, str], sha: str, path: str) -> None:
    """Make the index entry for ``path`` equal to its content at ``sha``, or absent if it has none."""
    entry = git.out("ls-tree", "-z", sha, "--", path)
    if not entry:
        git.run("update-index", "--force-remove", path, env=env)
        return
    mode, _object_type, blob = entry.split("\0")[0].split("\t", 1)[0].split()
    git.run("update-index", "--add", "--cacheinfo", mode, blob, path, env=env)


def create_base(repo: Path, candidate: ReplayCandidate) -> str:
    """Create ``wg/replay/<sha[:12]>``: the parent's tree with the commit's test changes applied.

    Works on a temporary index, so the working tree and the repository's own index are untouched.
    """
    if candidate.parent is None:
        raise ValueError(f"{candidate.sha} has no parent, so it has no replay base")
    git = Git(repo)
    config = ReplayConfig.for_repository(repo)
    branch = f"wg/replay/{candidate.sha[:12]}"
    ref = f"refs/heads/{branch}"
    with tempfile.TemporaryDirectory(prefix="wg-replay-index-") as scratch:
        env = {"GIT_INDEX_FILE": str(Path(scratch) / "index")}
        git.run("read-tree", candidate.parent, env=env)
        output = git.out("diff", "--name-status", "-z", "-M", candidate.parent, candidate.sha)
        for status, path, old in _name_status(output):
            if status[0] == "R" and old is not None and _kind(old, config) == "test":
                _sync_path(git, env, candidate.sha, old)
            if _kind(path, config) == "test":
                _sync_path(git, env, candidate.sha, path)
        tree = git.run("write-tree", env=env).stdout.strip()

    existing = git.run("rev-parse", "--verify", "--quiet", f"{ref}^{{tree}}", check=False).stdout.strip()
    if existing:
        if existing == tree:
            return branch
        raise ValueError(f"branch {branch} already exists with a different tree")
    message = f"Replay base for {candidate.sha}\n\nTests from {candidate.sha} on its parent."
    commit = git.run("commit-tree", tree, "-p", candidate.parent, "-m", message).stdout.strip()
    git.run("update-ref", ref, commit)
    return branch


@contextmanager
def _worktree(git: Git, ref: str, path: Path) -> Generator[Path]:
    git.run("worktree", "add", "--detach", str(path), ref)
    try:
        yield path
    finally:
        git.run("worktree", "remove", "--force", str(path), check=False)
        shutil.rmtree(path, ignore_errors=True)


def _exit_code(argv: Sequence[str], cwd: Path, timeout: float) -> int:
    try:
        proc = subprocess.run(list(argv), cwd=cwd, timeout=timeout, capture_output=True, check=False)
    except (subprocess.TimeoutExpired, OSError):
        return -1
    return proc.returncode


def _run_oracle(
    git: Git, ref: str, path: Path, oracle: Sequence[Sequence[str]], timeout: float
) -> tuple[int, ...]:
    with _worktree(git, ref, path) as worktree:
        return tuple(_exit_code(command, worktree, timeout) for command in oracle)


def validate(
    repo: Path,
    candidate: ReplayCandidate,
    base_ref: str,
    *,
    timeout: float = 1800.0,
    workdir: Path | None = None,
) -> Validation:
    """Oracle must fail at the base (some command non-zero) and pass at the real commit (all zero)."""
    if not candidate.oracle:
        return Validation(ok=False, reason="no_oracle", base=(), solution=())
    validate_ref(base_ref)
    git = Git(repo)
    root = Path(tempfile.mkdtemp(prefix="wg-replay-", dir=workdir))
    try:
        base = _run_oracle(git, base_ref, root / "base", candidate.oracle, timeout)
        if all(code == 0 for code in base):
            return Validation(ok=False, reason="oracle_green_at_base", base=base, solution=())
        solution = _run_oracle(git, candidate.sha, root / "solution", candidate.oracle, timeout)
        if any(code != 0 for code in solution):
            return Validation(ok=False, reason="oracle_red_at_solution", base=base, solution=solution)
        return Validation(ok=True, reason=None, base=base, solution=solution)
    finally:
        shutil.rmtree(root, ignore_errors=True)


_STUB_PATTERNS = (
    re.compile(r"\btodo!\s*\("),
    re.compile(r"\bunimplemented!\s*\("),
    re.compile(r"\bNotImplementedError\b"),
    re.compile(r'panic!\s*\(\s*"not implemented'),
)


@dataclass(frozen=True)
class WorktreeChanges:
    """A worktree's contents measured against a base commit, including uncommitted and untracked files."""

    features: ReplayFeatures
    tests_modified: tuple[str, ...]
    added_source_lines: dict[str, tuple[str, ...]]
    stubs: tuple[tuple[str, str], ...]


def _tests_modified(name_status: str, config: ReplayConfig) -> tuple[str, ...]:
    """Existing test files that were modified, deleted, or renamed away (new test files are not listed)."""
    listed: list[str] = []
    for status, path, old in _name_status(name_status):
        if status[0] == "R" and old is not None and _kind(old, config) == "test":
            listed.append(old)
        elif status[0] in "MD" and _kind(path, config) == "test":
            listed.append(path)
    return tuple(listed)


def worktree_changes(worktree: Path, base_sha: str, config: ReplayConfig | None = None) -> WorktreeChanges:
    """Measure the worktree's current contents against ``base_sha``.

    Uses a temporary index, so the worktree's real index and files are never written.
    """
    git = Git(worktree)
    config = config if config is not None else ReplayConfig.for_repository(worktree)
    base = git.resolve_commit(base_sha)
    with tempfile.TemporaryDirectory(prefix="wg-worktree-index-") as scratch:
        env = {"GIT_INDEX_FILE": str(Path(scratch) / "index")}
        git.run("read-tree", base, env=env)
        git.run("add", "-A", env=env)
        files = frozenset(path for path in git.run("ls-files", "-z", env=env).stdout.split("\0") if path)
        numstat = git.run("diff", "--cached", "-M", "--numstat", "-z", base, env=env).stdout
        patch = git.run("diff", "--cached", "-M", "-U0", base, env=env).stdout
        name_status = git.run("diff", "--cached", "-M", "--name-status", "-z", base, env=env).stdout

    changes = _parse_numstat(numstat)
    tests = [change for change in changes if _kind(change.path, config) == "test"]
    sources = [change for change in changes if _kind(change.path, config) == "source"]
    src_lines = sum(change.lines for change in sources)
    manifests = _manifest_dirs(files, MANIFESTS)
    packages = tuple(sorted({_nearest(change.path, manifests) or "" for change in sources}))
    features = ReplayFeatures(
        src_lines=src_lines,
        test_lines=sum(change.lines for change in tests),
        src_files=len(sources),
        test_files=len(tests),
        packages=packages,
        band=_band(src_lines),
    )

    source_paths = {change.path for change in sources}
    added = {path: tuple(lines) for path, lines in _added_lines(patch).items() if path in source_paths}
    stubs = tuple(
        (path, line)
        for path, lines in added.items()
        for line in lines
        if any(pattern.search(line) for pattern in _STUB_PATTERNS)
    )
    return WorktreeChanges(
        features=features,
        tests_modified=_tests_modified(name_status, config),
        added_source_lines=added,
        stubs=stubs,
    )
