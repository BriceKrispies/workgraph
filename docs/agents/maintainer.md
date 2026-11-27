# Workgraph maintainer: change, land, push and install, end to end

This file is the procedure for the `workgraph-maintainer` agent. Another session sends it off with
one Workgraph task that describes a gap in Workgraph. The agent then works without help:
1. changes the code;
2. verifies the change;
3. commits it;
4. pushes it to `origin/main`;
5. replaces the installed `workgraph.exe` on this machine with the new build;
6. reports back.

Follow every step in order. When a step says stop, stop: leave the work in place and report.

## Standing authority and hard limits

The repository owner has authorised this agent to commit, push to `origin` (`main` only, fast-forward
only) and install the binary without asking. The agent is also the coordinator for its own change, so
it runs the full gate itself. That is an exception to the usual rule that subagents only run cheap
checks.

Never:
- **Edit or switch the live checkout.** `C:\dev\workgraph` backs the installed command. Never switch
  its branch, edit files in it, or build in it. The only write allowed there is the final
  fast-forward in step 9.
- **Force-push, rewrite published history, or push any branch other than `main`.**
- **Raise a test budget.** A check that does not fit in 120 s is cut, never lengthened (`CLAUDE.md`,
  `docs/testing.md`).
- **Mention AI in git or task text.** No mention of AI, Claude, models or assistants in commit
  messages, authors or trailers, and no `Co-Authored-By` or session links. Write as the repository
  owner would. Commits carry a `Task-ID: WG-<n>` trailer.
- **Kill another session's `workgraph.exe mcp` process.** It is that session's tool connection.
- **Delete a directory with `rm -rf`.** Remove worktrees with `git worktree remove`.

## Environment for every command

Use absolute paths with `git -C` and `cargo --manifest-path`. Never rely on `cd`. Set these on every
cargo, bun and python command:

```sh
export RUSTUP_TOOLCHAIN=1.96.0-x86_64-pc-windows-msvc   # the pinned toolchain; the gnu default breaks float goldens
export CARGO_TARGET_DIR=C:/dev/workgraph-wt-<n>/target  # private to this worktree, never shared
export CARGO_BUILD_JOBS=2 RUST_TEST_THREADS=2           # tests run with at most 2 jobs on this machine
```

`<n>` is the task number, for example `WG-5700` → `5700`. `W=C:/dev/workgraph-wt-<n>` below.

## 1. Take the task

1. Read the task: `workgraph --json task show WG-<n>`.
2. If you were given only a description of the gap, create the task first: tag `workgraph`,
   repository `workgraph`, testable acceptance criteria.
3. Claim the task. If the claim is refused because the task is `proposed`, run
   `workgraph task update WG-<n> -s ready` first.

## 2. Create a worktree from the remote tip

```sh
git -C C:/dev/workgraph fetch origin
git -C C:/dev/workgraph worktree add -b wg-<n>-<slug> C:/dev/workgraph-wt-<n> origin/main
```

- Base the work on `origin/main`, never on the live checkout's `main`, which may be behind.
- If the directory exists from an earlier attempt, inspect it before reusing or removing it.

## 3. Understand before changing

Read:
- `CLAUDE.md` and `docs/testing.md`;
- the docs for the surface you touch: `docs/cli.md`, `docs/mcp.md`, `docs/api.md`, and the
  `docs/data-model.md` migration notes;
- the code paths, found with grep.

Reuse existing validation and helpers rather than writing parallel ones.

## 4. Implement

- **Code and tests together.** Tests cover the acceptance criteria. Add a negative case for every
  refusal.
- **Documentation in the same change:** `docs/cli.md`, `docs/mcp.md` and any other doc that
  describes the changed behaviour.
- **MCP schema changes** must update all four of these together, or the contract tests fail:
  - `contracts/mcp.json`, the tool schema;
  - `crates/wg-iface/src/mcp/argument_order.json`;
  - the `tools/list` row of `contracts/mcp/stdio.jsonl`, the recorded transcript;
  - `golden/pydantic/mcp/<tool>.json`. Each `ok` output lists every argument with its default.
- **Edit JSON fixtures without changing number formatting.**
  - Rewriting `stdio.jsonl` or a golden through a serializer that prints `1.0` as `1` changes the
    values the tests compare.
  - Use Python's `json`, after checking that `json.dumps(json.loads(line))` reproduces the original
    line, with the indent, `ensure_ascii` and trailing newline the file already uses.
  - Or make a minimal text edit.
- **Keep error messages pinned by Python-compatibility goldens** (`golden/`) unchanged. If a message
  must change, update its golden in the same commit and say why.
- **Database migrations.** A change that adds a migration migrates the live database on the first
  run of the new binary. Keep migrations data-compatible, and step 8 backs the database up first.

Run cheap checks while iterating:

```sh
cargo fmt --manifest-path $W/Cargo.toml --all
cargo check --manifest-path $W/Cargo.toml --workspace --tests
cargo clippy --manifest-path $W/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path $W/Cargo.toml -p <crate> <new test names>   # only your new tests while iterating
```

## 5. Run the gate

Run the repository's gate on the finished change, from the worktree:

```sh
python $W/tools/check.py --tests
```

It runs, in order:
- fmt, clippy and purity;
- the frontend checks;
- the workspace tests and the conformance corpus, under the 120 s budget.

**It must pass.**
- On a failure, fix the cause and rerun.
- If a failure is outside your change, check whether it also fails on unmodified `origin/main`, in
  a second scratch worktree. If it does, report it as pre-existing and stop. Do not land on a red
  gate.

## 6. Commit

```sh
git -C $W add -A
git -C $W commit
```

The commit message:
- states what Workgraph can now do, and why;
- is written in the owner's voice;
- ends with the trailer `Task-ID: WG-<n>`.

The author is the identity git is configured with. Check it with `git -C $W log -1 --format='%an <%ae>'`;
it must not be a model or `noreply@anthropic.com`.

## 7. Integrate and push

```sh
git -C $W fetch origin
git -C $W rebase origin/main          # only if origin/main moved since step 2
```

- If you rebased, rerun the gate (step 5).
- Then push as a fast-forward:

```sh
git -C $W push origin HEAD:main
```

- If the push is rejected because `origin/main` moved, fetch, rebase, rerun the gate and push again.
- Never use `--force`.
- If the rebase conflicts in a way you cannot resolve with confidence, push the branch
  (`git -C $W push origin wg-<n>-<slug>`) instead of `main`, stop, and report.

## 8. Install the new build

Only one install may run at a time, so take the lock first.
- **The lock is a directory:** `%LOCALAPPDATA%\workgraph\install.lock`. Create it with `mkdir`; it
  fails if the lock is held.
- **If it is held:** check its age. Older than 30 minutes means a dead install, so remove it with
  `rmdir` and take it. Otherwise wait for it, checking once a minute, for at most 15 minutes.
  If it is still held after that, stop and report.

With the lock held:

1. **Build what is on the remote.** Fetch, and confirm `origin/main` contains your commit. Build
   from `origin/main`'s tip, which normally equals your pushed `HEAD`, so installs only move forward:
   ```sh
   bun install --cwd $W/web && bun run --cwd $W/web build    # the server embeds web/dist
   cargo build --manifest-path $W/Cargo.toml --release -p wg-cli
   ```
2. **If the change adds a database migration**, back up the database with the *old* binary first:
   `workgraph backup`. Note the archive path.
3. **Replace the binaries in place**, in `C:/Users/Brice/.local/bin`. The MCP registration and the
   Claude Code hooks store this exact path.
   - Rename the current `workgraph.exe` to `workgraph.<yyyy-mm-dd>-<short-sha-of-old-build>.exe`.
     Renaming works while MCP servers are running the old file; overwriting does not.
   - Copy `$W/target/release/workgraph.exe` in as `workgraph.exe`.
   - Do the same for `workgraph-demo-agent.exe`.
4. **Verify the installed command:**
   - `workgraph --version`;
   - `workgraph doctor`;
   - a smoke command that exercises the new behaviour, for example the new CLI flag on a scratch
     task, cleaned up afterwards.

   If any of these fail, rename the backup back to `workgraph.exe` (and restore the database
   backup with `workgraph restore` if a migration ran). Release the lock, then stop and report.
5. **Restart the web server if it was running.** If `workgraph serve` was running before step 3
   (`Get-CimInstance Win32_Process -Filter "Name='workgraph.exe'"` shows its command line), stop it
   and start it again with the same command line, detached.
6. **Leave stdio MCP servers alone.** Do not kill the `workgraph.exe mcp ...` processes. Each one
   belongs to a Claude Code session and keeps running the old binary until that session reconnects
   (`/mcp` → reconnect) or restarts. Record their PIDs and start times for the report.
7. **Release the lock:** `rmdir`.

Keep at most the five newest dated backups of `workgraph.exe` in `.local/bin`. Delete older ones
only if they match the `workgraph.<date>-<sha>.exe` pattern; leave other files alone.

## 9. Bring the live checkout up to date

```sh
git -C C:/dev/workgraph branch --show-current     # must print main
git -C C:/dev/workgraph status --porcelain        # must print nothing
git -C C:/dev/workgraph fetch origin
git -C C:/dev/workgraph merge --ff-only origin/main
```

If it is not on `main`, has local changes, or cannot fast-forward, do not touch it. Say so in the
report.

## 10. Clean up

Run this from outside the worktree. Windows refuses to remove a directory that a shell is inside.

```sh
git -C C:/dev/workgraph worktree remove --force C:/dev/workgraph-wt-<n>
git -C C:/dev/workgraph branch -d wg-<n>-<slug>
```

On any earlier stop, keep the worktree and say where it is.

## 11. Record and report

1. In Workgraph, comment on `WG-<n>` with:
   - the commit SHAs on `origin/main`;
   - the gate result (pass, and the test-binary count);
   - the installed version and the backup file name;
   - the database backup, if any;
   - the MCP server PIDs still on the old binary.
2. Then report the result: `workgraph task result WG-<n> succeeded`.

Reply to the session that sent you with exactly these sections:

- **Outcome:** landed and installed / landed, not installed / not landed, with the reason.
- **Commits:** SHAs and one-line subjects on `origin/main`.
- **Verification:** the gate result, and the smoke command with its output.
- **Install:** the new version, the backup file, the database backup, and whether `workgraph serve`
  was restarted.
- **Still on the old binary:** the MCP server PIDs. Tell the user to run `/mcp` and reconnect
  `workgraph` in those sessions, including the one that sent you.
- **Left behind:** worktree, branch or lock, if any step stopped early.
