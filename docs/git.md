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
- Worktree rows track `preparing -> active -> released -> removed` (or `orphaned`).
  `workgraph worker recover` (and every worker start) reconciles rows with disk after a crash.
- Cleanup is explicit (`workgraph git cleanup-worktrees --apply`), only touches clean,
  Workgraph-created worktrees, and always keeps the branches.

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
