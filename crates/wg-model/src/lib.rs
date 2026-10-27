//! wg-model: entity values, slices, writes, events, effects, stamps, decisions.
//!
//! The vocabulary of the functional core (docs/rust-rewrite.md section 4.1). A command is decided
//! by a pure function over one slice:
//!
//! ```text
//! decide(&Slice, &Command, &Stamp) -> Result<Decision<Reply>, WgError>
//! ```
//!
//! The shell loads the [`slice`](mod@slice) inside the transaction, the core returns a [`Decision`] (row
//! [`Write`]s, [`EventDraft`]s, [`Effect`]s and a reply), and the shell applies the writes and
//! events, commits, performs the effects and renders the reply. This crate holds the values those
//! steps exchange; the rules live in the `Core` crates built on it (`wg-tasks`, `wg-registry`, ...).
//!
//! # Conventions
//!
//! These hold for every model module. LIB-10 and LIB-11 add the execution and knowledge families
//! by copying them.
//!
//! ## Purity (tier `Core`)
//!
//! * No `mut` binding, no `&mut`, no assignment outside a `let` initialiser: values are built
//!   whole and changed with struct update syntax (`Task { version: v + 1, ..task.clone() }`).
//! * No std collection in a type: sequences are [`rpds::VectorSync`], maps
//!   [`rpds::RedBlackTreeMapSync`], sets [`rpds::RedBlackTreeSetSync`]. Only ordered maps and sets:
//!   iteration order must not depend on a hasher. The `Sync` variants keep values `Send + Sync`.
//! * No clock and no randomness: time is [`Stamp::at`], ids come from [`Stamp::ids`].
//! * No `pub trait`, no `dyn`, no `unwrap`/`expect`, no panics. Errors are [`wg_types::WgError`].
//!
//! ## One module per table family
//!
//! A family is a root table and the tables whose rows belong to it (their foreign key to the root
//! cascades). Its entity structs live in `entity::<family>` and its event payloads in
//! `event::<family>`: [`entity::workspace`], [`entity::project`] (with `project_repositories`),
//! [`entity::repository`] (checkouts, remotes, refs, declared dependencies) and [`entity::task`]
//! (files, tags, links, comments, dependencies, relations, project and repository memberships,
//! external items).
//!
//! ## Entities
//!
//! * One struct per table, named after the Python ORM class (`TaskTag`, `RepositoryCheckout`).
//! * Fields are the columns, in `contracts/schema-0010.sql` order, named as the column (the
//!   `metadata` column is `metadata`, not Python's `metadata_`).
//! * Column types: id columns [`wg_types::Id`]; timestamp columns [`wg_types::Timestamp`] (the
//!   store's codec reads and writes O11); `INTEGER` `i64`; `BOOLEAN` `bool`; JSON objects
//!   [`entity::JsonObject`]; JSON lists of strings `VectorSync<String>`; nullable columns
//!   `Option<_>`. A text column holds a `wg_types` enum only when every Python writer validates it
//!   against that enum (task status and priority, comment kind, relation type, repository access
//!   mode); otherwise it is a `String`, so a database Python wrote always loads.
//! * Derives `Debug, Clone, PartialEq, Eq` and nothing that implies more than the row is.
//! * Each entity has `row_key(&self) -> RowKey` and `impl From<Entity> for Row`.
//!
//! ## How a write names its row
//!
//! Every table has a variant, named after its entity, in three enums of [`write`](mod@write): [`Table`] (the
//! SQL name), [`Row`] (the whole row) and [`RowKey`] (the primary key: a bare `Id` for surrogate
//! keys, named fields in primary-key column order for composite keys). A [`Write`] inserts a row,
//! replaces a whole row (optionally guarded by the `version` it expects, for `tasks`), or deletes
//! by key. The store maps each variant to SQL; the core never writes SQL or column lists. Adding a
//! table adds the three variants and the match arms the compiler then asks for.
//!
//! ## How a slice is declared
//!
//! * A struct named `<Area>Slice`, listed under `world_roots` in `tools/reports/core-rules.toml`,
//!   deriving `Debug, Clone, PartialEq, Eq, Default`.
//! * One public field per table, `RedBlackTreeMapSync<K, Entity>`, where `K` is the primary key
//!   (a tuple, parent id first, for composite keys, so a parent's children are one range).
//!   A slice may embed another (`TaskSlice::registry`).
//! * It holds only what the shell loaded for the decision's read set: absent means not loaded or
//!   not existing, and the core, which chose what to read, knows which.
//! * Lookups and per-parent queries are methods returning references or iterators in key order.
//! * `apply(&Write) -> Self` folds a write into the slice, so helpers called later in a decision
//!   see earlier changes; rows of tables the slice does not hold are ignored.
//! * The slice is added to the compile-time value assertions in `values.rs`.
//!
//! ## Events
//!
//! * One payload struct per `data` shape, named `<Entity><Verb>` after the event (`TaskCreated`);
//!   event types with the same shape share one (`TaskKeyed` for `task.archived`,
//!   `task.unarchived`, `task.became_ready`). Fields are the `data` keys, in the order Python
//!   builds the dict, with the types Python writes (ids as UUID strings, task keys as strings,
//!   `Option` for values that may be null).
//! * A key that some callers leave out is `Option<_>` with `skip_serializing_if`; one that may be
//!   left out or written as null is `Option<Option<_>>`. Open payloads (callers merge extra keys)
//!   end with a flattened `extra: JsonObject`.
//! * Each event type is one [`EventPayload`] variant, registered in the `event_payloads!` table
//!   of `event/mod.rs` with its wire name, entity type and schema version.
//! * [`EventDraft::new`] stamps an event from the [`Stamp`]; `caused_by` and `with_attempt` give
//!   Python's `ctx.caused_by(...)` and explicit `attempt_id`. [`EventDraft::data`] adds the
//!   reserved `_automation` key inside automation runs.
//!
//! ## Ids
//!
//! A decision's ids are [`derive_id`]`(stamp.entropy, slot)`. Helpers take an [`IdSeq`] and return
//! the advanced one with their result, so the slot of every id follows from the order of the
//! calls, and the same slice, command and stamp give the same decision, ids included.
//!
//! ## Decisions and effects
//!
//! A [`Decision`] is built by value (`Decision::new(reply).write(..).event(..)`). A helper that
//! changes rows but has no reply returns `Decision<()>`, joined with [`Decision::then`]. Effects
//! are data ([`Effect`]); their results come back as [`EffectResult`] inputs to the flow that asked.
//!
//! # A worked decision
//!
//! ```
//! use wg_model::event::TaskKeyed;
//! use wg_model::{Decision, EventDraft, EventPayload, Stamp, Task, TaskSlice, Write};
//! use wg_types::{ErrorCode, Id, Permission, WgError};
//!
//! /// `task archive`: sets `archived_at`, bumps the version and records `task.archived`.
//! fn archive(slice: &TaskSlice, task_id: Id, stamp: &Stamp) -> Result<Decision<Id>, WgError> {
//!     stamp.require(&[Permission::TaskWrite])?;
//!     let task = slice
//!         .task(task_id)
//!         .ok_or_else(|| WgError::not_found("task not found"))?;
//!     if task.archived_at.is_some() {
//!         return Ok(Decision::new(task.id));
//!     }
//!     let (event_id, _ids) = stamp.ids().next();
//!     let archived = Task {
//!         archived_at: Some(stamp.at),
//!         version: task.version + 1,
//!         ..task.clone()
//!     };
//!     let event = EventDraft::new(
//!         stamp,
//!         event_id,
//!         Some(task.workspace_id),
//!         task.id,
//!         EventPayload::TaskArchived(TaskKeyed { key: task.key.clone() }),
//!     );
//!     Ok(Decision::new(task.id)
//!         .write(Write::update_versioned(archived, task.version))
//!         .event(event))
//! }
//! ```

pub mod decision;
pub mod effect;
pub mod entity;
pub mod event;
pub mod slice;
pub mod stamp;
mod values;
pub mod write;

pub use decision::Decision;
pub use effect::{
    Effect, EffectError, EffectErrorKind, EffectOutcome, EffectResult, FsOp, GitOp, HttpCall,
    HttpMethod, HttpResponse, JobWake, ProcessExit, ProcessOutput, ProcessSpec, StreamEvent,
};
pub use entity::{
    ExternalItem, JsonObject, Project, ProjectRepository, Repository, RepositoryCheckout,
    RepositoryDependency, RepositoryRef, RepositoryRemote, Task, TaskComment, TaskDependency,
    TaskFile, TaskLink, TaskProject, TaskRelation, TaskRepository, TaskTag, Workspace,
};
pub use event::{Changes, EntityType, EventDraft, EventPayload, FieldChange, StoredEvent, StrMap};
pub use slice::{RegistrySlice, TaskSlice};
pub use stamp::{IdSeq, Stamp, derive_id, py_repr_str};
pub use write::{Row, RowKey, Table, Write};
