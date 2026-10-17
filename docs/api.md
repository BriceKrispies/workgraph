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
| Tasks | `GET/POST /ws/{ws}/tasks`, `POST /ws/{ws}/tasks/bulk`, `GET /ws/{ws}/ready`, `GET /ws/{ws}/graph`, `GET/PATCH /ws/{ws}/tasks/{ref}`, `POST .../claim, release, complete, accept, reject, comments, progress, result, dependencies, relations, repositories, archive, commits, classification, classification/confirm, verify, decompose`, `DELETE .../dependencies/{dep}`, `DELETE .../repositories/{repo}`, `GET .../history, attempts, commits, changes, diff, change-sets, classification, similar, estimate, verification, checks, acceptances, schedulability` |
| Stacks | `GET/POST /ws/{ws}/stacks` (`include_inactive=true` for landed and abandoned), `GET /ws/{ws}/stacks/{ref}` (`ref` is the parent task) |
| Runs and attempts | `GET /ws/{ws}/attempts`, `POST /ws/{ws}/runs`, `GET /attempts/{id}`, `GET /attempts/{id}/manifests`, `GET /attempts/{id}/log`, `POST /attempts/{id}/cancel, retry, rerun`, `GET /attempts/compare?a=&b=` |
| Jobs | `GET /jobs`, `GET /jobs/{id}`, `POST /jobs/{id}/cancel, retry`, `GET /workers`, `GET /ws/{ws}/scheduler` |
| Verification | `GET/POST /ws/{ws}/verification/configs`, `POST /verification/configs/{id}/enable|disable`, `GET /ws/{ws}/verification/runs`, `GET /verification/runs/{id}`, `GET /ws/{ws}/verification/failures` |
| Automation | `GET /automation/catalog`, `GET/POST /ws/{ws}/automation/rules`, `GET/PUT /ws/{ws}/automation/rules/{ref}`, `POST /ws/{ws}/automation/rules/{ref}/enable|disable|trigger`, `POST /ws/{ws}/automation/preview`, `GET /ws/{ws}/automation/runs`, `GET /ws/{ws}/notifications`, `POST /notifications/{id}/read` |
| Analytics | `GET /analytics/metrics`, `POST /ws/{ws}/analytics/query`, `GET /ws/{ws}/analytics/overview`, `POST /ws/{ws}/analytics/compare`, `GET /ws/{ws}/analytics/coupling`, `GET /ws/{ws}/analytics/failures`, `GET /ws/{ws}/analytics/history`, `POST /ws/{ws}/analytics/estimate`, `POST /ws/{ws}/analytics/export`, `GET /ws/{ws}/reports`, `POST /ws/{ws}/reports/retrospective` |
| Search and classification | `GET /ws/{ws}/search`, `GET /ws/{ws}/classification/evaluations`, `GET /ws/{ws}/classification/disagreements` |
| Events | `GET /ws/{ws}/events`, `GET /ws/{ws}/events/stream` (SSE; `?token=`, `after_seq`, `max_events`) |

The SSE stream emits `domain` events (one per new domain event, with the event JSON) and periodic
`queue` snapshots (job counts by status). The web UI uses it to refresh live views.
