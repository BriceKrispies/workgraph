# Capability frontier

Tracked in Workgraph as parent task WG-1117 (work items below are WG-1118 to WG-1128); progress
lives there (`workgraph task show WG-1117`), not in this document.

## Goal

Point an orchestrator at any repository Workgraph tracks, and have it break every task down,
mechanically, until each piece is **predicted to succeed with a stated probability** when handed
to a given model working alone (Haiku first; the model is a parameter). The prediction must be
calibrated: of the pieces predicted to succeed 80% of the time, about 80% do.

This needs two quantities, measured separately and then chained:

1. **P(success | actual size, model, repository).** How the chance that a model finishes a task
   on its own falls as the task grows. Measured on *replay* tasks built from Git history, where
   the real solution's diff gives the true size.
2. **How well a breakdown predicts size.** On new work no solution exists, so the size a
   decomposer can act on is its own prediction. Each leaf records predicted features before it
   runs; the accepted diff gives the actual features afterwards.

Chained, they give **P(success | predicted features, model, repository)**, the number a decomposer
uses to decide whether a piece is small enough.

## What a trial is

One task, one attempt, one model, one fixed harness, one fixed brief, graded by an oracle the
agent cannot change.

| Fixed by the protocol | Why |
| --- | --- |
| **Model, by full id** (`claude-haiku-5-5`, never `haiku`) | The `haiku` alias has resolved to two models: 75 historical subagent runs that asked for `haiku` ran `claude-haiku-4-5-20251001`, 49 ran `claude-haiku-5-5`. |
| Apparatus and Claude Code version | Tool set, permission mode and context rules change what an agent can do. |
| Brief template id | The brief is part of the treatment. Changing a template means a new id. |
| Repository rules at the base commit (`AGENTS.md`, hooks) | The same agent behaves differently under different rules. |
| Oracle definition | Success is decided before the first trial, not after looking at results. |
| Primary size metric | Chosen in advance so the analysis cannot pick whichever metric looks best. |

**Apparatus.** Trials run as Agent-tool subagents in a Workgraph trial worktree, the same way
the orchestrator will use them for real work. The Agent tool takes only aliases, so the model
cannot be pinned there. Instead the transcript records the model that actually ran (WG-1118),
and a trial whose model differs from its arm's pinned id is `invalid` with cause `wrong_model`.
It stays out of every fit. The `claude -p --model <id>` adapter (`run_start`) can pin a model
exactly and is the fallback if aliases drift too often.

**Markers.** Every brief carries `Task-ID: <KEY>` and `Trial-ID: <uuid>` lines. On `SubagentStop`
Workgraph parses the subagent's transcript. It links the run to the task and trial, and records
the model that ran, token usage, turns, tool calls and tool errors. All of these are measured.

## Size

| Feature | Ex-ante (predicted by the decomposer) | Ex-post (measured on the diff) |
| --- | --- | --- |
| **Non-test source lines added + removed** (primary, log2) | yes | yes |
| Source files touched | yes | yes |
| Packages touched (nearest `Cargo.toml` / `pyproject.toml`) | yes | yes |
| Symbols touched (Tree-sitter, `commit_symbols`) | no | yes |
| Test lines | no | yes |

**Bands** are powers of two of the primary metric: band *b* holds tasks with
2^b to 2^(b+1) - 1 source lines.

Size is not difficulty. Large commits are often renames, generated code or formatting.
Replay leaves out commits flagged as mechanical, and every fit carries the repository (and,
where enough data exists, the package) so that "large" is not confused with "easy".

## Replay tasks (WG-1120)

A commit that changes source and tests together becomes a task:

- **Base**: branch `wg/replay/<short sha>` = the parent commit plus only the commit's test-file
  changes. The agent starts from red tests, which is how the leaf contract works in production.
- **Brief** (`replay-v1`): the commit subject and body, the test files and the command that runs
  them, the repository's agent rules, and the markers. The brief never shows the solution.
- **Oracle**: the commit's tests (`cargo test -p <crate> --test <name>`, `pytest <files>`, or a
  per-repository template), plus the protocol's gate commands.
- **Validation**: the oracle must fail at the base and pass at the real commit, or the candidate
  is rejected. Commits whose tests live in the same files as source (inline unit tests) are
  excluded, because test and source lines cannot be separated.

Measured pool on rho at the time of writing: of 2,604 non-merge commits, 985 change Rust/Rho
source and tests together. Their non-test source sizes span 2^1 to 2^16 lines, with at least 26
commits in every band from 2^3 to 2^12.

## Outcome (WG-1121)

`experiment trial grade` decides the outcome without reading anything the model wrote:

- every oracle and gate command exits 0;
- no oracle/test file differs from the base (**gaming**);
- no stub in added lines: `todo!`, `unimplemented!`, `NotImplementedError`, TODO-only bodies.
  This is the WG-106 failure, where Haiku forced every node complete instead of running it.

`pass` requires all three. Exit codes, flags and the actual diff features are stored on the trial.

Every failed trial gets one **failure cause**, with its source (`mechanical`, `agent`, `human`):

| Cause | Meaning |
| --- | --- |
| `too_big` | The work fit the brief but ran out of coherent progress: partial diff, gave up, lost track. |
| `missing_context` | Needed knowledge the brief and repository did not make findable. |
| `ambiguous_spec` | The brief allowed a different, reasonable reading. |
| `coupling` | The change forced edits far outside the declared area. |
| `gaming` | Changed tests, stubbed, or injected test-only values. |
| `gate_gap` | Passed the oracle but is wrong; recorded when a later review finds it. |
| `harness` | Tool, environment or build failure that is not the model's. |
| `budget` | Stopped by a turn, token or time limit. |
| `wrong_model` | The model that ran is not the arm's model (trial is invalid). |
| `other` | Anything else, with a note. |

## Design: start big, step down (WG-1122)

The question is where the success curve crosses 50% and 80%. An adaptive staircase finds the
threshold in few trials, and starting at the top band pushes the model past its limit first:

1. Start at the top band. A **visit** is two distinct tasks from the band.
2. If at least one of the two passes, go **up one** band; otherwise go **down two**.
3. Stop after **6 reversals** or **24 trials**.
4. **Refine**: 16 trials in the bands within one band of the threshold estimate.
5. **Repeat**: 6 trials re-run tasks already run, to measure how much of the outcome is the
   model's own randomness rather than the task.
6. **Control**: a larger model (Sonnet, pinned id) runs 10 tasks that Haiku ran at and one band
   above its threshold. If the control also fails a task, the brief or oracle is suspect, not
   Haiku's limit.

**Trial budget per repository**: 40 Haiku trials (24 staircase + 16 refine, of which 6 are
repeats) and 10 control trials, so 50 trials per repository and 100 across E1-rho (WG-1125) and
E1-workgraph (WG-1126). Tokens are recorded per trial and reported, not capped. This size gives
threshold estimates with wide but honest intervals. The predictor's data then grows with every
production leaf (below), so the budget is a starting point, not the final sample.

## Analysis

- Pass rate per band with **Wilson** intervals.
- **Logistic regression** of pass on log2 size per arm and repository; the sizes at 50% and 80%
  come from the fit, with **bootstrap** intervals resampling by task with a fixed seed.
- Invalid trials are excluded from every fit and listed with their cause.
- **Calibration** of any predicted probability is reported as a Brier score and reliability bins.

## Predictor (WG-1123)

P(success | ex-ante features, model, repository) is a logistic model over all graded trials.
It uses log2 predicted source lines, log2 files and packages touched, with **per-repository
intercepts shrunk toward the shared fit** (L2). A new repository with few trials therefore starts
near the shared estimate and moves as its own data arrives. Each fit records its data cutoff and
the trials it used, and every prediction is labelled an inference with the fit's id.

**New repository**: before decomposing its backlog, run a short replay calibration on its own
history (the staircase with a smaller budget) to set its intercept.

**Production feedback**: every leaf an orchestrator sends to a model is a `live` trial, with
predicted features recorded before it runs and outcome and actual features afterwards. The
predictor's data keeps growing as it is used.

## Breakdown procedure (WG-1124)

Version `breakdown-v1`. Each step names the command that performs it.

1. **Triage** (`task contract-check`): a task is `not_delegable` when it needs a decision, has no
   checkable oracle, or is a report or register item. These go back to a human or the
   orchestrator.
2. **Contract**: before a leaf goes out the orchestrator writes its red tests, declares the files
   it may touch, its gate commands, what is out of scope, and its predicted features (metadata
   `contract`). Every acceptance criterion names the check that proves it.
3. **Predict** (`experiment predict TASK`): if predicted success meets the target, the task is a
   leaf.
4. **Split** otherwise: by package first, then by acceptance criterion, with interfaces defined
   in the parent before children that use them. Children become child tasks with dependencies.
   Return to step 2 for each child. **Stop splitting as soon as a piece meets the target**:
   finer pieces cost orchestrator work and add more places where pieces fail to fit together.
5. **Run** each leaf with the target model and the `leaf-v1` brief; grade it like a trial.
6. **Escalate** a failed leaf by its cause: `too_big` and `coupling` split again;
   `missing_context` and `ambiguous_spec` fix the contract and rerun; a leaf that fails after
   that goes to a larger model, which is recorded.
7. **Integrate** the leaves as a stack (`docs/design/stacked-integration.md`) and run the full
   checks once. The parent passes only when the integrated stack passes. Leaves that pass alone
   but fail together count against the breakdown, not the leaves.

## Validation of the whole (WG-1127, WG-1128)

- **E2 breakdown trials**: three large tasks per repository go through the procedure end to
  end. Measured: parent success, leaf success, predicted against actual leaf size, calibration.
- **Hold one repository out**: fit on all but one repository and score calibration on the one
  left out. This needs at least three repositories with replay trials, and the user registers the
  third. The predictor is trusted on an unseen repository only after this passes.

## Threats to validity

| Threat | Mitigation |
| --- | --- |
| Model alias drift | Actual model from the transcript; mismatches are invalid. |
| Brief leaks the solution | Templates are generated mechanically from the commit message and tests; no solution text. |
| Oracle too weak (passes wrong code) | Gaming and stub checks; gates; control arm; `gate_gap` recorded when found later. |
| Replay tasks easier than real tasks | E2 runs real backlog tasks; production `live` trials are compared with replay fits. |
| Repository state (red suites on main) | Oracles are the commit's own tests at its own parent, not the whole suite. |
| Selection by size bands | Bands are sampled from the validated pool with a fixed seed. |
| Few trials | Intervals are always reported; production trials keep adding data. |

## Data model and interfaces

- `subagent_runs`: `requested_model`, `model`, `usage`, `telemetry`, `task_id` (WG-1118).
- `experiments`: slug, question, protocol (frozen at start), status.
- `experiment_trials`: task, attempt, subagent run, arm, models requested and actual, phase,
  band, features (predicted and actual), base ref, worktree, status, outcome, outcome detail,
  failure cause and source.
- CLI `workgraph experiment ...` (create, freeze, replay scan/create, next, trial
  start/grade/classify, report, predict), `workgraph task contract-check`,
  `workgraph claude backfill-transcripts`; matching MCP tools for agents.
