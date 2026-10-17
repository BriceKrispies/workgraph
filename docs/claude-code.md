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
