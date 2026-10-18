# MCP server

Workgraph implements an MCP server with the official Python SDK (`mcp` 2.x, `MCPServer`).

| Transport | How |
| --- | --- |
| stdio | `workgraph mcp` (one process per client; ideal for Claude Code and desktop clients) |
| Streamable HTTP | `workgraph serve` exposes `http://127.0.0.1:8765/mcp/`; or `workgraph mcp --transport http --port 8766` |

The HTTP endpoint requires `Authorization: Bearer <token>` (the token is in `config/api-token`,
see `workgraph config path`) and validates `Host`/`Origin` against DNS rebinding.

## Identity and permissions

Every MCP call runs as an **agent** identity (`agent:<name>`, set with `--agent-name`) with the
default agent permissions: read everything, create/update tasks, claim, complete, report
progress/results, create attempts, link commits, request classification and trigger *manual*
automation rules. Agents cannot accept work, author automation rules, change trusted configuration
or start code execution unless the operator grants it:

```sh
uv run workgraph mcp --agent-name ci-bot --grant run:start --grant repo:register
```

There is no tool for raw SQL or shell commands. Analytics are queried with metric ids, allowlisted
dimensions and typed filters.

## Tools

`workspace_list`, `workspace_get`, `project_list`, `project_get`, `repository_list`,
`repository_get`, `repository_register`, `repository_status`, `task_create`, `task_get`,
`task_search`, `task_list`, `task_update`, `task_claim`, `task_release`, `task_complete`,
`task_get_ready`, `task_add_dependency`, `task_remove_dependency`, `task_add_relation`,
`task_add_comment`, `task_get_history`, `task_get_attempts`, `task_link_repository`,
`task_link_commit`, `task_report_progress`, `task_report_result`, `attempt_create`, `attempt_get`,
`attempt_list`, `run_start`, `run_get`, `run_cancel`, `git_get_changes`, `git_get_task_diff`,
`classification_get`, `classification_request`, `analytics_query`, `analytics_report`,
`automation_list`, `automation_trigger`, `stack_get`, `stack_list`, `stack_history`, `experiment_list`,
`experiment_get`, `experiment_report`.

`experiment_next` creates the next planned trial of an experiment, so it needs the `experiment:write` permission
(`workgraph mcp --grant experiment:write`), like `trial_classify`.

Stack mutations need the `stack:write` permission, which agents do not have by default: `stack_create`,
`stack_add_item`, `stack_remove_item`, `stack_integrate` and `stack_verify` are denied unless the server
is started with `workgraph mcp --grant stack:write`.

`trial_classify` needs the `experiment:write` permission (`workgraph mcp --grant experiment:write`), which agents
do not have by default. Experiment reads need `task:read`.

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

```sh
uv run workgraph claude install --mcp
```

or manually:

```sh
claude mcp add --scope user workgraph -- workgraph mcp
```

(When Workgraph is not installed globally, use the absolute path printed by
`uv run workgraph claude mcp-config`.)

## Register with another MCP client

Any standards-compliant client that launches stdio servers can use the block printed by
`uv run workgraph claude mcp-config`, for example in a client's `mcpServers` settings:

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
3. `attempt_create` - record the attempt (model, session id).
4. Work in the repository, committing with a `Task-ID: WG-12` trailer.
5. `task_report_progress` as work proceeds.
6. `task_link_commit` for anything not carrying the trailer.
7. `task_report_result` with `succeeded`/`failed` (and measured token usage if your runtime has it).
   The task moves to awaiting verification/review; a human or configured verification accepts it.
