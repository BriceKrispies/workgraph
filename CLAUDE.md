# Agent instructions

- **Every test run has a hard 120 s wall-clock budget.** It is enforced in tooling
  (`wg-conformance --budget`, capped at 120; `tools/check.py`, which kills a test step past 120 s),
  not by convention. A check that does not fit the budget is cut, never lengthened.
- Agents working on a single change run only cheap checks (`cargo check`, `cargo clippy -- -D
  warnings`, `cargo fmt`). Tests run once per stack, not per change.
- **Improving Workgraph from another session goes through the `workgraph-maintainer` agent**, which
  follows [`docs/agents/maintainer.md`](docs/agents/maintainer.md). It changes the code, runs the full
  gate, commits, pushes to `origin/main`, installs the new `workgraph.exe` and reports. It is the one
  agent that runs the full gate on its own change. `C:\dev\workgraph` backs the installed command, so
  never edit it or switch its branch. Work in a worktree.
