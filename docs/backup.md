# Backup and recovery

## Backups

```sh
uv run workgraph backup
uv run workgraph backup --output D:/backups/workgraph.zip --include-artifacts
```

Backups use SQLite's online backup API, so they are consistent while Workgraph is running. The zip
contains `workgraph.db`, `config/config.toml`, `config/workspaces/*.toml`, optionally `artifacts/`,
and a `manifest.json` with the schema revision and Workgraph version. Default location:
`<data>/backups/`.

## Restore

```sh
uv run workgraph restore D:/backups/workgraph.zip
uv run workgraph restore D:/backups/workgraph.zip --with-config
```

Restore validates the archive and the database's integrity, refuses to run while a worker is
heartbeating (use `--force` if you are sure), saves the current database as
`<data>/backups/pre-restore-<timestamp>.db`, replaces the database atomically and migrates it
forward to the current schema. Configuration files are restored only with `--with-config`.

## Export and import

```sh
uv run workgraph export workspace acme.jsonl -w acme
uv run workgraph --home ./other import acme.jsonl
```

Workspace exports contain every record of one workspace (tasks, events, attempts, commits,
classifications, analytics inputs, …) as JSON Lines in foreign-key order. Import refuses to touch
an existing workspace with the same id and rebuilds the search index. Analytics data can be
exported as CSV, JSON or Parquet (`workgraph analytics export`).

## Crash recovery

| Component | Recovery |
| --- | --- |
| Database | WAL mode; transactions are atomic. `workgraph doctor` runs `PRAGMA integrity_check`. |
| Jobs | A crashed worker stops heartbeating; its jobs' leases expire and are re-queued (or dead-lettered) by the next maintenance pass or `workgraph worker recover`. |
| Workers | Workers without a heartbeat for 2 minutes are marked `lost`. |
| Worktrees | Every worker start and `worker recover` reconcile `managed_worktrees` with disk: missing ones become `orphaned`, finished attempts' worktrees `released`, interrupted preparations `active`. Unknown directories are reported, never deleted. |
| Claude Code hooks | Payloads that cannot be ingested are spooled and replayed; a record failing five times is set aside as `.failed` for inspection. |
| Event log | `workgraph events verify` replays task events against current state. |

## Retention

`retention.telemetry_payload_days` and `retention.run_log_days` bound how long raw hook payloads
and run/verification log files are kept. Compaction runs daily in the maintenance worker or on
demand (`workgraph job compact --apply`). Purged items keep their metadata and a `purged_at`
timestamp; task, attempt and event records are never deleted.
