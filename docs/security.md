# Security boundaries

Workgraph treats task descriptions, comments, Git content (messages, trailers, file contents),
agent output and hook payloads as **untrusted input**.

## Identities and authorization

Every service call carries a `RequestContext` with an actor (`human:`, `agent:`, `system:`,
`automation:`) and a permission set; services call `require(...)` before mutating
(`security/permissions.py`).

| Capability | Humans (CLI, local UI) | Agents (MCP default) | Automation |
| --- | --- | --- | --- |
| Read tasks, repos, analytics | yes | yes | yes |
| Create/update/claim/complete tasks, report progress/results | yes | yes | create/update |
| Link commits, create attempts, request classification | yes | yes | classification |
| Trigger manual automation rules | yes | yes | - |
| Accept / reject work | yes | **no** | no |
| Register repositories | yes | grant `repo:register` | no |
| Create stacks, add/remove children, set stack item state | yes | grant `stack:write` (reads need `task:read`) | no |
| Start agent code execution | yes, if `execution.enabled` | grant `run:start` and `execution.enabled` | only rules authored by a human with `run:start` |
| Author automation rules, verification commands, agent config | yes | **no** | no |

Agent execution is **opt-in** (`execution.enabled = false` by default). Push, merge, publish and
destructive cleanup are never performed automatically (`allow_push`/`allow_merge` are false and no
code path pushes or merges). Task tracking and Git analysis work without granting any execution
privilege.

## Commands and processes

- All subprocesses use argument vectors (`shell=False`); executables are resolved explicitly.
- Agent commands and verification checks come only from trusted configuration or
  `config:manage`-authorised records - never from task text, agent output or automation templates.
- Argument templates for command agents allow a fixed set of placeholders, each substituted into
  a single argument.
- Verification checks refuse shell interpreters as the executable unless `allow_shell` is set by
  the operator.
- Child processes of command agents and checks get a minimal environment; secrets are not
  inherited (the Claude Code adapter inherits the user environment because Claude Code needs its
  own credentials).
- Timeouts and cancellation kill whole process trees (`taskkill /T` on Windows, process groups
  on POSIX). Output is bounded in memory and streamed to log artifacts.

## Filesystem

- Repository paths must exist, be Git working trees and (optionally) lie under the workspace's
  `allowed_roots`; Workgraph's own worktree/run/artifact directories cannot be registered.
- Workgraph only creates, modifies or removes worktrees it created under its data directory;
  user checkouts are never reset or cleaned. Branches are never deleted.
- Artifact and archive paths are validated against their root (no traversal); backup archives
  are checked for unsafe member paths before extraction.
- Writes of configuration, spool records and other important files are atomic (temp + rename).

## HTTP API and Streamable HTTP MCP

- Binds to `127.0.0.1` by default; non-loopback binding requires both configuration and
  `--allow-remote`.
- Every `/api` and `/mcp` request requires the local bearer token (constant-time comparison).
  The bundled UI receives it embedded in `index.html`; no CORS headers are ever sent, so other
  origins cannot read it.
- `Host` must be an allowed host (DNS-rebinding protection); state-changing requests with a
  foreign `Origin` are rejected. The MCP transport additionally enables the SDK's DNS-rebinding
  protection.
- Responses set `X-Content-Type-Options`, `X-Frame-Options: DENY`, `Referrer-Policy` and
  `Cache-Control: no-store` for API data.
- There is no endpoint or tool for raw SQL or arbitrary shell execution. Analytics accept only
  registered metric ids, allowlisted dimensions and typed filters; FTS queries are tokenised and
  quoted to prevent operator injection.

## Data handling

- Hook payloads are redacted (secret-looking keys and values: API keys, tokens, bearer headers,
  private keys, JWTs, `password=` pairs). Tool inputs/outputs are stored as shapes only unless
  `claude.capture_tool_payloads = true`; prompts are stored as lengths.
- Credentials are read from environment variables, never stored in configuration or the
  database. The API token file is created with user-only permissions where supported.
- The event log is append-only (database triggers). Retention compacts only verbose telemetry
  payloads and log files; records are never silently deleted.
- Automation templates interpolate values as text; rule logic and actions are fixed by the
  human-authored rule. Recursion is bounded by depth and rate limits.
- Agent prompts mark task descriptions as untrusted work requests that cannot override the run
  rules.
