# Task lifecycle, verification and acceptance

## States

```mermaid
stateDiagram-v2
    [*] --> backlog
    backlog --> ready
    backlog --> blocked
    ready --> claimed
    ready --> running
    ready --> blocked: dependency unsatisfied
    blocked --> ready: all dependencies accepted
    claimed --> running
    claimed --> ready: release / claim expiry
    running --> awaiting_verification: completion reported, checks configured
    running --> awaiting_review: completion reported, no checks
    claimed --> awaiting_review
    awaiting_verification --> awaiting_review: required checks passed
    awaiting_verification --> failed: required checks failed
    awaiting_review --> accepted: acceptance (automated or manual)
    awaiting_verification --> accepted: recorded manual acceptance
    awaiting_review --> ready: rejected
    running --> failed: attempt failed
    failed --> ready
    failed --> claimed: retry
    accepted --> ready: reopened
    backlog --> cancelled
    ready --> cancelled
    cancelled --> backlog: reopened
```

Transitions are validated by `core/policies/lifecycle.py`. Notable rules:

- **Completion is a claim, not acceptance.** `task complete` / `task_complete` /
  `task_report_result succeeded` move a task to `awaiting_verification` (when checks apply) or
  `awaiting_review`. Only the acceptance service can enter `accepted`.
- Moving to `ready` with unaccepted dependencies lands in `blocked` (reason `dependencies`);
  accepting the last blocker returns dependents to `ready` and emits `task.became_ready`.
  Claiming or running a task with unaccepted dependencies is rejected.
- **Inside a stack, a sibling dependency is satisfied earlier.** When both tasks are items of the
  same active stack, the dependency counts as satisfied once its stack item is `ready`, `merged` or
  `verified` (integration-ready), without acceptance. Setting the item to any of those states
  promotes its blocked dependents; leaving them, or removing the item from the stack, blocks them
  again. Dependencies on tasks outside the stack keep the accepted-only rule. Every satisfaction
  check goes through `satisfied_dependencies` in `core/services/tasks.py`.
- **Inside a stack, children are verified together.** A stack child stays `awaiting_verification`
  while its item moves through the stack. When the stack passes full verification on the
  integration tip, each child moves to `awaiting_review`. A child ejected by bisection returns to
  `ready` with a comment, and its dependents in the stack wait in `pending` for a new attempt.
- Dependencies are workspace-wide (across projects and repositories); cycles in dependencies and
  in the parent hierarchy are rejected with the offending path.
- Claims are exclusive leases; an expired claim is released by maintenance.
- Reopening accepted or cancelled work emits `task.reopened` (counted by the quality metrics).
- Updates accept `expected_version` for optimistic concurrency (`version_conflict` on mismatch).
- Records are archived, never deleted.

Parent tasks show a rollup of descendants (`by_status`, accepted count, progress).

## Attempts

Attempt states: `queued -> preparing -> running -> verifying -> succeeded`, or `failed`,
`timed_out`, `cancelled`, `abandoned`. Attempts are never deleted; retries (`retry_of`),
reruns (`rerun_of`) and nested subagent attempts (`parent_attempt_id`) are linked.
After `execution.max_attempts_per_task` failures the task is tagged `needs-review`
(`task.escalated`) and the scheduler stops retrying it.

## Verification

Verification configurations are **trusted configuration**: only actors with `config:manage`
(humans by default) can create them, and commands are argument vectors (no shell; a shell
interpreter as executable needs `allow_shell: true`).

```sh
uv run workgraph verify config ci --scope-type repository --scope api --check "{\"name\": \"tests\", \"kind\": \"test\", \"command\": [\"uv\", \"run\", \"pytest\", \"-q\"]}"
uv run workgraph verify config integration --scope-type project --scope platform --check "{\"name\": \"contract\", \"kind\": \"integration\", \"command\": [\"python\", \"tools/contract.py\"], \"timeout_seconds\": 600}"
```

Scopes: `workspace`, `project`, `repository` (runs in that repository's checkout), `task`.
Check fields: `name`, `kind` (test, lint, typecheck, build, integration, custom), `command`,
`repository`, `timeout_seconds`, `required`, `env`, `parser` (auto, pytest, generic), `tier`
(`fast` or `full`, default `full`).

Tiers select checks by cost. `fast` checks (lint, format, type check) are the ones an agent is told
to run itself: they are listed as `agent_checks` in the execution manifest. `workgraph verify run
--tier fast` runs only those checks, and a run restricted to the fast tier cannot accept a task
while full-tier checks exist. A run without `--tier` runs every check, as before.

A run records the exact revision manifest under test, each check's exit code, duration, timeout,
log artifact, output excerpt and parsed failure signatures (e.g. `test:tests/test_api.py::test_login`).
Integration checks run in a scratch directory with `WORKGRAPH_MANIFEST` and
`WORKGRAPH_REPO_<NAME>_PATH` for every repository in the manifest.

```sh
uv run workgraph verify run WG-12
uv run workgraph verify failures --check "unit tests"
```

## Acceptance

- **Automated**: `workgraph task accept WG-12` requires that the latest verification run passed
  all required checks.
- **Manual**: `workgraph task accept WG-12 --manual --reason "reviewed the diff"` records an
  explicit decision by the actor (required for tasks with no automated checks).
- **Reject**: `workgraph task reject WG-12 --reason "..."` sends the work back to `ready`.

Acceptance stores the accepted revision manifest and queues the final-change-surface computation.
Workgraph never merges, pushes or publishes accepted work, with one exception: accepting the parent
of a stack lands the stack (see below).

### Acceptance in stacks

- **Parent.** Once a stack's full verification passes, the parent task moves to `awaiting_review`
  (walking `ready` and `claimed` first if it was not already there), with the stack's passing run as
  its verification. A parent that cannot be claimed, for example because its own dependencies are
  unmet, stays where it is and the reason is recorded as `stack.parent_not_reviewable`.
- **Accepting the parent lands the stack.** `workgraph task accept WG-2` (automated) or
  `workgraph task accept WG-2 --manual --reason "..."` requires the stack to be `verified`; otherwise
  it is refused and nothing changes. A verified stack lands: each verified child is accepted
  (`mode` `automated`, linked to the stack's passing run), then the parent is accepted, and the stack
  becomes `landed`. The default branch moves to the verified integration tip. The details are in
  [Git](git.md#stack-landing).
- **Unfinished children refuse the landing.** Accepting the parent means the whole piece of work is
  done, so every item must be `verified`. A stack with a `pending`, `ready`, `conflict` or `ejected`
  child is refused, the refusal names each unfinished child and its state, and the children are either
  finished or removed from the stack first.
- **The default branch moved.** If the default branch moved after the stack was cut, nothing lands
  and the call is refused with `landing: deferred`. The stack is rebuilt on the new head, verified
  again, and lands by itself once that verification passes.
- **Auto-accept.** By default (`stacks.auto_accept = true`), a stack lands as soon as it verifies, as an
  automated acceptance of the parent with the reason `stack.auto_accept`.
- **Rejecting the parent** (`task reject`) leaves the stack as it is and records `stack.parent_rejected`.
  Nothing lands.
- **Membership.** Children join and leave an active stack with `workgraph stack add PARENT TASK` and
  `workgraph stack remove PARENT TASK` (or the matching API routes and MCP tools, which need
  `stack:write`). A child can only join if all its repositories are part of the stack, and a child that
  is `merged` or `verified` on the integration branch cannot leave it.
- **A child on its own.** Accepting a child that is an item of an active stack accepts it as usual
  (automated, or `--manual --reason` as for any child) and removes it from the stack. If its commits
  were on the integration branch, the branch is rebuilt without them, its in-stack dependents go back
  to `pending`, and the remaining stack is verified again. Its change is not landed by the stack.
