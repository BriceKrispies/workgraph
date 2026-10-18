"""Brief templates for agent trials.

A template is immutable once it has been used to render a brief: changing its text means adding a
new template id, so that every recorded trial can be traced to the exact wording it was given.
"""

from __future__ import annotations

import shlex
from collections.abc import Callable
from dataclasses import dataclass

from workgraph.core.errors import ValidationFailed


@dataclass(frozen=True)
class BriefContext:
    """Everything a brief template may print. Commands are argv tuples, rendered with ``shlex.join``."""

    task_key: str
    trial_id: str
    repository: str
    worktree: str
    branch: str
    subject: str
    body: str
    test_paths: tuple[str, ...]
    oracle: tuple[tuple[str, ...], ...]
    gates: tuple[tuple[str, ...], ...]


_REPLAY_V1 = """\
Task-ID: {task_key}
Trial-ID: {trial_id}

You are working alone in the git worktree {worktree} (repository {repository}), \
on branch {branch}. Work only there. Do not push or merge.

## Task
{subject}

{body_block}## Tests that must pass
These test files were added or changed for this task and fail now:
{tests}

Make them pass by changing the code, not the tests. Do not modify, delete or skip these test files.

Run them with:
{oracle}

## Checks that must also pass
{gates}

## Rules
Follow the repository's own agent instructions (AGENTS.md or CLAUDE.md) if it has them. Commit your work on {branch} \
when you are done.

## Report
Reply with what you changed, the output of the test command, and anything you could not finish.
"""


def _command_lines(commands: tuple[tuple[str, ...], ...]) -> str:
    return "\n".join(f"    {shlex.join(command)}" for command in commands)


def _replay_v1(context: BriefContext) -> str:
    body = context.body.rstrip("\n")
    return _REPLAY_V1.format(
        task_key=context.task_key,
        trial_id=context.trial_id,
        worktree=context.worktree,
        repository=context.repository,
        branch=context.branch,
        subject=context.subject,
        body_block=f"{body}\n\n" if body else "",
        tests="\n".join(f"- {path}" for path in context.test_paths),
        oracle=_command_lines(context.oracle),
        gates=_command_lines(context.gates) or "    (none)",
    )


TEMPLATES: dict[str, Callable[[BriefContext], str]] = {
    "replay-v1": _replay_v1,
}


def render(template_id: str, context: BriefContext) -> str:
    """Render ``context`` with the named template. Unknown ids are rejected, never guessed."""
    template = TEMPLATES.get(template_id)
    if template is None:
        known = ", ".join(sorted(TEMPLATES))
        raise ValidationFailed(
            f"unknown brief template {template_id!r}; known templates: {known}",
            details={"template": template_id, "known": sorted(TEMPLATES)},
        )
    return template(context)
