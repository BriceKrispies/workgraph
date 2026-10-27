//! The DTOs, one module per area.
//!
//! A DTO is one struct declared with [`wire_dto!`](crate::wire_dto): the fields in Python
//! declaration order, each of a type that serialises as the wire shows it (an `Option` is `null`
//! when absent, an id is its hyphenated text) or with a `serialize_with` adapter from
//! [`convention`](crate::convention) for timestamps and JSON columns. The macro derives
//! `Serialize`, the `NAME` and `FIELDS` constants, the constructor and the printing function from
//! that one declaration. Each field says where its value comes from:
//!
//! ```ignore
//! wire_dto! {
//!     /// A comment, progress report or result on a task.
//!     pub struct CommentOut<'a>("CommentOut") value from_model(c: &'a TaskComment) {
//!         id: Id = c.id,
//!         attempt_id: Option<Id> = c.attempt_id,
//!         author: &'a str = &c.author,
//!         #[serde(serialize_with = "model_z")]
//!         created_at: Timestamp = c.created_at,
//!     }
//! }
//!
//! let value = comment::from_model(&row); // a Pydantic model value
//! let names = CommentOut::FIELDS; // ["id", "attempt_id", "author", "created_at"]
//! ```
//!
//! `value` prints a Pydantic model, `dict` a plain dict where Python builds a dict literal. A DTO
//! that is only read from, or built by hand, is declared without the `from` clause (fields as
//! `name: Type`) and printed with [`Dto::value`] or [`Dto::dict`]. [`implemented`] lists the DTOs
//! the component-schema test checks.

pub mod agent_time_out;
pub mod agents;
pub mod analytics_compare_body;
pub mod analytics_compare_out;
pub mod analytics_estimate_body;
pub mod analytics_export_body;
pub mod analytics_filters;
pub mod analytics_mcp_shapes;
pub mod analytics_overview;
pub mod analytics_query;
pub mod analytics_query_out;
pub mod analytics_report_entry;
pub mod attempt;
pub mod attempt_dicts;
pub mod bodies;
pub mod classification_disagreement;
pub mod classification_evaluation;
pub mod classification_job;
pub mod classification_out;
pub mod classification_task;
pub mod claude;
pub mod comment;
pub mod commit_link;
pub mod contract_check;
pub mod coupling_out;
pub mod dependency_state;
pub mod estimate_out;
pub mod event;
pub mod experiment;
pub mod experiment_shapes;
pub mod failures_out;
pub mod graph_edge;
pub mod graph_node;
pub mod health;
pub mod job;
pub mod llm;
pub mod mcp_outputs;
pub mod metric_definition;
pub mod page;
pub mod predictor;
pub mod relation;
pub mod repository_brief;
pub mod retro_body;
pub mod retrospective_out;
pub mod scheduler;
pub mod search_result;
pub mod stack;
pub mod stack_create;
pub mod stack_item;
pub mod stack_item_add;
pub mod stack_repository;
pub mod subagents_out;
pub mod task;
pub mod task_brief;
pub mod task_changes;
pub mod task_detail;
pub mod task_diff;
pub mod task_file;
pub mod task_graph;
pub mod task_history_row;
pub mod task_link;
pub mod task_rollup;
pub mod trial;
pub mod verification;
pub mod worker;

#[doc(hidden)]
pub use wg_json::Value as DtoValue;

/// A wire struct declared with [`wire_dto!`](crate::wire_dto).
pub trait Dto: serde::Serialize {
    /// The OpenAPI component schema (or dict name) this DTO mirrors.
    const NAME: &'static str;
    /// The fields in Python declaration order.
    const FIELDS: &'static [&'static str];

    /// The DTO as a Pydantic model value (`model_dump(mode="json")`). A DTO that extends another
    /// serialises as a map (serde's `flatten`); it is turned back into a model named [`Dto::NAME`].
    fn value(&self) -> wg_json::Value {
        match wg_json::to_value(self) {
            wg_json::Value::Dict(pairs) => wg_json::Value::Model {
                name: Self::NAME.to_string(),
                fields: pairs
                    .into_iter()
                    .map(|(key, value)| (key.to_text().unwrap_or_default(), value))
                    .collect(),
            },
            other => other,
        }
    }

    /// The DTO as a plain dict value, for shapes Python builds as a dict literal.
    fn dict(&self) -> wg_json::Value {
        wg_json::to_dict(self)
    }
}

/// Declares a DTO: a struct that derives `Serialize` with the wire name given in the parentheses
/// as its serde name, and gets `NAME` and `FIELDS` (the field names in declaration order) as
/// associated constants plus the [`Dto`] impl. See the [`dto`](crate::dto) module for an example.
#[macro_export]
macro_rules! wire_dto {
    // A DTO built from its sources: each field names its type and the expression that fills it,
    // `new(..)` takes the sources, and a function of the given name returns the printed value
    // (`value` for a model, `dict` for a dict literal).
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident $(<$lt:lifetime>)? ($wire:literal)
        $mode:ident $fname:ident ( $($p:ident : $pt:ty),* $(,)? ) {
            $( $(#[$fmeta:meta])* $field:ident : $ty:ty = $val:expr ),* $(,)?
        }
    ) => {
        $crate::wire_dto! {
            $(#[$meta])*
            $vis struct $name $(<$lt>)? ($wire) {
                $( $(#[$fmeta])* pub $field : $ty ),*
            }
        }

        impl $(<$lt>)? $name $(<$lt>)? {
            /// The DTO filled from its sources.
            #[allow(clippy::new_without_default)]
            pub fn new($($p: $pt),*) -> Self {
                $name { $($field: $val),* }
            }
        }

        /// The DTO filled from its sources, printed.
        $vis fn $fname $(<$lt>)? ($($p: $pt),*) -> $crate::dto::DtoValue {
            $crate::dto::Dto::$mode(&<$name>::new($($p),*))
        }
    };
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident $(<$lt:lifetime>)? ($wire:literal) {
            $( $(#[$fmeta:meta])* $fvis:vis $field:ident : $ty:ty ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, serde::Serialize)]
        #[serde(rename = $wire)]
        $vis struct $name $(<$lt>)? {
            $( $(#[$fmeta])* $fvis $field : $ty ),*
        }

        #[allow(dead_code)]
        impl $(<$lt>)? $name $(<$lt>)? {
            /// The wire name of this DTO.
            pub const NAME: &'static str = $wire;
            /// The field names in Python declaration order.
            pub const FIELDS: &'static [&'static str] = &[$(stringify!($field)),*];
        }

        impl $(<$lt>)? $crate::dto::Dto for $name $(<$lt>)? {
            const NAME: &'static str = $wire;
            const FIELDS: &'static [&'static str] = &[$(stringify!($field)),*];
        }
    };
    // A DTO that extends another (`TaskDetail` is `TaskOut` plus four fields): the base is
    // flattened in front of the new fields. `fields_of` names the base again without lifetimes
    // (`TaskOut<'static>`), because a constant cannot mention the struct's own lifetime.
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident $(<$lt:lifetime>)? ($wire:literal) extends $base:ty, fields_of $fbase:ty {
            $( $(#[$fmeta:meta])* $fvis:vis $field:ident : $ty:ty ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, serde::Serialize)]
        #[serde(rename = $wire)]
        $vis struct $name $(<$lt>)? {
            #[serde(flatten)]
            pub base: $base,
            $( $(#[$fmeta])* $fvis $field : $ty ),*
        }

        #[allow(dead_code)]
        impl $(<$lt>)? $name $(<$lt>)? {
            /// The wire name of this DTO.
            pub const NAME: &'static str = $wire;
            /// The field names in Python declaration order: the base's, then this DTO's.
            pub const FIELDS: &'static [&'static str] = &$crate::dto::concat_fields::<
                { <$fbase>::FIELDS.len() + [$(stringify!($field)),*].len() },
            >(<$fbase>::FIELDS, &[$(stringify!($field)),*]);
        }

        impl $(<$lt>)? $crate::dto::Dto for $name $(<$lt>)? {
            const NAME: &'static str = $wire;
            const FIELDS: &'static [&'static str] = <$name $(<$lt>)?>::FIELDS;
        }
    };
}

/// `base` then `extra`, for the `FIELDS` of a DTO that extends another. `N` is the total length.
pub const fn concat_fields<const N: usize>(
    base: &[&'static str],
    extra: &[&'static str],
) -> [&'static str; N] {
    let mut all = [""; N];
    let mut index = 0;
    while index < N {
        all[index] = if index < base.len() {
            base[index]
        } else {
            extra[index - base.len()]
        };
        index += 1;
    }
    all
}

/// The DTOs that exist: the OpenAPI component schema name and the field names in order.
pub fn implemented() -> Vec<(&'static str, &'static [&'static str])> {
    let mut list = vec![
        (repository_brief::NAME, repository_brief::FIELDS.as_slice()),
        (task::NAME, task::FIELDS.as_slice()),
        (task_detail::NAME, task_detail::FIELDS.as_slice()),
        (task_brief::NAME, task_brief::FIELDS.as_slice()),
        (task_file::NAME, task_file::FIELDS.as_slice()),
        (task_link::NAME, task_link::FIELDS.as_slice()),
        (task_rollup::NAME, task_rollup::FIELDS.as_slice()),
        (dependency_state::NAME, dependency_state::FIELDS.as_slice()),
        (comment::NAME, comment::FIELDS.as_slice()),
        (relation::NAME, relation::FIELDS.as_slice()),
        (graph_node::NAME, graph_node::FIELDS.as_slice()),
        (graph_edge::NAME, graph_edge::FIELDS.as_slice()),
        (task_graph::NAME, task_graph::FIELDS.as_slice()),
        (page::NAME, page::FIELDS.as_slice()),
        (stack::NAME, stack::FIELDS.as_slice()),
        (stack_item::NAME, stack_item::FIELDS.as_slice()),
        (stack_repository::NAME, stack_repository::FIELDS.as_slice()),
        (stack_create::NAME, stack_create::FIELDS.as_slice()),
        (stack_item_add::NAME, stack_item_add::FIELDS.as_slice()),
        (event::NAME, event::FIELDS.as_slice()),
        (
            analytics_compare_body::NAME,
            analytics_compare_body::FIELDS.as_slice(),
        ),
        (
            analytics_estimate_body::NAME,
            analytics_estimate_body::FIELDS.as_slice(),
        ),
        (
            analytics_export_body::NAME,
            analytics_export_body::FIELDS.as_slice(),
        ),
        (
            analytics_filters::NAME,
            analytics_filters::FIELDS.as_slice(),
        ),
        (analytics_query::NAME, analytics_query::FIELDS.as_slice()),
        (retro_body::NAME, retro_body::FIELDS.as_slice()),
    ];
    list.extend(bodies::ALL.iter().map(|body| (body.name, body.fields)));
    list
}

#[cfg(test)]
mod tests {
    use wg_json::{Value, http_json};
    use wg_types::{Id, Timestamp};

    use super::Dto;
    use crate::convention::model_z;

    wire_dto! {
        #[allow(dead_code)]
        struct Base<'a>("BaseOut") {
            id: Id,
            name: &'a str,
            note: Option<&'a str>,
            #[serde(serialize_with = "model_z")]
            at: Option<Timestamp>,
        }
    }

    wire_dto! {
        #[allow(dead_code)]
        struct Child<'a>("ChildOut") extends Base<'a>, fields_of Base<'static> {
            done: bool,
        }
    }

    fn base() -> Base<'static> {
        Base {
            id: Id::nil(),
            name: "x",
            note: None,
            at: Timestamp::from_components(2026, 10, 25, 13, 56, 0, 0).ok(),
        }
    }

    const BASE_JSON: &str = r#""id":"00000000-0000-0000-0000-000000000000","name":"x","note":null,"at":"2026-10-25T13:56:00Z""#;

    #[test]
    fn a_dto_prints_its_fields_in_declaration_order_as_a_model() {
        assert_eq!(Base::NAME, "BaseOut");
        assert_eq!(Base::FIELDS, ["id", "name", "note", "at"]);
        let value = base().value();
        assert!(matches!(&value, Value::Model { name, .. } if name == "BaseOut"));
        assert_eq!(http_json(&value).unwrap(), format!("{{{BASE_JSON}}}"));
    }

    #[test]
    fn a_dto_can_print_as_a_plain_dict() {
        let value = base().dict();
        assert!(matches!(value, Value::Dict(_)));
    }

    #[test]
    fn an_extending_dto_lists_the_base_fields_first() {
        assert_eq!(Child::FIELDS, ["id", "name", "note", "at", "done"]);
        let value = Child {
            base: base(),
            done: true,
        }
        .value();
        assert!(matches!(&value, Value::Model { name, .. } if name == "ChildOut"));
        assert_eq!(
            http_json(&value).unwrap(),
            format!("{{{BASE_JSON},\"done\":true}}")
        );
    }
}
