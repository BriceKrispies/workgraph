"""Safe Git CLI execution.

* Always an argument vector, never a shell string.
* ``-C <path>`` instead of changing the process working directory.
* Prompts, pagers, external diff drivers and locale-dependent output are disabled.
* Read-only by default: helpers that write (worktrees, commits) are explicit.
"""

from __future__ import annotations

import functools
import os
import re
import shutil
import subprocess
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path

from workgraph.core.errors import GitError

_SHA_RE = re.compile(r"^[0-9a-f]{7,64}$")
_REF_RE = re.compile(r"^(?!-)(?!.*\.\.)(?!.*//)[A-Za-z0-9._/@{}^~+-]{1,255}$")


def git_executable() -> str:
    exe = shutil.which("git")
    if exe is None:
        raise GitError("git executable not found on PATH")
    return exe


def validate_ref(ref: str) -> str:
    """Reject refs that could be interpreted as options or contain odd characters."""
    if not _REF_RE.match(ref):
        raise GitError(f"invalid git ref {ref!r}")
    return ref


def is_sha(value: str) -> bool:
    return bool(_SHA_RE.match(value))


@functools.cache
def git_version() -> tuple[int, int]:
    """Major and minor version of the installed Git; ``(0, 0)`` when it cannot be read."""
    output = subprocess.run(
        [git_executable(), "--version"], capture_output=True, text=True, check=False
    ).stdout
    match = re.search(r"git version (\d+)\.(\d+)", output)
    return (int(match[1]), int(match[2])) if match else (0, 0)


@dataclass(frozen=True)
class WorktreeEntry:
    """One record of ``git worktree list --porcelain``. The first record is the main working tree."""

    path: Path
    head: str | None = None
    branch: str | None = None
    detached: bool = False
    bare: bool = False
    locked: bool = False
    prunable: bool = False


def parse_worktree_porcelain(output: str, *, nul: bool) -> list[WorktreeEntry]:
    """Parse porcelain output. ``nul`` selects ``-z`` framing (NUL terminators, NUL-separated records)."""
    separator = "\0" if nul else "\n"
    records: list[dict[str, str]] = []
    current: dict[str, str] | None = None
    for line in output.split(separator):
        if not line:
            current = None  # a blank line (or empty NUL field) closes the record
            continue
        key, _, value = line.partition(" ")
        if key == "worktree":
            current = {"worktree": value}
            records.append(current)
        elif current is not None:
            current[key] = value
    return [
        WorktreeEntry(
            path=Path(record["worktree"]),
            head=record.get("HEAD") or None,
            branch=record["branch"].removeprefix("refs/heads/") if "branch" in record else None,
            detached="detached" in record,
            bare="bare" in record,
            locked="locked" in record,
            prunable="prunable" in record,
        )
        for record in records
    ]


@dataclass
class WorkingTreeStatus:
    branch: str | None = None
    head: str | None = None
    ahead: int | None = None
    behind: int | None = None
    staged: list[str] = field(default_factory=list)
    unstaged: list[str] = field(default_factory=list)
    untracked: list[str] = field(default_factory=list)


@dataclass(frozen=True)
class GitResult:
    args: list[str]
    returncode: int
    stdout: str
    stderr: str


class Git:
    def __init__(self, path: Path | str, *, timeout: float = 120.0) -> None:
        self.path = Path(path)
        self.timeout = timeout

    def _env(self, extra: Mapping[str, str] | None = None) -> dict[str, str]:
        env = dict(os.environ)
        env.update(
            {
                "GIT_TERMINAL_PROMPT": "0",
                "GIT_PAGER": "cat",
                "PAGER": "cat",
                "LC_ALL": "C",
                "LANG": "C",
                "GIT_OPTIONAL_LOCKS": "0",
            }
        )
        env.pop("GIT_DIR", None)
        env.pop("GIT_WORK_TREE", None)
        env.update(extra or {})
        return env

    def run(
        self,
        *args: str,
        check: bool = True,
        input_text: str | None = None,
        timeout: float | None = None,
        binary: bool = False,
        env: Mapping[str, str] | None = None,
    ) -> GitResult:
        argv = [
            git_executable(),
            "-C",
            str(self.path),
            "-c",
            "core.quotepath=off",
            "-c",
            "color.ui=never",
            *args,
        ]
        try:
            proc = subprocess.run(
                argv,
                input=input_text.encode("utf-8") if input_text is not None else None,
                capture_output=True,
                timeout=timeout or self.timeout,
                env=self._env(env),
                check=False,
            )
        except subprocess.TimeoutExpired as exc:
            raise GitError(f"git {' '.join(args[:3])} timed out after {exc.timeout}s") from exc
        except OSError as exc:
            raise GitError(f"failed to execute git: {exc}") from exc
        stdout = proc.stdout.decode("latin-1" if binary else "utf-8", errors="replace")
        stderr = proc.stderr.decode("utf-8", errors="replace")
        result = GitResult(args=list(args), returncode=proc.returncode, stdout=stdout, stderr=stderr)
        if check and proc.returncode != 0:
            raise GitError(
                f"git {' '.join(args[:4])} failed ({proc.returncode}): {stderr.strip()[:500]}",
                details={"args": list(args), "returncode": proc.returncode, "stderr": stderr[:2000]},
            )
        return result

    def out(self, *args: str, check: bool = True, timeout: float | None = None) -> str:
        return self.run(*args, check=check, timeout=timeout).stdout

    # ------------------------------------------------------------------ inspection

    def is_repository(self) -> bool:
        if not self.path.exists():
            return False
        result = self.run("rev-parse", "--is-inside-work-tree", check=False)
        if result.returncode == 0:
            return True
        bare = self.run("rev-parse", "--is-bare-repository", check=False)
        return bare.returncode == 0 and bare.stdout.strip() == "true"

    def toplevel(self) -> Path:
        return Path(self.out("rev-parse", "--show-toplevel").strip()).resolve()

    def git_common_dir(self) -> Path:
        value = self.out("rev-parse", "--git-common-dir").strip()
        candidate = Path(value)
        if not candidate.is_absolute():
            candidate = (self.path / candidate).resolve()
        return candidate

    def is_worktree(self) -> bool:
        git_dir = Path(self.out("rev-parse", "--absolute-git-dir").strip())
        return git_dir.resolve() != self.git_common_dir().resolve()

    def worktrees(self) -> list[WorktreeEntry]:
        """Every worktree of this repository, the main working tree first."""
        if git_version() >= (2, 36):
            return parse_worktree_porcelain(self.out("worktree", "list", "--porcelain", "-z"), nul=True)
        return parse_worktree_porcelain(self.out("worktree", "list", "--porcelain"), nul=False)

    def head(self) -> str | None:
        result = self.run("rev-parse", "--verify", "--quiet", "HEAD", check=False)
        return result.stdout.strip() or None

    def current_branch(self) -> str | None:
        result = self.run("symbolic-ref", "--quiet", "--short", "HEAD", check=False)
        return result.stdout.strip() or None

    def root_commits(self) -> list[str]:
        result = self.run("rev-list", "--max-parents=0", "--all", check=False)
        if result.returncode != 0:
            return []
        return sorted({line.strip() for line in result.stdout.splitlines() if line.strip()})

    def remotes(self) -> dict[str, str]:
        result = self.run("remote", "-v", check=False)
        remotes: dict[str, str] = {}
        for line in result.stdout.splitlines():
            parts = line.split()
            if len(parts) >= 2 and (len(parts) < 3 or parts[2] == "(fetch)"):
                remotes.setdefault(parts[0], parts[1])
        return remotes

    def default_branch(self) -> str | None:
        for remote in self.remotes():
            result = self.run("symbolic-ref", "--quiet", f"refs/remotes/{remote}/HEAD", check=False)
            if result.returncode == 0 and result.stdout.strip():
                return result.stdout.strip().split("/", 3)[-1]
        configured = self.run("config", "--get", "init.defaultBranch", check=False).stdout.strip()
        branches = self.local_branches()
        for candidate in (configured, "main", "master", "trunk", "develop"):
            if candidate and candidate in branches:
                return candidate
        return self.current_branch() or (branches[0] if branches else None)

    def local_branches(self) -> list[str]:
        result = self.run("for-each-ref", "--format=%(refname:short)", "refs/heads", check=False)
        return [b for b in result.stdout.splitlines() if b]

    def refs(self) -> dict[str, str]:
        """All branch and tag refs -> commit SHA (tags peeled)."""
        result = self.run(
            "for-each-ref",
            "--format=%(refname)%00%(objectname)%00%(*objectname)",
            "refs/heads",
            "refs/tags",
            "refs/remotes",
            check=False,
        )
        refs: dict[str, str] = {}
        for line in result.stdout.splitlines():
            parts = line.split("\x00")
            if len(parts) == 3 and not parts[0].endswith("/HEAD"):
                refs[parts[0]] = parts[2] or parts[1]
        return refs

    def is_shallow(self) -> bool:
        return self.out("rev-parse", "--is-shallow-repository", check=False).strip() == "true"

    def object_exists(self, sha: str) -> bool:
        if not is_sha(sha):
            return False
        return self.run("cat-file", "-e", f"{sha}^{{commit}}", check=False).returncode == 0

    def resolve_commit(self, ref: str) -> str:
        validate_ref(ref)
        return self.out("rev-parse", "--verify", f"{ref}^{{commit}}").strip()

    def is_ancestor(self, ancestor: str, descendant: str) -> bool:
        return self.run("merge-base", "--is-ancestor", ancestor, descendant, check=False).returncode == 0

    def show_file(self, sha: str, path: str) -> bytes | None:
        argv = [git_executable(), "-C", str(self.path), "show", f"{sha}:{path}"]
        try:
            proc = subprocess.run(
                argv, capture_output=True, timeout=self.timeout, env=self._env(), check=False
            )
        except (subprocess.TimeoutExpired, OSError):
            return None
        return proc.stdout if proc.returncode == 0 else None

    # ------------------------------------------------------------------ status

    def status_porcelain(self) -> WorkingTreeStatus:
        result = self.run("status", "--porcelain=v2", "--branch", "-z", "--untracked-files=normal")
        status = WorkingTreeStatus()
        entries = result.stdout.split("\0")
        i = 0
        while i < len(entries):
            entry = entries[i]
            i += 1
            if not entry:
                continue
            if entry.startswith("# branch.oid "):
                oid = entry.split(" ", 2)[2]
                status.head = None if oid == "(initial)" else oid
            elif entry.startswith("# branch.head "):
                head = entry.split(" ", 2)[2]
                status.branch = None if head == "(detached)" else head
            elif entry.startswith("# branch.ab "):
                ahead, behind = entry.split(" ")[2:4]
                status.ahead = int(ahead.lstrip("+"))
                status.behind = int(behind.lstrip("-"))
            elif entry.startswith(("1 ", "u ", "2 ")):
                renamed = entry.startswith("2 ")
                fields = entry.split(" ", 9 if renamed else 8)
                xy, path = fields[1], fields[-1]
                if renamed:
                    i += 1  # the original path follows as its own NUL-terminated entry
                if xy[0] != ".":
                    status.staged.append(path)
                if xy[1] != ".":
                    status.unstaged.append(path)
            elif entry.startswith("? "):
                status.untracked.append(entry[2:])
        return status

    def is_clean(self) -> bool:
        status = self.status_porcelain()
        return not (status.staged or status.unstaged or status.untracked)


def normalize_remote_url(url: str) -> str:
    """Canonical form for comparing remotes: host/path, lowercase host, no scheme/creds/.git."""
    value = url.strip()
    value = re.sub(r"^[a-z+]+://", "", value)
    value = re.sub(r"^[^@/]+@", "", value)  # strip credentials / ssh user
    if ":" in value and "/" in value and value.index(":") < value.index("/"):
        host, _, path = value.partition(":")
        if not path.isdigit():
            value = f"{host}/{path}"
    value = re.sub(r":\d+/", "/", value)
    value = value.rstrip("/")
    if value.endswith(".git"):
        value = value[:-4]
    host, _, rest = value.partition("/")
    return f"{host.lower()}/{rest}" if rest else host.lower()
