# Automation rules

Rules are declarative JSON documents persisted in the database. Only actors with
`automation:manage` can create or edit them; agents may trigger rules whose trigger type is
`manual`.

```json
{
  "name": "triage critical bugs",
  "description": "tag and notify on critical bug reports",
  "enabled": true,
  "trigger": {"type": "event", "event_types": ["task.created"]},
  "conditions": {"all": [
    {"field": "task.priority", "op": "eq", "value": "critical"},
    {"any": [
      {"field": "task.title", "op": "matches", "value": "(?i)crash|bug"},
      {"field": "classification.category", "op": "eq", "value": "bug_fix"}
    ]}
  ]},
  "actions": [
    {"type": "add_tags", "tags": ["triage"]},
    {"type": "notify", "level": "warning", "title": "Critical task {{task.key}}: {{task.title}}"}
  ],
  "retry_max_attempts": 3,
  "max_depth": 3
}
```

```sh
uv run workgraph automation create @rules/triage.json
uv run workgraph automation preview --rule "triage critical bugs" --task WG-12
uv run workgraph automation runs --rule "triage critical bugs"
```

## Triggers

| Type | Fields |
| --- | --- |
| `event` | `event_types`: `task.created`, `task.updated`, `task.became_ready`, `task.completed`, `task.accepted`, `task.status_changed`, `attempt.failed`, `verification.completed`, `git.commit_ingested`, `classification.completed`, `repository.changes_detected` |
| `schedule` | `cron` (5-field, UTC) **or** `interval_seconds` (≥ 60) |
| `manual` | triggered with `automation trigger` / `automation_trigger`, optional inputs (e.g. `task`) |

## Conditions

Deterministic predicates over a context containing `event` (type, actor, entity, data), `task`,
`classification`, `attempt`, `input` and `now`. Operators: `eq`, `ne`, `in`, `not_in`, `contains`,
`not_contains`, `gt`, `gte`, `lt`, `lte`, `exists`, `not_exists`, `matches` (regex), `starts_with`;
combinators `all`, `any`, `not`. Every evaluation stores a per-predicate trace.

## Actions

| Type | Effect |
| --- | --- |
| `classify_task` | queue classification of the trigger task |
| `add_tags` | add tags |
| `update_task` | priority, status (backlog/ready/blocked/cancelled), assignee, agent, auto_execute |
| `create_child_tasks` | children (templated titles), optional sequential dependencies, inherited repositories |
| `create_dependency` | `depends_on` or `blocks` another task |
| `enqueue_agent_run` | privileged: needs execution enabled and an author holding `run:start` |
| `run_verification` | queue a verification job |
| `generate_report` | queue a retrospective |
| `notify` | local notification (UI and `automation notifications`) |

`{{path}}` templates interpolate context values as data only; task text can never become rule
logic or commands.

## Execution, idempotency and safety

- The maintenance worker reads new events after a persisted cursor; each (rule, trigger) pair
  has a unique `trigger_key`, so reprocessing never duplicates runs.
- Matched runs execute in an `automation_action` job with the rule's retry policy; actions that
  already succeeded are skipped on retry; exhausted retries dead-letter the job.
- Events produced by automation carry their depth; runs beyond `max_depth` (and the global
  `automation.max_depth`) are recorded as `skipped` with the recursion-guard reason. A per-rule
  rate limit (30 runs/minute) stops loops that stay within depth.
- Every run records the trigger, depth, matched flag, reason, condition trace, per-action results
  and errors - the history of which rule did what and why.
- `preview` (dry run) evaluates a stored or unsaved rule against an event or inputs and lists the
  planned actions without executing them.
