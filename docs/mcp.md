# MCP server

Workgraph implements an MCP server with the official Python SDK (`mcp` 2.x, `MCPServer`).

| Transport | How |
| --- | --- |
| stdio | `workgraph mcp` (one process per client; ideal for Claude Code and desktop clients) |
| Streamable HTTP | `workgraph serve` exposes `http://127.0.0.1:8765/mcp/`; or `workgraph mcp --transport http --port 8766` |

The HTTP endpoint requires `Authorization: Bearer <token>` (the token is in `config/api-token`,
see `workgraph config path`) and validates `Host`/`Origin` against DNS rebinding.

One daemon serves every client on a machine: `workgraph serve --ensure` starts it only when nothing
answers, and `workgraph serve --restart` replaces it after an upgrade (see [claude-code.md](claude-code.md)
and [cli.md](cli.md#the-daemon)). Over HTTP the agent identity comes from the `X-Workgraph-Agent` request
header (WG-5702): a call with `X-Workgraph-Agent: claude-code` runs as `agent:claude-code`, exactly as the
same call over stdio with `--agent-name claude-code` does. A missing header keeps the agent the server was
started with; a header that is not 1 to 64 characters of letters, digits, `-`, `_` or `.` is ignored.

## Identity and permissions

Every MCP call runs as an **agent** identity (`agent:<name>`, set with `--agent-name`) with the
default agent permissions: read everything, create/update tasks, claim, complete, report
progress/results, create attempts, link commits, request classification and trigger *manual*
automation rules. Agents cannot accept work, author automation rules, change trusted configuration
or start code execution unless the operator grants it:

```sh
workgraph mcp --agent-name ci-bot --grant run:start --grant repo:register
```

There is no tool for raw SQL or shell commands. Analytics are queried with metric ids, allowlisted
dimensions and typed filters.

## Tools

`workspace_list`, `workspace_get`, `project_list`, `project_get`, `repository_list`,
`repository_get`, `repository_register`, `repository_status`, `task_create`, `task_get`,
`task_search`, `task_list`, `task_update`, `task_claim`, `task_release`, `task_complete`,
`task_get_ready`, `task_promote_ready`, `task_import_breakdown`, `task_intake`, `task_add_dependency`, `task_remove_dependency`, `task_add_relation`,
`task_add_comment`, `task_get_history`, `task_get_attempts`, `task_link_repository`,
`task_link_commit`, `task_report_progress`, `task_report_result`, `attempt_create`, `attempt_start`, `attempt_submit`, `attempt_get`,
`attempt_list`, `run_start`, `run_get`, `run_cancel`, `git_get_changes`, `git_get_task_diff`,
`classification_get`, `classification_request`, `analytics_query`, `analytics_report`, `analytics_subagents`, `analytics_agent_time`, `analytics_bottlenecks`,
`automation_list`, `automation_trigger`, `stack_get`, `stack_list`, `stack_history`, `experiment_list`,
`experiment_get`, `experiment_report`, `experiment_predict`, `task_contract_check`, `task_request_decision`,
`decision_list`, `task_decisions`.

`task_request_decision(task, question, options)` asks the user a question the task waits on; `options` is a list of
`{id?, label, detail?, recommended?}` (empty means a free-text answer) and the recommended option should be marked.
It tags the task `needs-decision`. The user answers in the web UI or CLI; agents have no tool to answer.
`task_decisions(task)` returns the task's decisions with the chosen option or free-text answer and note once given,
and `decision_list(status, limit)` lists decisions (`open` by default) with their task. See [Decisions](api.md#decisions).

`task_create` with `ready: true` is triaged by `tasks.intake_proposed`. With intake on, the task stays `proposed` whatever `ready` says, and the reply's `warning` says so and how a human promotes it (`task_intake` with `admit` and `ready`). With intake off, the task lands `ready` when the promote-ready rule would move it (no unaccepted dependency, not tagged `parked`) and lands in `backlog` with a `warning` naming the reason otherwise.
`task_promote_ready` (with or without `dry_run`) lists the candidates it leaves out in `skipped` (a key-to-reason map),
present only when there is one.

`task_create` and `task_update` take `files` (a list of `{path, anchor?, note?}`; `task_update` also
takes `add_files`, and `files: []` clears them). `task_get` returns them as `files`, and `task_list`
takes `file` to filter by an exact path.

`task_update` takes `parent` (a key such as `WG-12` or a UUID) to set or change the task's parent, and
`clear_parent: true` to remove it; both together are refused. The parent is resolved in the task's own workspace
and checked with the same rules as `task_create`: a nonexistent or other-workspace parent is `not_found`, and a
parent that would close a cycle in the parent chain is refused with `dependency_cycle`, naming the task and
the parent that would close the loop. The change is recorded in the task's history as a `parent_id` change.

`task_update` takes `category` to set a task's category. It must be one of the classifier taxonomy's
categories (`bug_fix`, `feature`, `refactor`, `maintenance`, `infrastructure`, `performance`, `security`,
`testing`, `documentation`, `research`, `architecture`, `other`); any other value is refused with a validation
error that lists them. The CLI spelling is `workgraph task update REF --category CATEGORY`. The change is
recorded in the task's history as a `task.updated` change `category: [old, new]`.

`experiment_next` creates the next planned trial of an experiment, so it needs the `experiment:write` permission
(`workgraph mcp --grant experiment:write`), like `trial_classify`.

`experiment_fit` refits and stores the success predictor for a model (`experiment:write`). `experiment_predict` takes
either `task` (reads its `contract.predicted` features) or `repository`, `src_lines`, `src_files` and `packages`, and
returns the probability, interval, fit id and data cutoff, labelled as an inference (`task:read`).

`task_contract_check` (`task:read`) gives the leaf verdict for a task (`task`, `model`, optional `target`, default 0.8):
`not_delegable` with reasons, `incomplete` with the missing contract fields, `split`, or `leaf`. Its `prediction` is
the same inference as `experiment_predict`.

Stack mutations need the `stack:write` permission, which agents do not have by default: `stack_create`,
`stack_add_item`, `stack_remove_item`, `stack_integrate` and `stack_verify` are denied unless the server
is started with `workgraph mcp --grant stack:write`.

`trial_start`, `trial_grade` and `trial_classify` need the `experiment:write` permission (`workgraph mcp --grant experiment:write`), which agents
do not have by default. `trial_start` returns the trial's worktree, branch, base sha and brief; `trial_grade` takes an optional `timeout` in seconds. Experiment reads need `task:read`.

All tools have JSON-Schema input validation and structured output. List tools accept
`limit`/`offset` and return `next_offset`. Errors are returned as tool errors whose text is
`{"error": {"code": ..., "message": ..., "details": ...}}`.

## Resources

| URI | Content |
| --- | --- |
| `workgraph://workspaces` | all workspaces |
| `workgraph://workspaces/{workspace}/summary` | workspace summary |
| `workgraph://workspaces/{workspace}/projects/{project}` | project summary |
| `workgraph://workspaces/{workspace}/tasks/{task}` | task details |
| `workgraph://workspaces/{workspace}/tasks/{task}/history` | events and attempts |
| `workgraph://workspaces/{workspace}/analytics/{metric}` | a metric grouped by week |
| `workgraph://analytics/catalog` | metric definitions |

## Register with Claude Code

Claude Code connects to the one daemon over HTTP, with the bearer token and the `X-Workgraph-Agent:
claude-code` header:

```sh
workgraph claude install --mcp-http
```

`--mcp-http` runs `claude mcp remove workgraph -s user` and then
`claude mcp add --transport http -s user workgraph http://127.0.0.1:8765/mcp/ --header "Authorization: Bearer <token>" --header "X-Workgraph-Agent: claude-code"`.
It reads the token from `config/api-token`; start the daemon first with `workgraph serve --ensure`.

A stdio registration (one `workgraph mcp` process per session) remains available for clients that need it:

```sh
workgraph claude install --mcp
```

or manually:

```sh
claude mcp add --scope user workgraph -- workgraph mcp
```

(When Workgraph is not installed globally, use the absolute path printed by
`workgraph claude mcp-config`.)

## Register with another MCP client

Any standards-compliant client that launches stdio servers can use the block printed by
`workgraph claude mcp-config`, for example in a client's `mcpServers` settings:

```json
{
  "mcpServers": {
    "workgraph": { "type": "stdio", "command": "C:/Users/me/.local/bin/workgraph.exe", "args": ["mcp"] }
  }
}
```

HTTP-capable clients connect to `http://127.0.0.1:8765/mcp/` with the bearer token header.

## A typical agent session

1. `task_get_ready` - pick the highest-priority unblocked task.
2. `task_claim` - exclusive lease (default 120 minutes).
3. `attempt_create` - record the attempt (model, session id). It makes no worktree.
   `attempt_start` replaces steps 2 and 3 when the work should be isolated in managed git worktrees (the same
   ones a Workgraph-run agent gets, with the stack base for stack children). It claims a ready task (or
   continues your own claim), records the attempt, prepares one worktree per linked repository and moves the
   task and attempt to running. It returns `attempt_id`, `attempt_number`, `task_key`, `repositories` (each with
   `repository`, `checkout_path`, `branch`, `base_sha` and `base`) and `agent_checks`. Commit in those
   checkouts. Requires `task:claim` and `attempt:write`; a task in any other status is refused.
4. Work in the repository, committing with a `Task-ID: WG-12` trailer.
5. `task_report_progress` as work proceeds.
6. `task_link_commit` for anything not carrying the trailer.
7. `task_report_result` with `succeeded`/`failed` (and measured token usage if your runtime has it).
   The task moves to awaiting verification/review; a human or configured verification accepts it.

After `attempt_start`, finish with `attempt_submit` instead of step 7: it concludes the attempt from the
commits you made in the managed checkouts, exactly as a Workgraph-run attempt is concluded. It captures the
outcome and links the commits, enforces read-only repositories, runs the fast checks (or the full
verification) and moves the task to review, or fails the attempt and task when you pass `status: "failed"`
with an `error`. It returns `status`, `verification`, `outcome`, `task_status` and `attempt_status`. It requires
`task:claim`, `attempt:write` and `verify:run`, and stack children also `stack:write`. An attempt without a
managed worktree (from `attempt_create`) is refused.
