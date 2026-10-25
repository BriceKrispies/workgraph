# HTTP API

`workgraph serve` exposes the API under `/api/v1` (OpenAPI at `/api/docs` and `/api/openapi.json`),
Server-Sent Events, the Streamable HTTP MCP endpoint at `/mcp/` and the web UI at `/`.

Authentication: `Authorization: Bearer <token>` (token in `config/api-token`). `GET /api/v1/health`
is public. Errors: `{"error": {"code", "message", "details"}}` with HTTP status 401, 403, 404,
409, 421, 422 or 503.

```sh
curl -H "Authorization: Bearer <token>" http://127.0.0.1:8765/api/v1/ws/acme/tasks?status=ready
```

| Area | Endpoints |
| --- | --- |
| Meta | `GET /health`, `GET /meta`, `GET /settings`, `PATCH /settings`, `GET /agents`, `GET /llm/health`, `GET /claude/status` |
| Workspaces | `GET/POST /workspaces`, `GET /workspaces/{ws}` |
| Projects | `GET/POST /ws/{ws}/projects`, `GET/PATCH /ws/{ws}/projects/{ref}`, `POST /ws/{ws}/projects/{ref}/repositories`, `DELETE /ws/{ws}/projects/{ref}/repositories/{repo}` |
| Repositories | `GET/POST /ws/{ws}/repositories`, `GET/PATCH/DELETE /ws/{ws}/repositories/{ref}`, `POST .../refresh`, `GET .../status`, `POST .../sync`, `GET .../commits`, `GET .../commits/{sha}`, `GET /ws/{ws}/worktrees` |
| Tasks | `GET/POST /ws/{ws}/tasks`, `POST /ws/{ws}/tasks/bulk`, `GET /ws/{ws}/ready`, `GET /ws/{ws}/graph` (`project`, `repository`, `root`, `include_closed`), `GET/PATCH /ws/{ws}/tasks/{ref}`, `POST .../claim, release, complete, accept, reject, comments, progress, result, dependencies, relations, repositories, archive, commits, classification, classification/confirm, verify` (optional `tier=fast|full`), `decompose`, `DELETE .../dependencies/{dep}`, `DELETE .../repositories/{repo}`, `GET .../history, attempts, commits, changes, diff, change-sets, classification, similar, estimate, verification, checks, acceptances, schedulability` |
| Decisions | `GET /ws/{ws}/decisions` (`status=open\|decided\|withdrawn\|all`, default `open`; `limit`, default 200), `GET /ws/{ws}/decisions/{id}`, `GET/POST /ws/{ws}/tasks/{ref}/decisions` (`201` on create), `POST /ws/{ws}/decisions/{id}/choose`, `POST /ws/{ws}/decisions/{id}/withdraw`. See [Decisions](#decisions) |
| Stacks | `GET/POST /ws/{ws}/stacks` (`include_inactive=true` for landed and abandoned), `GET /ws/{ws}/stacks/{ref}`, `POST /ws/{ws}/stacks/{ref}/items` (body `{"task": ref}`, adds a child; `201`), `DELETE /ws/{ws}/stacks/{ref}/items/{task}` (removes a child), `GET /ws/{ws}/stacks/{ref}/history` (the stack's events in order, with the verification runs they name), `POST /ws/{ws}/stacks/{ref}/integrate` (queues integration; `202` with one job per stack repository), `POST /ws/{ws}/stacks/{ref}/verify` (queues full verification of the integration tip; `202` with the verification job), and `GET /ws/{ws}/tasks/{ref}/stacks` (the stacks a task is an item of). `ref` is the parent task for the stack routes |
| Runs and attempts | `GET /ws/{ws}/attempts`, `POST /ws/{ws}/runs`, `GET /attempts/{id}`, `GET /attempts/{id}/manifests`, `GET /attempts/{id}/log`, `POST /attempts/{id}/cancel, retry, rerun`, `GET /attempts/compare?a=&b=` |
| Jobs | `GET /jobs`, `GET /jobs/{id}`, `POST /jobs/{id}/cancel, retry`, `GET /workers`, `GET /ws/{ws}/scheduler` |
| Verification | `GET/POST /ws/{ws}/verification/configs`, `POST /verification/configs/{id}/enable|disable`, `GET /ws/{ws}/verification/runs`, `GET /verification/runs/{id}`, `GET /ws/{ws}/verification/failures` |
| Automation | `GET /automation/catalog`, `GET/POST /ws/{ws}/automation/rules`, `GET/PUT /ws/{ws}/automation/rules/{ref}`, `POST /ws/{ws}/automation/rules/{ref}/enable|disable|trigger`, `POST /ws/{ws}/automation/preview`, `GET /ws/{ws}/automation/runs`, `GET /ws/{ws}/notifications`, `POST /notifications/{id}/read` |
| Analytics | `GET /analytics/metrics`, `POST /ws/{ws}/analytics/query`, `GET /ws/{ws}/analytics/overview`, `POST /ws/{ws}/analytics/compare`, `GET /ws/{ws}/analytics/coupling`, `GET /ws/{ws}/analytics/failures`, `GET /ws/{ws}/analytics/history`, `POST /ws/{ws}/analytics/estimate`, `POST /ws/{ws}/analytics/export`, `GET /ws/{ws}/reports`, `POST /ws/{ws}/reports/retrospective` |
| Search and classification | `GET /ws/{ws}/search`, `GET /ws/{ws}/classification/evaluations`, `GET /ws/{ws}/classification/disagreements` |
| Events | `GET /ws/{ws}/events`, `GET /ws/{ws}/events/stream` (SSE; `?token=`, `after_seq`, `max_events`) |

The SSE stream emits `domain` events (one per new domain event, with the event JSON) and periodic
`queue` snapshots (job counts by status). The web UI uses it to refresh live views.

## Decisions

A decision is a question a task waits on, with the options on offer. Every response below is a `DecisionOut`:

```json
{"id": "...", "task_id": "...", "task_key": "WG-12", "question": "Which store?",
 "options": [{"id": "a", "label": "SQLite", "detail": "Zero setup", "recommended": true}],
 "status": "open", "chosen_option_id": null, "other_text": null, "note": null,
 "decided_by": null, "decided_at": null, "created_by": "human:me",
 "created_at": "...", "updated_at": "...", "version": 1}
```

- `GET /ws/{ws}/decisions?status=open&limit=200` returns `{"items": [...]}`; each item is a `DecisionOut`
  plus `"task": {key, title, status, priority, tags, description}`. Open decisions come by task priority,
  then oldest first, and leave out archived and cancelled tasks; decided ones by `decided_at`, newest first.
- `GET /ws/{ws}/tasks/{ref}/decisions` returns `{"items": [DecisionOut]}`, oldest first.
- `POST /ws/{ws}/tasks/{ref}/decisions` takes `{question, options: [{id?, label, detail?, recommended?}]}`
  and returns `201`. An omitted `id` becomes "a", "b", ... by position; `options: []` asks for free text.
- `POST /ws/{ws}/decisions/{id}/choose` takes `{option_id?, other_text?, note?}`: exactly one of
  `option_id` and `other_text`, and `option_id` must name an option. Otherwise `422 validation_failed`.
  An answered decision may be chosen again. The answer is also written as the task comment
  `User decision YYYY-MM-DD: <question> -> <answer>`.
- `POST /ws/{ws}/decisions/{id}/withdraw` takes `{reason?}` and only applies to an open decision.

Answering or withdrawing a task's last open decision removes its `needs-decision`, `parked` and
`triage:needs-decision` tags. Each change emits a `task.decision_*` event on the task.
