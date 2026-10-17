# Extending Workgraph

Workgraph defines interfaces only at real subsystem boundaries.

## A new agent provider

Implement `AgentProvider` (`src/workgraph/integrations/agents/base.py`):

```python
class MyProvider:
    name = "my-agent"
    adapter_type = "my_agent"

    def capabilities(self) -> AgentCapabilities: ...
    def check_available(self) -> tuple[bool, str]: ...
    def config_snapshot(self) -> dict[str, Any]: ...  # non-secret, stored on each attempt
    def run(
        self, request: AgentRunRequest, on_event: EventSink, should_cancel: CancelCheck
    ) -> AgentRunResult: ...
```

Register a config type and construction in `integrations/agents/registry.py`. Use
`integrations/process.run_process` for subprocesses (timeouts, cancellation, tree kill, log
capture). Report usage/cost only when the agent measured it. Often the generic command adapter
([agent-protocol.md](agent-protocol.md)) is enough and needs no code.

## A new inference provider

Implement `LLMProvider` (`integrations/llm/base.py`: `capabilities`, `list_models`,
`check_model`, `generate_json`, `embed`) and either add a config type in `config/settings.py`
plus a branch in `integrations/llm/registry.build`, or register a factory at runtime in
`integrations/llm/registry.EXTRA_FACTORIES`. Raise `ProviderUnavailableError`/`ModelUnavailableError`
for connectivity problems (jobs retry) and `InvalidModelOutputError` for unusable output.

## A new metric

Add a `Metric` to `analytics/metrics.py` (id, family, definition, unit, statistic kind, base
relation, time column, value/where expressions) and run `uv run tools/gen_metric_docs.py`. If it
needs new data, extend the warehouse fact tables in `analytics/warehouse.py`. Metrics are only
ever composed from the registry and validated parameters.

## A new automation action or trigger

Add a Pydantic action model to `automation/rules.py` (include it in the `Action` union), implement
it in `AutomationService._execute_action` and add it to the catalog set; new trigger events go in
`TRIGGER_EVENTS` and must be emitted by a service with `record_event`.

## A new language for structural analysis

Add the grammar package, a loader in `analytics/code/structure._loaders`, node-type mappings in
`SYMBOL_NODES` and the file extension in `analytics/code/classify.EXTENSION_LANGUAGES`.

## A new MCP tool or API endpoint

Add a method to the relevant application service (with `require(...)` and `record_event(...)`),
then expose it in `mcp/server.py` (via the `tool` helper) and/or `api/routes/*.py`, and a CLI
command in `cli/commands/*.py`. Interfaces must not contain business logic.

## A schema change

Edit the ORM in `persistence/orm/`, generate a migration (see [testing.md](testing.md)), review it
(FTS shadow tables are excluded from autogenerate), update `persistence/schema_version.SCHEMA_HEAD`
and add a test that upgrades a populated database.

## A new event schema version

Bump `EVENT_SCHEMA_VERSIONS[event_type]` in `core/events/log.py` and register an upcaster from the
previous version; readers always see the current shape and stored events are never rewritten.
