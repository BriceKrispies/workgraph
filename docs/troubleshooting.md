# Troubleshooting

Start with:

```sh
uv run workgraph doctor
uv run workgraph doctor --verify-events
```

| Symptom | Cause and fix |
| --- | --- |
| `no default workspace configured` | `uv run workgraph workspace use <slug>` or pass `-w <slug>`. |
| `agent code execution is disabled` | Execution is opt-in: `uv run workgraph config set execution.enabled true`. |
| `agent adapter 'claude_code' is unavailable` | Install Claude Code or set `agents.claude_code.executable` to its absolute path. Nothing changed in the task. |
| Runs stay `queued` | No worker is running. Use `workgraph serve` (starts one) or `workgraph worker run`. Check `workgraph worker list` and resource limits (`execution.max_concurrent_runs`, `jobs.per_agent_concurrency`). |
| Classification jobs end in `dead` with `provider_unavailable` | Ollama is not running or the model is not pulled (`ollama pull <model>`). Retry with `workgraph job retry <id>`, or switch to the heuristic provider. Tracking is unaffected. |
| Task stays `blocked` | A dependency is not *accepted* yet (completion is not enough). `workgraph task show <key>` lists blockers. |
| `cannot move task ... to accepted` | Use `workgraph task accept` (after verification) or `--manual --reason ...`. |
| `version_conflict` | Someone changed the task; re-read it and retry with the new `expected_version`. |
| `resource_busy` | Another attempt holds an in-place checkout lease or claim. Wait, or release it. |
| Verification `error` with "repository ... not part of the revision manifest" | The check names a repository not linked to the task; link it (`task link-repo`) or scope the config differently. |
| Commits not attributed | Use a `Task-ID: KEY` trailer, run `workgraph git sync`, or link explicitly with `workgraph git link KEY SHA`. Inferred links (task key in the message) have confidence 0.5 and are labelled as inferred. |
| History rewritten (rebase/force-push) | `workgraph git sync <repo> --full` reconciles reachability and carries links to rewritten commits. |
| Final change surface missing | It is computed after acceptance by a background job; `workgraph git surface KEY` computes it now. |
| Hooks not recorded | `workgraph claude status` shows installation and ingestion counts; `workgraph claude replay` ingests spooled events; inspect `<data>/spool/claude-hooks/*.failed`. |
| Web UI shows "API is running" | The UI is not built: `pnpm --dir web install` then `pnpm --dir web build`. |
| `401 unauthorized` from scripts | Send `Authorization: Bearer $(cat <config>/api-token)` (path from `workgraph config path`). |
| `421` / `403` from the API | Host or Origin not allowed. Use `http://127.0.0.1:<port>`; for development set `WORKGRAPH_ALLOWED_ORIGINS`. |
| `database is locked` warnings | Another process held a write transaction longer than 15 s; check for stuck external tools. Workgraph never holds write transactions during external calls. |
| Worktree left behind after a crash | `workgraph worker recover` then `workgraph git cleanup-worktrees --apply` (only clean, Workgraph-owned worktrees are removed; branches are kept). |

Logs are JSON lines in the log directory (`workgraph config path`) when running `serve` or
`worker run`.
