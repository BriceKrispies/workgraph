---
name: milestone-breakdown
description: Break a large project or goal into Workgraph tasks as a ladder of milestones, each leaving the system working in a new, checkable state that the next builds on. Use when asked to plan, break down, decompose or add work to Workgraph, and before calling task_create or task_import_breakdown for more than one task.
---

# Milestone breakdown for Workgraph

You are turning a goal into Workgraph tasks. The failure to avoid is the obvious one: writing a
task for every step you can think of. Hundreds of tiny tasks cost more to track, claim, verify
and integrate than the work they describe, and most of them are really the checklist of a few
real tasks. Aim for a **ladder**: a short list of milestones, each one a rung the next stands on.

## What a task is

A task is a **milestone**: a change that leaves the whole system working in a new state that a
check can prove, and that later tasks build on.

Test every candidate with one sentence: **"After this, you can run ___ and see ___."** If you
can't fill it in with something a person or a check could observe, it is not a task.

- *Task:* "After this, `rc build hello.r` produces a native binary that prints hello on Linux."
- *Not a task:* "Add the `Token` enum", "Write tests for the lexer", "Wire the `--target` flag".
  These are **steps**. They go in the acceptance criteria of the task they belong to.

A task should be a coherent piece of work one capable agent finishes in one session: commonly
hundreds to a couple of thousand changed lines, not ten. Workgraph's own measurements
(`docs/design/capability-frontier.md`) found models finishing tasks in the largest size band
tested. When unsure, make the task bigger.

## The procedure

1. **Name the end state** in a paragraph: what exists, what it does, how you would know.
2. **Write the ladder: 6 to 15 milestones**, ordered so each one runs on its own and the next
   builds on it. The first rung is a *walking skeleton*: the thinnest end-to-end version that
   runs. Put founding requirements (for example "agents can control it", "runs on two
   platforms") into the earliest rung where they can exist, not the last, because retrofitting
   them is the expensive part.
3. **Create one root task** for the whole goal, and the milestones as its children, one task
   each. Each milestone gets a description (the "after this, you can run ... and see ..."
   sentence) and 2 to 5 acceptance criteria that name the check proving them. Milestone N
   depends on milestone N-1.
4. **Decompose only the next milestone** into 3 to 8 child tasks, each passing the test above.
   Leave every later milestone as a single task. You will know far more when you get there,
   and a detailed plan for rung 9 written before rung 1 runs is wrong anyway.
5. **Fold steps into criteria.** Before creating a task, ask: would the same agent do this
   straight after its sibling, in the same area, with nothing worth checking in between? Then
   it is one task. Merge them.
6. **When a milestone is accepted**, decompose the next one, using what you learned.

## While working on a task

Something you notice mid-task (a bug nearby, a refactor you'd like, a missing test) is usually
**not** a new task:

- If it's needed for your task, do it as part of your task.
- If it isn't, add a comment to your task (`task_add_comment`) or to the milestone it belongs to.
- Create a task only when it's a separate milestone in its own right, and search first
  (`task_search`, `task_list`).

## Limits Workgraph enforces

- `tasks.max_open_children` refuses a create under a parent that already has that many open
  children, and lists them. That refusal means your breakdown is too fine: merge, or put the
  work into an existing child's criteria. Don't work around it by creating the task without a
  parent.
- `tasks.duplicate_check` refuses a create that matches an open task. Comment on the existing
  one instead.
- `tasks.intake_proposed` holds agent-created tasks as `proposed` until a human admits them.

## Writing the ladder with task_import_breakdown

You can create the milestones in one call from a markdown section (see `docs/cli.md`,
"Importing a breakdown"). Bold headers are tasks, `Depends on:` links them, and `Acceptance:`
bullets become criteria. Create the root task first and pass it as `parent`.

```markdown
## Ladder

**M1 Walking skeleton: hello world, natively, under agent control**
After this, `rc build hello.r` produces a Linux binary that prints hello, and an agent can
connect to the running compiler's control socket and list its stages.
Acceptance:
- `rc build examples/hello.r && ./hello` prints `hello` (CI script `ci/m1.sh`)
- the control-socket test lists the stages lex, parse, check and emit

**M2 Types, modules and a live compiler API**
After this, multi-file programs with structs and generics compile, and an agent can query and
replace a function's definition in a running compile session.
Depends on: M1
Acceptance:
- the `examples/modules` suite builds and passes
- the API test swaps a function body and rebuilds incrementally

**M3 Second target**
After this, the same examples build for Windows from Linux.
Depends on: M2
Acceptance:
- `ci/m3.sh` cross-builds and runs `examples/` under the Windows runner
```

Then, for M1 only, import (or create) its 3 to 8 children with `parent` set to M1's key.

## Checklist before you create anything

- [ ] Every task passes "after this, you can run ___ and see ___".
- [ ] Only the next milestone has children, and it has 8 or fewer.
- [ ] No task is a step another task's agent would do anyway.
- [ ] Each acceptance criterion names the check that proves it.
- [ ] The whole plan fits on one screen.
