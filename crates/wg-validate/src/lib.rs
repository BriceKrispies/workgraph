//! wg-validate: request validation with Pydantic-shaped error lists.
//!
//! A [`Model`] describes the fields of a request body or of an MCP tool's arguments. [`validate`]
//! checks a JSON value against it in Pydantic's lax mode and returns either the value Pydantic's
//! `model_dump(mode="json")` would give (defaults filled in, datetimes normalised, unknown keys
//! dropped) or the list of [`ValidationError`]s with Pydantic's `type`, `loc`, `msg`, `input` and
//! `ctx`.
//!
//! Models are built in code or loaded from the JSON Schema documents in `contracts/`
//! ([`model_from_json_schema`]). Those documents carry what Pydantic's schema generator emits and
//! no more: the order of the fields (their keys are sorted) and the Python validators (slug, key
//! prefix, tags, files) are not in them. [`Model::ordered`] and [`Check`] add both back.
//!
//! The golden vectors of GV-14 (`golden/pydantic/`) pin the behaviour.

mod checks;
mod datetime;
mod error;
mod json_schema;
mod schema;
mod validate;

pub use error::{Loc, ValidationError};
pub use json_schema::{LoadError, LoadOptions, model_from_json_schema};
pub use schema::{Check, ChoiceKind, Extra, Field, FloatRules, IntRules, Model, Schema, StrRules};
pub use validate::validate;
