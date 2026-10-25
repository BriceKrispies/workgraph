"""Golden vectors GV-03 (``gitparse``).

Covers the git output parsers in ``integrations/git/parse.py``, the worktree parser and remote
URL normaliser in ``integrations/git/cli.py``, and the path classification in
``analytics/code/classify.py``.

Throwaway repositories are created under a temporary directory with fixed identities and dates,
so every commit SHA, and therefore every byte of git output, is reproducible. Run from the
repository root with ``uv run python tools/golden/gitparse.py``. Writes ``golden/gitparse/*.json``.
"""

from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import tempfile
from dataclasses import asdict, is_dataclass
from datetime import datetime
from pathlib import Path
from typing import Any

from workgraph.analytics.code.classify import file_class, language_for, module_for
from workgraph.integrations.git.cli import Git, normalize_remote_url, parse_worktree_porcelain
from workgraph.integrations.git.parse import (
    CHERRY_PICK_RE,
    META_FORMAT,
    parse_diff_tree,
    parse_meta,
    parse_numstat_z,
    parse_patch_ids,
    parse_trailers,
)

ROOT = Path(__file__).resolve().parents[2]
OUT_DIR = ROOT / "golden" / "gitparse"
PLACEHOLDER = "@ROOT@"
EPOCH = 1_700_000_000  # 2023-11-14T22:13:20Z

ADA = ("Ada Lovelace", "ada@example.test")
ZOE = ("Zoë Ångström", "zoe@example.test")
CARL = ("Carl Sagan", "carl@example.test")


def _remove_tree(path: Path) -> None:
    """Delete a temporary tree; git writes read-only object files that Windows will not unlink."""
    if not path.exists():
        return
    for item in [path, *path.rglob("*")]:
        item.chmod(stat.S_IWRITE | stat.S_IREAD)
    shutil.rmtree(path)


def _isolate_environment(home: Path) -> None:
    """Drop caller git settings so only the fixed values below reach git."""
    for key in [k for k in os.environ if k.startswith("GIT_")]:
        del os.environ[key]
    (home / "gitconfig").write_bytes(b"")
    os.environ.update(
        {
            "HOME": str(home),
            "USERPROFILE": str(home),
            "GIT_CONFIG_GLOBAL": str(home / "gitconfig"),
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_TERMINAL_PROMPT": "0",
            "TZ": "UTC",
        }
    )


def git(repo: Path, *args: str, env: dict[str, str] | None = None) -> str:
    proc = subprocess.run(
        ["git", "-C", str(repo), *args],
        capture_output=True,
        env=env if env is not None else dict(os.environ),
        check=False,
    )
    if proc.returncode != 0:
        raise SystemExit(f"git {' '.join(args)} failed: {proc.stderr.decode('utf-8', 'replace')}")
    return proc.stdout.decode("utf-8")


def stamp(epoch: int, offset: str = "+0000") -> str:
    return f"{epoch} {offset}"


def identity_env(
    author: tuple[str, str],
    author_date: str,
    committer: tuple[str, str],
    committer_date: str,
) -> dict[str, str]:
    env = dict(os.environ)
    env.update(
        {
            "GIT_AUTHOR_NAME": author[0],
            "GIT_AUTHOR_EMAIL": author[1],
            "GIT_AUTHOR_DATE": author_date,
            "GIT_COMMITTER_NAME": committer[0],
            "GIT_COMMITTER_EMAIL": committer[1],
            "GIT_COMMITTER_DATE": committer_date,
        }
    )
    return env


def put(repo: Path, rel: str, data: bytes) -> None:
    target = repo / rel
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(data)


def commit(
    repo: Path,
    work: Path,
    message: str,
    *,
    author: tuple[str, str],
    author_date: str,
    committer: tuple[str, str] | None = None,
    committer_date: str | None = None,
) -> str:
    env = identity_env(
        author,
        author_date,
        committer or author,
        committer_date or author_date,
    )
    message_file = work / "message.txt"
    message_file.write_bytes(message.encode("utf-8"))
    git(repo, "add", "-A", env=env)
    git(
        repo,
        "commit",
        "-q",
        "--no-verify",
        "--no-gpg-sign",
        "--allow-empty",
        "-F",
        str(message_file),
        env=env,
    )
    return git(repo, "rev-parse", "HEAD").strip()


def build_history(root: Path) -> dict[str, str]:
    """Create the ``history`` repository and return the SHAs of its named commits."""
    repo = root / "history"
    work = root / "work"
    template = root / "template"
    for directory in (repo, work, template):
        directory.mkdir(parents=True)
    git(repo, "init", "-q", "-b", "main", f"--template={template}")
    git(repo, "config", "core.autocrlf", "false")
    shas: dict[str, str] = {}

    put(repo, "src/app/main.py", b"def main():\n    return 0\n")
    put(repo, "tests/test_main.py", b"def test_main():\n    assert True\n")
    put(repo, "docs/README.md", b"# Demo\n\nFirst line.\n")
    shas["root"] = commit(
        repo,
        work,
        "Initial import\n\nBody of the root commit.\n\nTask-ID: WG-1\nReviewed-by: Grace Hopper <grace@example.test>\n",
        author=ADA,
        author_date=stamp(EPOCH),
    )

    # Non-ASCII author, author and committer in different zones, a space in a file name,
    # a binary file, a rename with edits, a deletion.
    put(repo, "src/app/main.py", b"def main():\n    return 0\n\ndef helper():\n    return 1\n")
    put(repo, "src/a b.py", b"x = 1\n" * 3)
    put(repo, "src/naïve.py", b"print('ok')\n")
    put(repo, "assets/logo.bin", bytes(range(256)))
    (repo / "docs" / "README.md").replace(repo / "docs" / "GUIDE.md")
    put(repo, "docs/GUIDE.md", b"# Demo\n\nFirst line.\nSecond line.\n")
    (repo / "tests" / "test_main.py").unlink()
    shas["refactor"] = commit(
        repo,
        work,
        "Refactor main, add assets\n\nTask-ID: WG-2\n",
        author=ZOE,
        author_date=stamp(EPOCH + 3600, "+0530"),
        committer=CARL,
        committer_date=stamp(EPOCH + 7200, "-0800"),
    )

    git(repo, "switch", "-q", "-c", "feature")
    put(repo, "src/feature.py", b"FEATURE = True\n")
    shas["feature"] = commit(
        repo,
        work,
        "Add feature flag\n\nTask-ID: WG-3\n",
        author=ADA,
        author_date=stamp(EPOCH + 10800),
    )

    git(repo, "switch", "-q", "main")
    data = (repo / "src" / "app" / "main.py").read_bytes()
    put(repo, "src/app/main_copy.py", data)
    put(repo, "src/app/main.py", data + b"\nVERSION = 2\n")
    shas["copy"] = commit(
        repo,
        work,
        "Copy main and tweak the original\n\nTask-ID: WG-5\n",
        author=ADA,
        author_date=stamp(EPOCH + 14400),
    )

    put(repo, "docs/GUIDE.md", b"# Demo\n\nFirst line.\nSecond line.\nThird line.\n")
    shas["guide"] = commit(
        repo,
        work,
        "Fix guide wording\n\nTask-ID: WG-4\n",
        author=CARL,
        author_date=stamp(EPOCH + 18000),
    )

    env = identity_env(ADA, stamp(EPOCH + 21600), ADA, stamp(EPOCH + 21600))
    git(repo, "merge", "--no-ff", "-q", "-m", "Merge feature into main\n\nTask-ID: WG-3", "feature", env=env)
    shas["merge"] = git(repo, "rev-parse", "HEAD").strip()

    git(repo, "switch", "-q", "-c", "hotfix", shas["refactor"])
    env = identity_env(ADA, stamp(EPOCH + 25200), ADA, stamp(EPOCH + 25200))
    git(repo, "cherry-pick", "-x", shas["guide"], env=env)
    shas["cherry"] = git(repo, "rev-parse", "HEAD").strip()

    git(repo, "switch", "-q", "main")
    shas["empty"] = commit(repo, work, "Empty checkpoint", author=ADA, author_date=stamp(EPOCH + 28800))
    shas["unicode"] = commit(
        repo,
        work,
        "Schöne Grüße ✓\n\nReviewed-by: Zoë Ångström <zoe@example.test>\nReviewed-by: Carl Sagan <carl@example.test>\n",
        author=ZOE,
        author_date=stamp(EPOCH + 32400),
    )

    # Worktrees: a branch, a detached head, a locked one and one whose directory was removed.
    wt = root / "wt"
    git(repo, "worktree", "add", "-q", str(wt / "feature"), "feature")
    git(repo, "worktree", "add", "-q", "--detach", str(wt / "detached"), shas["refactor"])
    git(repo, "worktree", "add", "-q", "-b", "locked-branch", str(wt / "locked"), shas["guide"])
    git(repo, "worktree", "lock", "--reason", "rebase in progress", str(wt / "locked"))
    git(repo, "worktree", "add", "-q", "-b", "gone", str(wt / "gone"), shas["refactor"])
    _remove_tree(wt / "gone")

    bare = root / "bare.git"
    bare.mkdir()
    git(bare, "init", "-q", "--bare", "-b", "main", f"--template={template}")
    return shas


def _plain(value: Any) -> Any:
    if is_dataclass(value) and not isinstance(value, type):
        return _plain(asdict(value))
    if isinstance(value, dict):
        return {str(k): _plain(v) for k, v in value.items()}
    if isinstance(value, (list, tuple)):
        return [_plain(v) for v in value]
    if isinstance(value, datetime):
        return value.isoformat()
    if isinstance(value, Path):
        return value.as_posix()
    return value


def _commit_json(meta: Any) -> dict[str, Any]:
    data = _plain(meta)
    data["subject"] = meta.subject
    data["cherry_picked_from"] = meta.cherry_picked_from
    return data


def _anonymise(text: str, root: Path) -> str:
    for form in {str(root), str(root).replace("\\", "/")}:
        text = text.replace(form, PLACEHOLDER)
    return text


def _write(name: str, cases: list[dict[str, Any]]) -> int:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    text = json.dumps({"cases": cases}, sort_keys=True, indent=2, ensure_ascii=False) + "\n"
    (OUT_DIR / f"{name}.json").write_bytes(text.encode("utf-8"))
    return len(cases)


# Synthetic inputs copied from tests/unit/test_foundations.py so the Rust suite can check the same
# cases the Python unit tests check.
SYNTHETIC_DIFF_TREE = (
    f"{'a' * 40}\0:100644 100644 1111111 2222222 R087\0old.py\0new.py\0:000000 100644 0000000 3333333 A\0bin.dat\0"
    "3\t1\t\0old.py\0new.py\0-\t-\tbin.dat\0"
)
SYNTHETIC_META = (
    f"\x1e{'a' * 40}\x1ftree\x1fp1 p2\x1fA\x1fa@x\x1f2026-01-01T00:00:00+00:00\x1fC\x1fc@x\x1f"
    "2026-01-01T00:00:00+00:00\x1fTask-ID: WG-1\x1dReviewed-by: X\x1fSubject\n\nBody\n"
)
SYNTHETIC_WORKTREE_Z = (
    "worktree C:/dev/repo\0HEAD 1111111111111111111111111111111111111111\0branch refs/heads/main\0\0"
    "worktree C:/dev/wt-detached\0HEAD 2222222222222222222222222222222222222222\0detached\0\0"
    "worktree C:/dev/wt-locked\0HEAD 3333333333333333333333333333333333333333\0"
    "branch refs/heads/feature/x\0locked rebase in progress\0\0"
    "worktree C:/dev/wt-gone\0HEAD 4444444444444444444444444444444444444444\0"
    "branch refs/heads/old\0prunable gitdir file points to non-existent location\0\0"
    "worktree C:/dev/bare.git\0bare\0\0"
)
SYNTHETIC_URLS = [
    "git@github.com:Org/Repo.git",
    "https://github.com/Org/Repo",
    "ssh://git@GitHub.com/Org/Repo.git",
    "https://user:pw@github.com/Org/Repo.git/",
]
EXTRA_URLS = [
    "https://github.com:443/Org/Repo.git",
    "ssh://git@host.example:2222/team/proj.git",
    "ssh://git@github.com:22/Org/Repo.git",
    "git+ssh://git@example.com/a/b",
    "git://example.com/a/b.git",
    "http://Example.COM/a/b",
    "HTTPS://GITHUB.COM/ORG/REPO.GIT",
    "https://user@github.com/Org/Repo",
    "git@gitlab.com:group/sub/repo.git",
    "git@github.com:12345/repo.git",
    "git@bitbucket.org:team/repo",
    "https://dev.azure.com/org/project/_git/repo",
    "https://github.com/org/repo.git",
    "file:///C:/repos/demo.git",
    "/srv/git/demo.git",
    "   https://github.com/a/b.git  ",
    "",
]
EXTRA_TRAILER_RAW = [
    "",
    "Task-ID: WG-1\x1dReviewed-by: X",
    "Task-ID:WG-9\x1dNo colon line\x1d   \x1dLink: https://x.example/y:1",
    "Key-2: value with spaces   \x1dEmpty:\x1dok: yes",
]
EXTRA_CHERRY_MESSAGES = [
    "Subject\n\n(cherry picked from commit 0123456789abcdef0123456789abcdef01234567)\n",
    "(cherry picked from commit abcdef1)",
    "(cherry picked from commit abcdef)",
    "(cherry picked from commit ABCDEF1)",
    "(cherry picked from commit abcdef1) and (cherry picked from commit 1234567)",
    "no reference here",
]
PATHS = [
    "src/workgraph/core/x.py",
    "README.md",
    "tests/test_api.py",
    "src/app.test.ts",
    "docs/guide.md",
    "pyproject.toml",
    "src/workgraph/app.py",
    "a.rs",
    "a.unknown",
    "Makefile",
    "Dockerfile",
    ".github/workflows/ci.yml",
    ".env.local",
    "src/__tests__/x.tsx",
    "e2e/login.spec.ts",
    "lib/Foo_test.go",
    "pkg/x_spec.rb",
    "Tests/FooTests.cs",
    "LICENSE",
    "CHANGELOG.md",
    "src/main/java/App.java",
    "cmd/tool/main.go",
    "packages/ui/src/index.tsx",
    "apps/web/page.tsx",
    "internal/x/y.go",
    ".gitignore",
    "uv.lock",
    "data/config.json",
    "docs/x/y.rst",
    "a/b/c/d/e.py",
    "lib/pkg/mod.c",
    "README",
    "src/A.PY",
    "src/naïve.py",
    "src/a b.py",
    "assets/logo.bin",
    "x",
]


def main() -> None:
    home = Path(tempfile.mkdtemp(prefix="wg-gv03-home-"))
    root = Path(tempfile.mkdtemp(prefix="wg-gv03-"))
    try:
        _isolate_environment(home)
        root = root.resolve()
        shas = build_history(root)
        repo = root / "history"
        git_repo = Git(repo)
        order = [
            shas["root"],
            shas["refactor"],
            shas["feature"],
            shas["copy"],
            shas["guide"],
            shas["merge"],
            shas["cherry"],
            shas["empty"],
            shas["unicode"],
        ]
        names = {sha: name for name, sha in shas.items()}

        meta_cases: list[dict[str, Any]] = []
        all_meta_raw = git_repo.run(
            "log",
            "--no-walk=unsorted",
            "--stdin",
            f"--format={META_FORMAT}",
            input_text="\n".join(order) + "\n",
        ).stdout
        for sha in order:
            raw = git_repo.run(
                "log", "--no-walk=unsorted", "--stdin", f"--format={META_FORMAT}", input_text=sha + "\n"
            ).stdout
            meta_cases.append(
                {
                    "name": f"commit:{names[sha]}",
                    "input": {"raw": raw},
                    "output": {"commits": [_commit_json(m) for m in parse_meta(raw)]},
                }
            )
        meta_cases.append(
            {
                "name": "history:all_commits",
                "input": {"raw": all_meta_raw},
                "output": {"commits": [_commit_json(m) for m in parse_meta(all_meta_raw)]},
            }
        )
        meta_cases.append(
            {
                "name": "synthetic:test_foundations.test_parse_diff_tree_and_meta",
                "input": {"raw": SYNTHETIC_META},
                "output": {"commits": [_commit_json(m) for m in parse_meta(SYNTHETIC_META)]},
            }
        )
        meta_cases.append({"name": "synthetic:empty", "input": {"raw": ""}, "output": {"commits": []}})

        metas = parse_meta(all_meta_raw)
        diff_cases: list[dict[str, Any]] = []
        for meta in metas:
            line = f"{meta.sha} {meta.parents[0]}" if meta.parents else meta.sha
            raw = git_repo.run(
                "diff-tree",
                "--stdin",
                "-z",
                "-r",
                "-M",
                "-C",
                "--root",
                "--raw",
                "--numstat",
                input_text=line + "\n",
            ).stdout
            diff_cases.append(
                {
                    "name": f"commit:{names[meta.sha]}",
                    "input": {"raw": raw},
                    "output": {"commits": _plain(parse_diff_tree(raw))},
                }
            )
        diff_cases.append(
            {
                "name": "synthetic:test_foundations.test_parse_diff_tree_and_meta",
                "input": {"raw": SYNTHETIC_DIFF_TREE},
                "output": {"commits": _plain(parse_diff_tree(SYNTHETIC_DIFF_TREE))},
            }
        )

        numstat_cases: list[dict[str, Any]] = []
        pairs = [
            (shas["root"], shas["refactor"]),
            (shas["refactor"], shas["copy"]),
            (shas["refactor"], shas["feature"]),
            (shas["copy"], shas["guide"]),
            (shas["refactor"], shas["cherry"]),
            (shas["guide"], shas["merge"]),
            (shas["root"], shas["unicode"]),
        ]
        for base, final in pairs:
            raw = git_repo.run("diff", "--numstat", "-z", "-M", base, final).stdout
            numstat_cases.append(
                {
                    "name": f"range:{names[base]}..{names[final]}",
                    "input": {"raw": raw},
                    "output": {"files": _plain(parse_numstat_z(raw))},
                }
            )

        patch_lines = "\n".join(
            f"{m.sha} {m.parents[0]}" if m.parents else m.sha for m in metas if len(m.parents) <= 1
        )
        patches = git_repo.run(
            "diff-tree", "--stdin", "-p", "-r", "--root", input_text=patch_lines + "\n"
        ).stdout
        patch_raw = git_repo.run("patch-id", "--stable", input_text=patches).stdout
        patch_cases = [
            {
                "name": "history:single_parent_commits",
                "input": {"raw": patch_raw},
                "output": {"ids": parse_patch_ids(patch_raw)},
            }
        ]

        head_worktrees = git_repo.run("worktree", "list", "--porcelain", "-z").stdout
        head_plain = git_repo.run("worktree", "list", "--porcelain").stdout
        bare_z = Git(root / "bare.git").run("worktree", "list", "--porcelain", "-z").stdout
        worktree_cases: list[dict[str, Any]] = []
        for name, raw, nul in (
            ("history:z", head_worktrees, True),
            ("history:plain", head_plain, False),
            ("bare:z", bare_z, True),
        ):
            anonymised = _anonymise(raw, root)
            if "wg-gv03-" in anonymised:
                raise SystemExit(f"temporary path leaked into worktree vector {name}")
            worktree_cases.append(
                {
                    "name": name,
                    "input": {"output": anonymised, "nul": nul},
                    "output": {"entries": _plain(parse_worktree_porcelain(anonymised, nul=nul))},
                }
            )
        worktree_cases.append(
            {
                "name": "synthetic:test_foundations.test_parse_worktree_porcelain_entries_in_both_framings",
                "input": {"output": SYNTHETIC_WORKTREE_Z, "nul": True},
                "output": {"entries": _plain(parse_worktree_porcelain(SYNTHETIC_WORKTREE_Z, nul=True))},
            }
        )
        worktree_cases.append(
            {
                "name": "synthetic:empty",
                "input": {"output": "", "nul": True},
                "output": {"entries": []},
            }
        )

        trailer_cases: list[dict[str, Any]] = [
            {
                "name": f"raw:{index}",
                "input": {"raw": raw},
                "output": {"trailers": _plain(parse_trailers(raw))},
            }
            for index, raw in enumerate(EXTRA_TRAILER_RAW)
        ]

        cherry_cases: list[dict[str, Any]] = []
        for index, message in enumerate(EXTRA_CHERRY_MESSAGES):
            match = CHERRY_PICK_RE.search(message)
            cherry_cases.append(
                {
                    "name": f"message:{index}",
                    "input": {"message": message},
                    "output": {"sha": match.group(1) if match else None},
                }
            )
        for meta in metas:
            match = CHERRY_PICK_RE.search(meta.message)
            cherry_cases.append(
                {
                    "name": f"commit:{names[meta.sha]}",
                    "input": {"message": meta.message},
                    "output": {"sha": match.group(1) if match else None},
                }
            )

        url_cases = [
            {
                "name": f"url:{index}",
                "input": {"url": url},
                "output": {"normalized": normalize_remote_url(url)},
            }
            for index, url in enumerate(SYNTHETIC_URLS + EXTRA_URLS)
        ]

        paths = sorted(
            set(PATHS) | set(git_repo.run("ls-tree", "-r", "--name-only", "main").stdout.splitlines())
        )
        path_cases = []
        for path in paths:
            path_cases.append(
                {
                    "name": path,
                    "input": {"path": path},
                    "output": {
                        "language": language_for(path),
                        "class": file_class(path),
                        "module": {str(depth): module_for(path, depth) for depth in (1, 2, 3)},
                    },
                }
            )

        counts = {
            "commit_meta": _write("commit_meta", meta_cases),
            "diff_tree": _write("diff_tree", diff_cases),
            "numstat_z": _write("numstat_z", numstat_cases),
            "patch_id": _write("patch_id", patch_cases),
            "worktree_porcelain": _write("worktree_porcelain", worktree_cases),
            "trailers": _write("trailers", trailer_cases),
            "cherry_pick": _write("cherry_pick", cherry_cases),
            "remote_urls": _write("remote_urls", url_cases),
            "file_classes": _write("file_classes", path_cases),
        }
        for name, count in counts.items():
            print(f"{name}: {count} cases")
    finally:
        _remove_tree(root)
        _remove_tree(home)


if __name__ == "__main__":
    main()
