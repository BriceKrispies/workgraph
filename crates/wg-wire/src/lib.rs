//! wg-wire: DTOs, error envelope, MCP descriptors, OpenAPI document.
//!
//! This crate is the wire side of the contract: what the HTTP API, the MCP server and the CLI
//! `--json` flag put on the wire, as values for `wg-json` to print.
//!
//! * [`error`] is the error envelope (O02, O06, O09): `{"error": {"code", "message", "details"}}`.
//! * [`convention`] holds the helpers that fix the DTO conventions.
//! * [`dto`] holds the DTOs, one module per area, each with its Pydantic field order.
//! * [`contract`] embeds the OpenAPI and MCP snapshots from `contracts/`.
//!
//! # DTO conventions
//!
//! A DTO mirrors one Pydantic model of `src/workgraph/core/models/schemas.py`.
//!
//! * Fields are listed in declaration order of the Python class. The OpenAPI snapshot sorts
//!   `properties` by name, so only `required` keeps Python's order; the DTOs are therefore
//!   checked against the snapshot's field names and against the Python source for the order.
//! * `to_value` returns a [`wg_json::Value::Model`], so the CLI and HTTP dialects print it as
//!   `model_dump(mode="json")` does: every field is present, an absent optional is `null`
//!   (never omitted), and `Value::DateTime` fields print in the `Z` form.
//! * A timestamp field names its format explicitly with [`convention::TimeFormat`], because the
//!   same instant prints as `...Z` under a model and as `...+00:00` where Python calls
//!   `isoformat()` on a bare datetime.
//! * A DTO is built from model values with `from_model`; it holds no state of its own.
//! * Each DTO module exports `NAME` (the OpenAPI component schema) and `FIELDS` (the field names
//!   in order); [`dto::implemented`] lists the DTOs that exist, which is the list the
//!   component-schema test checks. Schemas not on the list are reported by the test, never failed
//!   (plan 5.5).

pub mod contract;
pub mod convention;
pub mod dto;
pub mod error;
