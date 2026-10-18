"""Trial runner: start a planned trial in a fresh clone, then grade that clone mechanically.

A trial's clone holds only the replay base branch, with no remote, so the solution commit is not reachable by
any ref in it. Grading measures the clone's working tree against the base, runs the oracle and the protocol's
gates there, and records what was measured. Nothing is judged by a model.
"""

from __future__ import annotations

import os
import shlex
import shutil
import subprocess
import time
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from sqlalchemy import String, cast, select

from workgraph.core.context import RequestContext
from workgraph.core.errors import ConflictError, ValidationFailed
from workgraph.core.models.enums import TrialStatus
from workgraph.core.models.schemas import TaskOut, TrialOut
from workgraph.core.services.base import Service
from workgraph.core.services.repositories import primary_checkout
from workgraph.experiments import briefs, replay
from workgraph.experiments.briefs import BriefContext
from workgraph.experiments.contract import contract_problems
from workgraph.experiments.escape import Escape, find_escapes, normalize_path_text
from workgraph.integrations.claude_code.transcript import tool_calls
from workgraph.integrations.git.cli import Git
from workgraph.persistence.orm import Repository, SubagentRun, TaskRepository
from workgraph.security.permissions import Permission, require

BRANCH_PREFIX = "wg/trial/"
OUTPUT_TAIL = 2000
Commands = tuple[tuple[str, ...], ...]


class TrialRunner(Service):
    def start(self, ctx: RequestContext, workspace: str | None, trial_ref: str) -> dict[str, Any]:
        """Clone the replay base into a fresh trial directory, run the protocol's setup, and render the brief."""
        require(ctx, Permission.EXPERIMENT_WRITE)
        trial = self.app.experiments.trial(ctx, workspace, trial_ref)
        if trial.status != TrialStatus.PLANNED:
            raise ConflictError(
                f"trial {trial.id} is {trial.status}; only a planned trial can start",
                details={"trial": trial.id, "status": trial.status},
            )
        protocol = self.app.experiments.get(ctx, workspace, trial.experiment_id).protocol
        task = self.app.tasks.get(ctx, workspace, trial.task_key)
        spec = _task_spec(task)
        repository, checkout = self._repository(trial.task_id)
        if spec.commit is None:
            subject, body = task.title, task.description
        else:
            subject, body = _commit_message(checkout, spec.commit)
        base_ref = spec.base_ref or _default_branch(checkout)

        short = trial.id[:8]
        branch = f"{BRANCH_PREFIX}{short}"
        dest = self.app.paths.worktrees_dir / "trials" / short
        if dest.exists():
            raise ConflictError(f"trial directory {dest} already exists", details={"path": str(dest)})
        env = {**os.environ, **protocol.get("env", {})}
        dest.parent.mkdir(parents=True, exist_ok=True)
        try:
            # --single-branch with --branch <base> makes the clone hold only the base; the remote is then removed
            # so no other branch or tag of the source repository is reachable by ref from the clone. "isolated" (the
            # default) copies only the base branch's objects; "shared" keeps alternates to the source object store, so
            # solution objects stay readable by SHA, and the base branch's name carries a prefix of that SHA.
            Git(checkout).out(
                "clone",
                "--no-local" if protocol.get("clone", "isolated") == "isolated" else "--shared",
                "--single-branch",
                "--no-tags",
                "--branch",
                base_ref,
                str(checkout),
                str(dest),
            )
            clone = Git(dest)
            clone.out("checkout", "-b", branch)
            clone.out("remote", "remove", "origin")
            for command in protocol.get("setup", []):
                argv = [part.replace("{worktree}", str(dest)) for part in command]
                record = _run_command(argv, dest, None, env)
                if record["exit"] != 0:
                    raise ValidationFailed(
                        f"setup command {shlex.join(argv)} exited {record['exit']}: {record['output_tail']}",
                        details={"command": argv, "exit": record["exit"]},
                    )
            base_sha = clone.out("rev-parse", "HEAD").strip()
            if spec.commit is None:
                test_paths = _declared_test_paths(spec.files, replay.ReplayConfig.for_repository(dest))
            else:
                test_paths = spec.test_paths
            oracle, gates = _commands(protocol, spec, dest, base_sha, test_paths)
            context = BriefContext(
                task_key=task.key,
                trial_id=trial.id,
                repository=repository,
                worktree=str(dest),
                branch=branch,
                subject=subject,
                body=body,
                test_paths=tuple(test_paths),
                oracle=oracle,
                gates=gates,
                acceptance=spec.acceptance,
                files=spec.files,
                out_of_scope=spec.out_of_scope,
            )
            brief = briefs.render(str(protocol["brief_template"]), context)
            started = self.app.experiments.start_trial(
                ctx, workspace, trial.id, base_ref=base_sha, worktree_path=str(dest)
            )
        except Exception:
            shutil.rmtree(dest, ignore_errors=True)
            raise
        return {
            "trial": started,
            "worktree": str(dest),
            "branch": branch,
            "base_sha": base_sha,
            "brief": brief,
        }

    def grade(
        self,
        ctx: RequestContext,
        workspace: str | None,
        trial_ref: str,
        *,
        timeout: float = 3600.0,
    ) -> TrialOut:
        """Measure a started trial's clone and record the outcome; a flagged failure is classed as gaming."""
        require(ctx, Permission.EXPERIMENT_WRITE)
        trial = self.app.experiments.trial(ctx, workspace, trial_ref)
        if trial.status not in (TrialStatus.RUNNING, TrialStatus.INVALID):
            raise ConflictError(
                f"trial {trial.id} is {trial.status}; only a running or invalid trial is graded",
                details={"trial": trial.id, "status": trial.status},
            )
        if trial.worktree_path is None or trial.base_ref is None:
            raise ValidationFailed(f"trial {trial.id} has no worktree; start it before grading it")
        worktree = Path(trial.worktree_path)
        if not worktree.is_dir():
            raise ValidationFailed(f"trial worktree {worktree} is missing", details={"path": str(worktree)})
        base_sha = trial.base_ref
        protocol = self.app.experiments.get(ctx, workspace, trial.experiment_id).protocol
        spec = _task_spec(self.app.tasks.get(ctx, workspace, trial.task_key))
        oracle, gates = _commands(protocol, spec, worktree, base_sha, spec.test_paths)

        if trial.subagent_run_id is None:
            run = self._latest_run(trial.id)
            if run is not None:
                run_id, attempt_id, model = run
                trial = self.app.experiments.link_run(
                    ctx,
                    workspace,
                    trial.id,
                    attempt_id=attempt_id,
                    subagent_run_id=run_id,
                    actual_model=model,
                )

        changes = replay.worktree_changes(worktree, base_sha)
        flags: dict[str, Any] = {}
        if changes.tests_modified:
            flags["tests_modified"] = list(changes.tests_modified)
        if changes.stubs:
            flags["stubs"] = [{"path": path, "line": line} for path, line in changes.stubs]
        if spec.leaf:
            undeclared = [path for path in changes.changed_paths if path not in spec.files]
            if undeclared:
                flags["outside_declared_files"] = undeclared
        escape_check, escapes = self._escapes(trial, worktree)
        if escapes:
            flags["escaped_clone"] = [
                {"tool": e.tool, "tool_use_id": e.tool_use_id, "root": e.root, "excerpt": e.excerpt}
                for e in escapes
            ]

        env = {**os.environ, **protocol.get("env", {})}
        oracle_records = [_run_command(argv, worktree, timeout, env) for argv in oracle]
        gate_records = [_run_command(argv, worktree, timeout, env) for argv in gates]
        head_sha = Git(worktree).out("rev-parse", "HEAD").strip()
        passed = all(record["exit"] == 0 for record in oracle_records + gate_records) and not flags
        run_detail = (
            {
                "subagent_run_id": trial.subagent_run_id,
                "attempt_id": trial.attempt_id,
                "model": trial.actual_model,
            }
            if trial.subagent_run_id is not None
            else None
        )
        graded = self.app.experiments.record_grade(
            ctx,
            workspace,
            trial.id,
            "pass" if passed else "fail",
            outcome_detail={
                "base_sha": base_sha,
                "head_sha": head_sha,
                "run": run_detail,
                "oracle": oracle_records,
                "gates": gate_records,
                "flags": flags,
                "escape_check": escape_check,
            },
            # record_grade stores this under features["actual"], so the measured dict is passed bare.
            features=changes.features.as_dict(),
        )
        if not passed and flags and graded.status != TrialStatus.INVALID:
            self.app.experiments.classify(
                ctx, workspace, trial.id, "gaming", "mechanical", notes=_flag_summary(flags)
            )
        return self.app.experiments.trial(ctx, workspace, trial.id)

    def _repository(self, task_id: str) -> tuple[str, Path]:
        """The name and primary checkout of the task's first linked repository."""
        with self.db.session() as session:
            repo_id = session.scalars(
                select(TaskRepository.repository_id)
                .where(TaskRepository.task_id == task_id)
                .order_by(TaskRepository.created_at)
            ).first()
            if repo_id is None:
                raise ValidationFailed("the trial's task has no linked repository")
            repo = session.get(Repository, repo_id)
            assert repo is not None
            checkout = primary_checkout(repo)
            if checkout is None:
                raise ValidationFailed(f"repository {repo.name!r} has no available checkout")
            return repo.name, Path(checkout.path)

    def _escapes(self, trial: TrialOut, worktree: Path) -> tuple[str, list[Escape]]:
        """Tool calls in the linked run's transcript that name a path outside the clone: (check status, escapes)."""
        if trial.subagent_run_id is None:
            return "no_run", []
        with self.db.session() as session:
            run = session.get(SubagentRun, trial.subagent_run_id)
            if run is None:
                return "no_run", []
            transcript = run.transcript_path
        if transcript is None or not Path(transcript).is_file():
            return "no_transcript", []
        roots = self._source_roots(trial.task_id, worktree)
        return "checked", find_escapes(tool_calls(transcript), roots)

    def _source_roots(self, task_id: str, clone: Path) -> list[str]:
        """Checkout paths of the task's repository that a clone must not read: every non-managed checkout, primary
        first, excluding the clone itself and any directory that contains it."""
        with self.db.session() as session:
            repo_id = session.scalars(
                select(TaskRepository.repository_id)
                .where(TaskRepository.task_id == task_id)
                .order_by(TaskRepository.created_at)
            ).first()
            if repo_id is None:
                return []
            repo = session.get(Repository, repo_id)
            assert repo is not None
            checkouts = sorted(
                (c for c in repo.checkouts if c.kind != "managed_worktree"),
                key=lambda c: (not c.is_primary, c.created_at),
            )
            paths = [c.path for c in checkouts]
        clone_text = normalize_path_text(str(clone)).rstrip("/")
        roots: list[str] = []
        for path in paths:
            root_text = normalize_path_text(path).rstrip("/")
            if clone_text == root_text or clone_text.startswith(root_text + "/"):
                continue
            roots.append(path)
        return roots

    def _latest_run(self, trial_id: str) -> tuple[str, str | None, str | None] | None:
        """The most recent subagent run whose telemetry names this trial: (run id, attempt id, model)."""
        with self.db.session() as session:
            rows = session.scalars(
                select(SubagentRun)
                .where(cast(SubagentRun.telemetry, String).like(f"%{trial_id}%"))
                .order_by(SubagentRun.stopped_at.desc().nulls_last())
            )
            for run in rows:
                telemetry = run.telemetry
                trial_ids = telemetry.get("trial_ids", []) if isinstance(telemetry, dict) else []
                if trial_id in trial_ids:
                    return run.id, run.attempt_id, run.model
        return None


def _replay_meta(metadata: Mapping[str, Any], key: str) -> dict[str, Any]:
    meta = metadata.get("replay")
    if not isinstance(meta, dict) or not isinstance(meta.get("sha"), str):
        raise ValidationFailed(f"task {key} is not a replay task")
    return meta


def _commit_message(checkout: Path, sha: str) -> tuple[str, str]:
    git = Git(checkout)
    subject = git.out("log", "-1", "--format=%s", sha).strip()
    body = git.out("log", "-1", "--format=%b", sha).rstrip("\n")
    return subject, body


@dataclass(frozen=True)
class _Spec:
    """What a task's trial is run and graded against: a replay commit, or a leaf's declared contract.

    ``commit`` is set only for replay tasks. ``oracle`` is the task's own oracle; the protocol's oracle template
    applies to replay tasks only. ``gates`` are the task's own gates, run before the protocol's gates.
    """

    leaf: bool
    base_ref: str | None
    commit: str | None
    test_paths: tuple[str, ...]
    oracle: Commands
    gates: Commands
    files: tuple[str, ...]
    out_of_scope: tuple[str, ...]
    acceptance: tuple[tuple[str, str], ...]


def _task_spec(task: TaskOut) -> _Spec:
    """The spec of a replay task, or of a leaf whose ``metadata.contract`` is complete. Anything else is refused."""
    if isinstance(task.metadata.get("replay"), dict):
        meta = _replay_meta(task.metadata, task.key)
        return _Spec(
            leaf=False,
            base_ref=str(meta["base_ref"]),
            commit=str(meta["sha"]),
            test_paths=tuple(str(path) for path in meta["test_paths"]),
            oracle=_argv_tuples(meta.get("oracle", [])),
            gates=(),
            files=(),
            out_of_scope=(),
            acceptance=(),
        )
    missing, not_delegable = contract_problems(task)
    if missing or not_delegable:
        problems = [*not_delegable, *(f"missing {name}" for name in missing)]
        raise ValidationFailed(
            f"task {task.key} has no replay metadata and cannot run as a leaf: {'; '.join(problems)}",
            details={"task": task.key, "missing": missing, "not_delegable": not_delegable},
        )
    contract: dict[str, Any] = task.metadata["contract"]
    base = contract.get("base_ref")
    return _Spec(
        leaf=True,
        base_ref=str(base) if base is not None else None,
        commit=None,
        test_paths=(),
        oracle=_argv_tuples(contract["oracle"]),
        gates=_argv_tuples(contract["gates"]),
        files=tuple(str(path) for path in contract["files"]),
        out_of_scope=tuple(str(item) for item in contract["out_of_scope"]),
        acceptance=tuple(zip(task.acceptance_criteria, contract["checks"], strict=True)),
    )


def _argv_tuples(value: Any) -> Commands:
    return tuple(tuple(str(part) for part in command) for command in value)


def _declared_test_paths(files: Sequence[str], config: replay.ReplayConfig) -> tuple[str, ...]:
    return tuple(path for path in files if any(replay.match_glob(path, glob) for glob in config.test_globs))


def _default_branch(checkout: Path) -> str:
    """The repository's default branch, or the checked-out branch when no default can be read."""
    git = Git(checkout)
    branch = git.default_branch() or git.current_branch()
    if branch is None:
        raise ValidationFailed(
            f"checkout {checkout} has no default branch; set contract.base_ref on the task",
            details={"path": str(checkout)},
        )
    return branch


def _commands(
    protocol: Mapping[str, Any],
    spec: _Spec,
    clone: Path,
    base_sha: str,
    test_paths: Sequence[str],
) -> tuple[Commands, Commands]:
    """The oracle and gate commands of a trial, shared by start and grade so they cannot disagree.

    The oracle must exist, or the trial cannot be run or graded.
    """
    template = protocol.get("oracle_template")
    if template is not None and not spec.leaf:
        existing, packages = replay.template_inputs(clone, base_sha, test_paths)
        oracle: Commands = (replay.expand_oracle_template(template, existing, packages),) if existing else ()
    else:
        oracle = spec.oracle
    if not oracle:
        raise ValidationFailed("the trial has no oracle command, so it cannot be run or graded")
    gates = spec.gates + _argv_tuples(protocol.get("gates", []))
    return oracle, gates


def _text(value: bytes | str | None) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return value


def _run_command(
    argv: Sequence[str], cwd: Path, timeout: float | None, env: Mapping[str, str]
) -> dict[str, Any]:
    """Run one command; a timeout or a command that cannot start records exit -1."""
    started = time.monotonic()
    try:
        proc = subprocess.run(
            list(argv), cwd=cwd, env=dict(env), capture_output=True, timeout=timeout, check=False
        )
        code = proc.returncode
        output = _text(proc.stdout) + _text(proc.stderr)
    except subprocess.TimeoutExpired as exc:
        code = -1
        output = _text(exc.stdout) + _text(exc.stderr)
    except OSError as exc:
        code = -1
        output = str(exc)
    return {
        "argv": list(argv),
        "exit": code,
        "duration_ms": int((time.monotonic() - started) * 1000),
        "output_tail": output[-OUTPUT_TAIL:],
    }


def _flag_summary(flags: Mapping[str, Any]) -> str:
    parts: list[str] = []
    if "tests_modified" in flags:
        parts.append("tests modified: " + ", ".join(flags["tests_modified"]))
    if "stubs" in flags:
        parts.append("stubs in: " + ", ".join(sorted({stub["path"] for stub in flags["stubs"]})))
    if "escaped_clone" in flags:
        parts.append(
            "escaped clone via: " + ", ".join(sorted({escape["root"] for escape in flags["escaped_clone"]}))
        )
    if "outside_declared_files" in flags:
        parts.append("outside declared files: " + ", ".join(flags["outside_declared_files"]))
    return "; ".join(parts)
