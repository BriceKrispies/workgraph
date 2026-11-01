# Extending Workgraph

Workgraph defines interfaces only at real subsystem boundaries. The rule for every change: decisions
go in a Core or Pure crate as functions over values; I/O goes in a shell crate; interfaces only
parse and render. `cargo purity` and `cargo lint-arch` enforce the split ([architecture.md](architecture.md)).

## A new agent provider

Agent providers live in `wg-agentspec` (types and decisions: capabilities, availability, the
config snapshot stored on each attempt, request and result shapes) and `wg-agents` (the shell that
spawns the agent). Add the provider's data and argument decisions to `wg-agentspec`, and its
process handling to `wg-agents`, using `wg-proc` for subprocesses (timeouts, cancellation, tree
kill, log capture). Report usage and cost only when the agent measured it. Often the generic
command adapter ([agent-protocol.md](agent-protocol.md)) is enough and needs no code.

## A new inference provider

Providers are in `wg-intel` (decisions: prompts, parsing, the heuristic provider) and its feature
crate `wg-intel-feature` (config and HTTP calls). Add a config type, then a branch where providers
are built. Report connectivity problems as provider or model unavailable (jobs retry) and unusable
output as invalid model output.

## A new metric

Add a metric definition (id, family, definition, unit, statistic kind, base relation, time column,
value and where expressions) to the registry in `wg-analytics` (`core/metrics.rs`). If it needs new
data, extend the fact tables in `wg-analytics`. Metrics are only composed from the registry and
validated parameters. Regenerate the metric reference in [analytics.md](analytics.md) by hand to
match.

## A new automation action or trigger

Add the action to the rule spec in `wg-automation` (`spec.rs`) and its execution in
`wg-automation-feature`; new trigger events go in the trigger validation in `rules.rs` and must be
emitted by a decision that records the event.

## A new language for structural analysis

Add the tree-sitter grammar dependency to `wg-codeparse`, its loader and node-type mappings, and the
file extension mapping in `wg-codeanalysis`.

## A new MCP tool or API endpoint

Put the decision in the domain's Core crate, then expose it in the domain's feature crate: an MCP
tool, an HTTP route and a CLI command, each with its pure arguments and view code in `spec/`.
`contracts/` holds snapshots of the CLI tree, OpenAPI document and MCP descriptors; update the
snapshot with the change so the contract tests keep matching.

## A schema change

Add a migration to `wg-store`, the matching row codec in `wg-rows`, and the column in
`wg-archivespec`'s layout if exports include it. Add a store test that upgrades a populated
database from the previous revision.

## A new event schema version

Bump the event type's schema version in `wg-events` and register an upcaster from the previous
version. Readers always see the current shape; stored events are never rewritten.
