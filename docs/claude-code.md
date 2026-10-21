# Claude Code integration

Workgraph integrates with Claude Code in three independent ways:

1. **MCP** - Claude Code calls Workgraph tools (see [mcp.md](mcp.md)).
2. **Execution adapter** - Workgraph launches Claude Code for an attempt.
3. **Lifecycle hooks** - Claude Code reports session, tool, subagent and task events to Workgraph.

## Execution adapter

`agents.claude_code` (type `claude_code`) runs the documented programmatic interface:

```text
claude -p --output-format stream-json --verbose --permission-mode <mode> --session-id <uuid>
       [--model M] [--allowedTools ...] [--max-budget-usd N] [--add-dir <other repos>]
```

- The prompt (task snapshot, repositories, rules) is sent on stdin and stored as an artifact.
- Each stdout line is a JSON message (`system`/`assistant`/`user`/`result`); no terminal output
  is scraped. Tool uses, tool errors, subagent launches, turns, the session id and the final
  `result` are recorded.
- Usage and cost come only from the `result` message and are labelled `claude_code_reported`.
- Workgraph chooses the session id up front and sets `WORKGRAPH_ATTEMPT_ID` and `WORKGRAPH_TASK_ID`
  in the environment, so hooks fired by that session correlate with the attempt automatically.
- Timeouts and cancellation terminate the whole process tree.
- If `claude` is missing, starting a run fails immediately with an actionable error and no
  state changes.

## Lifecycle hooks

```sh
uv run workgraph claude install
uv run workgraph claude status
uv run workgraph claude uninstall
```

`install` merges handlers for `SessionStart`, `SessionEnd`, `PreToolUse`, `PostToolUse`,
`PostToolUseFailure`, `SubagentStart`, `SubagentStop`, `TaskCreated`, `TaskCompleted` and `Stop`
into `~/.claude/settings.json` (or `--scope project|local --project-dir DIR`). It:

- parses the existing JSON and refuses to touch an unparsable file,
- writes a timestamped backup before changing anything, then writes atomically,
- adds handlers in exec form (`command` = the workgraph executable, `args = ["claude", "hook"]`)
  with `"async": true`, so hooks never block Claude Code,
- is idempotent, and `uninstall` removes only Workgraph-owned handlers.

Hooks are **passive**: the handler prints nothing and always exits 0, so it cannot block tools,
change permissions or add context.

### Ingestion

`workgraph claude hook` reads the hook JSON from stdin through a minimal entry point (no web,
analytics or MCP imports). It:

- validates the payload leniently (unknown fields are tolerated; a `schema_version` of 1 or 2 is
  recorded from the fields present),
- redacts secrets and, unless `claude.capture_tool_payloads` is enabled, stores only the *shape*
  of tool inputs/outputs plus safe summaries (file path, command head, MCP server),
- is idempotent (a stable `dedupe_key` per received payload),
- writes the redacted record atomically to the spool directory if the database is busy,
  missing or on another schema, to be replayed by the worker or `workgraph claude replay`.

### Correlation

Claude Code's session ids, subagent ids and internal task ids are mapped onto Workgraph records
but never become authoritative:

| Situation | Binding |
| --- | --- |
| Session launched by Workgraph | `WORKGRAPH_ATTEMPT_ID` in the hook environment -> attempt and task (`workgraph_launched`) |
| External session, bound explicitly | `workgraph claude bind WG-12 --session <id>` or `--cwd DIR` (`explicit_binding`) |
| External session with `WORKGRAPH_TASK_ID` set | task only (`environment`) |
| Anything else | recorded as `unbound` and bound later if a binding appears |

Subagents of a bound session become nested attempts (`parent_attempt_id`) that finish on
`SubagentStop`. `TaskCreated`/`TaskCompleted` populate `agent_internal_tasks`; one is linked to a
Workgraph task only if its subject names a task key explicitly.

### Transcript telemetry

Claude Code writes each subagent's transcript (JSONL) and an `agent-<id>.meta.json` sidecar. On
`SubagentStop` Workgraph reads both (`integrations/claude_code/transcript.py`, stdlib only) and
records what actually ran, which the hook payload does not carry:

- the model that ran (`subagent_runs.model`) and the requested alias (`requested_model`, from the sidecar),
- token usage summed per API message, turns, tool calls per name, tool errors and prompt length,
- a `Task-ID: <KEY>` marker in the first prompt links the run to that task (`subagent_runs.task_id`),
  but only when exactly one task in scope has that key; `Trial-ID:` markers are recorded as well.

Dedupe: one API message is split over several lines sharing `message.id`. Usage comes from the line
with the largest `output_tokens` per message id (ties: the last line); tool uses are counted once per
`tool_use` id; `<synthetic>` messages are ignored.

If the run has a child attempt, that attempt gets the model and usage (`usage_source =
claude_code_transcript`). If the run has no attempt but a task was linked, the run binds to that task's most recent attempt
that is still open (`queued`, `preparing` or `running`) and has no subagent run yet. The attempt gets the
model, usage (`usage_source = claude_code_transcript`) and telemetry, and an `attempt.subagent_bound`
event is recorded. Its status is left alone, because the orchestrator finishes it. Only when no such
attempt exists is a finished top-level attempt created for the task (adapter `claude_code_subagent`).
Finished attempts are never reused. A missing or unreadable transcript leaves the run unchanged.

The transcript's timestamps also give the run's wall-time breakdown (`telemetry.time`: seconds and calls per
tool class, model and waiting time, and the ten slowest calls). Runs stored before this existed get it from
`workgraph claude backfill-time`, which fills only that key.

Transcripts that were not readable when the hook fired (or that predate this) can be applied later:

```sh
uv run workgraph claude backfill-transcripts [--limit N]
```

It processes runs with a transcript path and no model, prints `parsed`, `missing`, `linked` and
`attempts_created`, and is idempotent.

`uv run workgraph claude backfill-time [--limit N]` stores the wall-time breakdown (`telemetry.time`) on
runs that lack it, reading the same transcripts. It writes nothing else and is idempotent.
