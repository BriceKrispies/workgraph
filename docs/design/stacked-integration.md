# Stacked integration

Status: **accepted design, not implemented.** Tracked as parent task WG-2 in the `workgraph`
repository; work items 1-7 below are WG-3 to WG-9.

## Problem

Verification today is per attempt. When an agent finishes and the task has checks, the full check
set runs in that attempt's managed worktree (`orchestration/runs.py`), and `task accept` reads the
result of that run. For work made of several changes this is the wrong unit:

- Each change pays for a full test run, and agents usually run the suite themselves first as well.
- Changes are verified in isolation. Two changes that pass alone can fail together, and nothing
  catches that until they meet on `main`.
- Dependent changes cannot start until their dependency is **accepted**, so related work is
  serialised through human review.

## Model

A multi-change piece of work is a **stack**: a parent task whose child tasks are the changes.
Parent/child tasks, dependencies and rollups already exist; the stack adds an integration branch
and the machinery to fill it, verify it once and land it.

```
main ──●─────────────────────────────────────────────── (fast-forward on acceptance)
       │ base_sha (fresh off main)
       ├── wg/WG-11/attempt-1 ──●──●            (child, no sibling dependency)
       ├── wg/WG-12/attempt-1 ──●               (child, no sibling dependency)
       │                         └── wg/WG-13/attempt-1 ──●   (child, depends on WG-12)
       │
       └── wg/stack/WG-10 ── merge WG-11 ── merge WG-12 ── merge WG-13 ── [full checks once]
```

### 1. Stack creation

- A stack belongs to one parent task and one repository per participating repository. It records
  `base_sha`, resolved from the repository's default branch **at creation**, and the integration
  branch name `wg/stack/<parent-key>` (the key lowercased, e.g. `wg/stack/wg-2`).
- The integration branch is always cut **fresh from `main`** and is never rebased. If `main` moves
  before the stack lands, the integration branch is rebuilt from the new `main` and the merged
  children are replayed in their recorded order (see step 6).
- Per-child integration state lives in `stack_items` (`pending`, `ready`, `merged`, `conflict`,
  `ejected`, `verified`), not in the task lifecycle enum. Children stay in
  `awaiting_verification` while they move through the stack.

### 2. Worktree bases come from the dependency graph

When an attempt for a stack child is prepared (`WorktreeManager.prepare`, via the existing
`RepoRequest.target_ref`):

- a child with no dependency on a sibling branches from the stack's `base_sha`, so independent
  children run in parallel;
- a child that depends on a sibling branches from that sibling's attempt head (its
  `ChangeSetItem.head_sha`), which is true stacking.

Inside a stack, a sibling dependency is satisfied when the sibling is integration-ready (step 3),
not when it is accepted. Dependencies on tasks outside the stack keep the existing rule.

### 3. Check tiers

`CheckSpec` gains `tier: "fast" | "full"` (default `full`, so existing configs keep their
behaviour).

- When a stack child's attempt finishes, only `fast` checks run in its worktree (lint, format,
  type check). Passing marks the stack item `ready`; failing fails the attempt as today.
- The agent's execution manifest lists the fast checks as the checks the agent should run, so
  agents stop running the full suite themselves.
- Tasks outside a stack keep running every tier, as today.

### 4. Integration queue

- Ready items are merged onto the integration branch in dependency order, then priority, then
  readiness time. Each merge is a job holding the `repo:<id>` resource key, so integration into a
  repository is serialised by the existing job queue.
- Merging happens in a dedicated managed worktree for the integration branch, never in a user
  checkout.
- A merge conflict aborts the merge, marks the item `conflict` and returns the child to its agent
  (a new attempt) with the conflicting paths and the instruction to rebase onto the current
  integration tip. The rest of the stack continues.

### 5. Full verification once, on the integration tip

- When no item is `pending` or `ready` (everything is `merged`, `conflict` or `ejected`), the `full`
  tier runs once against the integration tip. The run records a revision manifest of the tip.
- **Pass:** every merged item becomes `verified`, each child moves to `awaiting_review`, and the
  stack becomes `verified`.
- **Fail:** bisect over the merge sequence. Run the failing required checks against successive
  prefixes of the merged children (binary search over the recorded merge commits) to find the first
  child whose merge introduces the failure. Eject it (state `ejected`, child returned to its agent
  with the failure signatures), rebuild the integration branch without it, and re-run the full tier
  on the remainder.

### 6. Stack acceptance

- Accepting the parent task accepts the stack: if `main` still equals `base_sha`, `main` is
  fast-forwarded to the verified integration tip; otherwise the integration branch is rebuilt
  fresh from the new `main` (step 1), the full tier runs again, and the stack lands only if it
  passes.
- Every verified child gets a `TaskAcceptance` (mode `automated`, linked to the stack's
  verification run and the accepted manifest), then the parent is accepted.
- A workspace or project policy `stack.auto_accept` (default off) accepts a stack automatically when
  its full verification passes, so a human is only involved when something fails.
- Accepting a single child on its own remains possible (manual acceptance with a reason) and
  removes it from the stack.

## What already exists

Parent/child tasks, dependencies and rollups; managed worktrees with `RepoRequest.target_ref`;
change sets with base/head/commit SHAs; revision manifests; verification configs, runs and failure
signatures; `TaskAcceptance`; job queue resource keys and leases; event log and automation
triggers.

## Work breakdown

Each item depends on the ones it lists. Items without a dependency between them can proceed in
parallel.

1. **Check tiers** - `CheckSpec.tier`, tier selection in verification runs, fast checks in the
   agent execution manifest. *(no dependencies)*
2. **Stack model** - `stacks` and `stack_items` tables (migration `0003`), stack service (create
   fresh off `main`, add/remove children, state), read endpoints in CLI/API/MCP. *(no dependencies)*
3. **Dependency-driven worktree bases** - attempt base from stack base or sibling head; sibling
   dependency satisfied at integration-ready. *(depends on 2)*
4. **Integration queue** - ready-on-fast-pass, ordered merge jobs on `repo:<id>`, integration
   worktree, conflict handling and return to agent. *(depends on 1, 3)*
5. **Stack verification and bisect** - full tier on the integration tip, verified/eject transitions,
   prefix bisection, rebuild without the culprit. *(depends on 4)*
6. **Stack acceptance** - parent acceptance lands the stack (fast-forward or rebuild from new
   `main`), child acceptances, `stack.auto_accept` policy, single-child acceptance removes it from
   the stack. *(depends on 5)*
7. **Interfaces and documentation** - `workgraph stack ...` CLI group, API routes, MCP tools,
   web stack view, `docs/git.md`, `docs/task-lifecycle.md`, `docs/IMPLEMENTATION_STATUS.md`.
   *(depends on 6)*
