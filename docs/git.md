# Git integration and multi-repository workflows

Workgraph uses the Git CLI (argument vectors, `-C <path>`, no shell, prompts and pagers disabled)
and never rewrites history, pushes or merges.

## Repository identity

A repository's canonical identity is its Workgraph UUID. Registering a path computes:

- the **identity fingerprint** (sorted root-commit SHAs),
- normalised remote URLs (`git@github.com:Org/Repo.git` = `https://github.com/Org/Repo`),
- default branch, shallow state, checkout kind (clone or linked worktree).

Registering the same history at another path (a second clone, a moved directory, a renamed
remote) adds a **checkout** to the existing repository instead of creating a duplicate.
`workgraph repo refresh` re-validates every checkout and reports `ok`, `degraded` or
`unavailable`. `workgraph repo remove` archives the registration only; files are never touched.
Repositories without remotes or without commits are supported.

## Ingestion

```sh
uv run workgraph git sync
uv run workgraph git sync api --full
```

- **Incremental** (default): new commits are found with `git rev-list --all --not <previous tips>`.
- **Full**: every reachable commit is reconciled. Commits no longer reachable from any ref are
  marked `reachable = false` (with `unreachable_since` and whether the object still exists);
  commits that reappear are restored. A non-fast-forward ref move triggers this automatically.
- Per commit: SHA, tree, parents, author/committer, timestamps, subject, message, trailers,
  merge flag, patch-id, branches (tracked/local), and per file: status (A/M/D/R/C/T), rename
  similarity and old path, insertions, deletions, binary flag, language, file class
  (production/test/config/docs) and module. Merge commits are measured against their first parent.
- Structural analysis jobs run Tree-sitter over each non-merge commit (Python, C#, TypeScript/TSX,
  JavaScript, Go, Rust) and record functions/types added, modified and deleted. Other files fall
  back to line-level statistics with the reason recorded.
- Full diffs are never stored; `git diff`/`git show` are re-run on demand. Missing objects are
  recorded in `git_object_gaps` and reported.

## Attribution

| Source | Confidence | Evidence |
| --- | --- | --- |
| `explicit` (`git link`, `task_link_commit`) | 1.0 | who linked it, note |
| `execution_context` (commits between an attempt's baseline and outcome) | 0.95 | attempt, range |
| `trailer` (`Task-ID: WG-12`, `Workgraph-Task:`) | 0.95 | trailer text |
| `worktree` (reachable from a managed worktree branch) | 0.9 | branch |
| `rewrite_reconciliation` (carried over to a rebased/amended commit) | ≤ 0.85 | original SHA |
| `message_inference` (task key mentioned in the message) | 0.5, or 0.35 if several keys | matched keys, "inferred" label |

A commit may belong to several tasks and a task to many commits. Analytics count a shared commit
once per repository and de-duplicate rewritten/cherry-picked copies by patch-id.

**Rewrites:** a new commit with the same patch-id as an unreachable one (rebase) or the same
author, author date and subject (amend) is recorded as a `rewrite` equivalence and inherits the
original's links. Same patch-id while both are reachable, or a `(cherry picked from commit …)`
trailer, is a `cherry_pick`.

## Cumulative churn versus final change surface

These are separate measurements and are always reported separately:

- **Cumulative churn**: lines added/removed summed over the task's distinct linked non-merge commits
  (confidence ≥ 0.5), plus files touched, repeated edits and structural counts.
- **Final change surface**: `git diff --numstat -M` between the first attempt's baseline revision
  and the accepted revision (or, without attempts, the parent of the earliest linked commit and the
  latest linked commit), with renames, binary files and functions/types affected.
  Computed automatically after acceptance (`workgraph git surface WG-12` recomputes).

A high churn/final ratio indicates code rewritten during the task. Neither number measures
difficulty or necessity.

## Managed worktrees and execution manifests

Each agent attempt receives an **execution manifest**: task and attempt ids, every repository
with checkout path, baseline SHA, target ref, access mode, isolation, expected changes and the
applicable verification checks.

- Writable repositories get a managed worktree on a new branch
  `wg/<task-key>/attempt-<n>-<id>` under the Workgraph data directory.
- Read-only repositories get a detached snapshot worktree; any commit or edit there is detected
  as a policy violation and fails the attempt.
- With `execution.isolation = "in_place"` the user checkout is used under an exclusive lease;
  a second incompatible attempt fails with `resource_busy`.
- Parallel attempts on the same repository each get their own worktree.
- **Base inside a stack.** When the task is an item of an active stack and the stack covers the
  repository, the attempt's target ref is chosen from the dependency graph, unless the caller passed
  one (reruns do). A child with no sibling dependency that changed the repository starts from the
  stack's `base_sha`, so independent children run side by side. A child that depends on siblings
  starts from the head its latest succeeded attempt left in the repository (the sibling's
  `ChangeSetItem.head_sha`). If several siblings changed the repository, the one whose head builds
  on the others is the base, and the rest are recorded as `superseded`. If two of those heads are not
  on one line of history, preparation fails with a `ValidationFailed` asking for an explicit
  dependency between the two siblings. The chosen source, base and sibling are recorded in the
  manifest entry (`base`) and in the `attempt.workspace_prepared` and `worktree.created` events.
  Tasks outside a stack, and repositories a stack does not cover, use the default branch as before.
  `in_place` isolation writes the user checkout directly, so no stack base is applied there.
  A child whose item is `conflict` starts from the integration tip instead (`source`
  `integration_tip`), so it resolves the conflict on top of what is already integrated.
- **Integration branch and merge queue.** Each stack repository has one integration branch,
  `wg/stack/<parent-key>` with the key lowercased (`wg/stack/wg-2`), created at the stack's `base_sha` the first time it is needed. Its
  worktree lives at `worktrees/stacks/<stack-id>/<repository>` and is recorded in
  `managed_worktrees` with no attempt, so it stays `active` while the stack is open and is never
  released by an attempt. Workgraph never merges in a user checkout.
  - **Fast gate.** When an attempt of a stack child succeeds, only the `fast` checks run
    (`CheckSpec.tier`). A pass moves the stack item to `ready` and leaves the task
    `awaiting_verification`; a fail fails the attempt and the task as usual. A child with no fast
    checks becomes ready without a run.
  - **Queue.** Each ready item enqueues a `stack_integrate` job per stack repository. The job holds
    the exclusive `exclusive:stack:<stack-id>:<repository-id>` key, which only this stack's
    integration and verification hold, so another stack on the same repository keeps merging while
    this one verifies. Its idempotency key names the stack's latest event, so repeated requests for
    an unchanged stack collapse into one job. `workgraph stack integrate PARENT` and
    `POST /ws/{ws}/stacks/{ref}/integrate` queue it on demand.
  - **Order.** An item is due once its in-stack dependencies are `merged` or `verified`. Due items
    are taken by dependency depth, then task priority, then the time they became ready.
  - **Merge.** For each due item the head its latest succeeded attempt left in the repository is
    merged with `git merge --no-ff --no-edit` in the integration worktree, committed as Workgraph's
    integration identity. The merge commit is recorded per repository in the item's `detail`
    (JSON: `merges` and `conflicts`) and in the `stack.integration_merged` event. An item becomes
    `merged` once every repository it changed has merged its current head, and the stack moves
    `open -> integrating` at the first merge.
  - **Conflict.** A merge that conflicts is aborted (`git merge --abort`). The item moves to
    `conflict` with the conflicting paths (`git diff --name-only --diff-filter=U`), the integration
    tip and an instruction to rebase onto that tip. The task returns to `ready` with a comment
    carrying the same information, and the rest of the queue continues.
  - Items with no change in a repository are skipped for that repository (`stack.integration_skipped`).
  - **Settled.** The queue settles when no item is `pending` or `ready` and at least one item is on
    the integration branch. After every integrate job that finds the stack settled, a
    `stack_verify` job is queued, keyed by the integration tips, so repeated settling on the same
    tips collapses into one job. `workgraph stack verify PARENT` and
    `POST /ws/{ws}/stacks/{ref}/verify` queue it on demand; on demand the key also names the stack's
    latest event, so each request after a state change runs again. The job holds the stack's own
    `exclusive:stack:<stack-id>:<repository-id>` key for each of its repositories, plus
    `verify:<repository-id>` (bounded by `jobs.verify_concurrency`, default 1), so two heavy
    verifications never run at once on one repository, while the other stacks' integration on that
    repository continues. The stack moves `integrating -> verifying` while it runs.
  - **Full verification on the tip.** The parent task's `full` checks (workspace, project and
    repository scoped checks of the stack's repositories) run once, attributed to the parent task,
    with each repository's checks executing in its integration worktree at the current tip. The
    run's events carry the stack id and the tip shas. A stack with no full checks passes without a run.
  - **Regression checks on the tip.** A required check in `baseline: "regression"` mode that fails on
    the tip passes the stack when each failure signature it reports also fails the same check on the
    stack's base. The base outcome comes from the most recent earlier run of the parent whose manifest
    holds exactly the base commits (the check results of that run are reused), and otherwise from a new
    run at the base in the bisect worktrees. A regression-mode failure with a new signature, a timeout,
    or no parsed signature counts as failing. `absolute` checks are judged as before. The
    `stack.verified` event carries the base failures it tolerated under `tolerated`. A base outcome is
    reused only when its check did not error and the run ran the check with the same definition as the
    current `CheckSpec` (command, env, parser, baseline and the rest, compared with the run's
    `config_snapshot`); otherwise the base is measured again.
  - **Errored checks stop the stack.** A check whose run is `error` (its exit code is listed in
    `error_exit_codes`, or it was cancelled) judged nothing. If a required check errors on the tip, on
    the base or on a bisect probe, the verification stops: no pass, no bisection, no ejection. The stack
    returns to `integrating` and `stack.verification_errored` records the run id, the stage (`tip`,
    `base` or `bisect`), the errored check names and the tail of each output. Fix the check or the
    runner, then verify again.
  - **Pass.** Every merged item becomes `verified`, each child task moves `awaiting_verification`
    to `awaiting_review` (the reason names the run), and the stack becomes `verified`. Nothing is
    accepted; acceptance is a separate step.
  - **Fail: bisect.** The merge sequence is the items on the integration branch in `merge_order`.
    Prefix `k` takes, for each repository, the merge commit of the last of the first `k` items that
    merged into it (the base commit if none). The checks that count against the tip (required checks
    that failed, judged by the regression rule above) are run against successive prefixes, with a
    binary search for the smallest failing prefix, assuming failure is monotonic. Probes run in one
    detached bisect worktree per repository at `worktrees/stacks/<stack-id>/bisect-<repository>`,
    removed afterwards. Each probe is a verification run and a `stack.bisect_probe` event with its
    shas and result. A probe fails when it reports a new failure signature for those checks. A probe
    whose check errors stops the bisection (see errored checks above); nothing is ejected from it.
  - **Eject and rebuild.** If an `absolute` check fails on the base, nothing is ejected: the stack
    returns to `integrating` (`stack.verification_failed_on_base`) and the items stay `merged`.
    Otherwise the culprit at the failing prefix moves to `ejected`, with its failing check names, the
    new failure signatures (not the ones the base already had) and run id in the item `detail`. Its task returns to `ready` with a comment carrying
    the same information. Items that depend on the culprit, directly or through other items, move
    back to `pending` with the reason `dependency <KEY> was ejected`. Each integration branch is then
    reset to its base (`git reset --hard`, in the integration worktree only) and the remaining items
    are replayed in merge order with the integration identity, updating `merges`. A replay that
    conflicts is handled like an integration conflict. The stack is `integrating` again and
    verification is queued on the rebuilt tips. An ejected child's next attempt starts from the
    integration tip.
- Worktree rows track `preparing -> active -> released -> removed` (or `orphaned`).
  `workgraph worker recover` (and every worker start) reconciles rows with disk after a crash.
- Cleanup is explicit (`workgraph git cleanup-worktrees --apply`), only touches clean,
  Workgraph-created worktrees, and always keeps the branches.

## Stack landing

Accepting the parent task of a verified stack lands it (`workgraph task accept PARENT`, or
`--manual --reason`). Nothing else moves a default branch: a child accepted on its own does not.

- **Locks.** The landing runs inside the stack's `stack_verify` job, so it holds that stack's keys
  and the repository's `verify:` key: no integration or verification of the same stack runs
  alongside it, and with the default `jobs.verify_concurrency = 1` no other stack's verification
  does either. Integration of other stacks is not blocked. A landing that is accepted directly
  (`task accept PARENT`) runs in the request, outside the job queue; it is guarded by the stack's
  own integration state, and its default-branch moves are compare-and-swap, so a branch another
  landing moved first is refused, not overwritten.
- **Checks first.** For every stack repository, the default branch head is read. When it still
  equals the stack's `base_sha`, the default branch fast-forwards to the verified integration tip
  (`wg/stack/<parent-key>`, the tip recorded by the passing `stack.verified` event). The integration
  branch must still be at that tip, and the tip must descend from the base. Every check runs before
  any ref moves, so a refusal leaves every repository untouched.
- **Fast-forward.** If a user checkout has the default branch checked out, the branch moves with
  `git merge --ff-only <tip>` in that checkout, and the checkout must be clean (`git status
  --porcelain` empty), otherwise the landing is refused with a `ValidationFailed` naming the path. If
  no checkout has it, the branch moves with `git update-ref refs/heads/<default> <tip> <base_sha>`, a
  compare-and-swap that fails if the branch moved in between. A failure part way through restores the
  repositories already moved.
- **The default branch moved.** Nothing lands. The stack's `base_sha` is updated to the new head for
  each repository, the stack is rebuilt from it (the integration branches are reset to the new base
  and the merged children are replayed in merge order), and a `stack_verify` job is queued. The
  landing request (mode, reason, actor) is kept on the stack in its `land_request` column and the
  event `stack.land_requested` records the old and new base shas. The accept call is refused with
  `landing: deferred`. When the rebuilt stack verifies, it lands by itself. If that verification
  fails, the stack goes to the usual bisection and the request stays pending for the next pass.
- **Before verification.** A stack whose default branch moved before its verification started is rebuilt
  onto the new head first (`stack.rebased`, no landing request), so the full checks run once, on the tip that lands.
- **On landing.** Each verified child is accepted (`mode` `automated`, `verification_run_id` the
  stack's passing run, the accepted manifest from that run) and then the parent is accepted with the
  requested mode and reason. The stack becomes `landed`, the `stack.landed` event records the landed
  tip of each repository, and the integration worktrees are removed (`worktree remove --force`) and
  the integration branch is deleted (`stack.integration_removed`). Dependents of the accepted tasks
  are promoted by the normal acceptance path.
- **Refusals.** A landing that is refused (stack not verified, parent not awaiting review, any child
  not `verified` (listed with its state, for example `ejected` or `pending`), a child not awaiting
  review, dirty checkout, integration work in flight) is recorded as `stack.landing_refused` and changes nothing
  else. With `stacks.auto_accept` enabled, the landing runs when the stack verifies, and a refusal
  leaves the stack `verified` for a manual accept.
- **Auto-accept.** `stacks.auto_accept` (on by default; global, overridable per workspace; see
  [Configuration](configuration.md)) lands a stack as soon as it verifies, as an automated acceptance
  of the parent with the reason `stack.auto_accept`.

A child accepted on its own is removed from the stack. Its commits, if they were on the integration
branch, are removed by rebuilding the branch without them, which the stack then verifies again. The
change itself is not landed, so it reaches the default branch only if it is landed by other means.

**Inspecting a stack.** `workgraph stack show PARENT` lists each repository with its base sha,
integration branch and tip (blank while the integration worktree does not exist), and the children in
merge order with their state and, for conflicts and ejections, the paths or failed checks.
`workgraph stack history PARENT` (also `GET /ws/{ws}/stacks/{ref}/history` and `stack_history`)
returns the stack's events in order, together with the verification runs they name. The web Stacks
page shows the same data, with the bisection timeline.

## Cross-repository change sets

An attempt touching several repositories creates a change set with one item per writable
repository. Git cannot commit atomically across repositories, so each item is tracked
independently (`pending`, `committed`, `verified`, `failed`) and the set is `open`,
`partially_applied`, `applied`, `partially_verified`, `verified` or `failed`. Exact revision
manifests are stored for the baseline, the outcome, every verification run and the accepted state.

## Optional Git hook

`workgraph git install-hook api` adds a `post-commit` hook that queues an incremental sync. It is
never required: periodic and on-demand syncs collect everything, and the hook refuses to overwrite
a hook it did not create.
