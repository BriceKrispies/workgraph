//! Row mapping: one declaration per table gives its column list, its `values()` encoding, its
//! `decode()` and its primary key.
//!
//! ```ignore
//! row_codec! {
//!     Workspace key(id: id) {
//!         id: id,
//!         slug: text,
//!         next_task_number: int,
//!         settings: object,
//!         archived_at: opt_ts,
//!         created_at: ts,
//!     }
//! }
//! ```
//!
//! Each field is `name: codec`. The name is both the entity field and the column (columns are
//! listed in the order of `contracts/schema-0011.sql`, which is the declaration order). The
//! codec picks the pair of functions from [`crate::codec`] that encode and decode the column:
//!
//! | codec | Rust type | encode | decode |
//! |---|---|---|---|
//! | `id`, `opt_id` | `Id`, `Option<Id>` | `id_sql`, `opt_id_sql` | `id`, `opt_id` |
//! | `text`, `opt_text` | `String`, `Option<String>` | `text_sql`, `opt_text_sql` | `text`, `opt_text` |
//! | `int`, `opt_int` | `i64`, `Option<i64>` | `Raw::Int`, `opt_int_sql` | `int`, `opt_int` |
//! | `real`, `opt_real` | `Real`, `Option<Real>` | `real_sql`, `opt_real_sql` | `real`, `opt_real` |
//! | `bool` | `bool` | `bool_sql` | `flag` |
//! | `ts`, `opt_ts` | `Timestamp`, `Option<Timestamp>` | `ts_sql`, `opt_ts_sql` | `ts`, `opt_ts` |
//! | `object`, `opt_object` | `JsonObject`, `Option<JsonObject>` | `object_sql`, `opt_object_sql` | `object`, `opt_object` |
//! | `strings`, `objects`, `values` | JSON lists | `strings_sql`, `objects_sql`, `values_sql` | same names |
//! | `decision_options` | `VectorSync<DecisionOption>` | `decision_options_sql` | `decision_options` |
//! | `opt_correlation` | `Option<Correlation>` | `opt_correlation_sql` | `opt_correlation` |
//! | `blob` | `VectorSync<u8>` | `blob_sql` | `blob` |
//! | `parsed` | a wire-string enum | `text_sql(v.as_str())` | `parsed` |
//! | `task_key` | `TaskKey` | `Raw::Text(v.to_string())` | `TaskKey::parse` of the text |
//!
//! `key(id: id)` is the primary key of a table whose [`RowKey`] variant is a tuple of one value;
//! `key{a: id, b: int}` names the fields of a struct variant. Key codecs are `id`, `text`, `int`.
//!
//! `row_codec!` writes one `TableRow` implementation. `table_rows!`
//! takes the declarations of every table and also writes the dispatching `columns`,
//! `primary_key`, `key_values` and `row_values` functions, so a table is declared once.
//!
//! `row_projection!` is the read-only form for a struct decoded from a few named columns of a
//! join, an aggregate or a column subset (same codecs):
//!
//! ```ignore
//! wg_storecodec::row_projection! {
//!     PathTotal { path: text, changes: int, insertions: int, deletions: int }
//! }
//! // PathTotal::COLUMNS, PathTotal::from_raw(&values)
//! ```

/// The bound parameter of one field value.
macro_rules! row_encode {
    (id, $v:expr) => {
        $crate::codec::id_sql($v)
    };
    (opt_id, $v:expr) => {
        $crate::codec::opt_id_sql($v)
    };
    (text, $v:expr) => {
        $crate::codec::text_sql(&$v)
    };
    (opt_text, $v:expr) => {
        $crate::codec::opt_text_sql(&$v)
    };
    (int, $v:expr) => {
        $crate::codec::Raw::Int($v)
    };
    (opt_int, $v:expr) => {
        $crate::codec::opt_int_sql($v)
    };
    (real, $v:expr) => {
        $crate::codec::real_sql($v)
    };
    (opt_real, $v:expr) => {
        $crate::codec::opt_real_sql($v)
    };
    (bool, $v:expr) => {
        $crate::codec::bool_sql($v)
    };
    (ts, $v:expr) => {
        $crate::codec::ts_sql($v)
    };
    (opt_ts, $v:expr) => {
        $crate::codec::opt_ts_sql($v)
    };
    (object, $v:expr) => {
        $crate::codec::object_sql(&$v)?
    };
    (opt_object, $v:expr) => {
        $crate::codec::opt_object_sql(&$v)?
    };
    (strings, $v:expr) => {
        $crate::codec::strings_sql(&$v)?
    };
    (objects, $v:expr) => {
        $crate::codec::objects_sql(&$v)?
    };
    (values, $v:expr) => {
        $crate::codec::values_sql(&$v)?
    };
    (decision_options, $v:expr) => {
        $crate::codec::decision_options_sql(&$v)?
    };
    (opt_correlation, $v:expr) => {
        $crate::codec::opt_correlation_sql($v.as_ref())
    };
    (blob, $v:expr) => {
        $crate::codec::blob_sql(&$v)
    };
    (parsed, $v:expr) => {
        $crate::codec::text_sql($v.as_str())
    };
    (task_key, $v:expr) => {
        $crate::codec::Raw::Text($v.to_string())
    };
}

/// The value of one field, read from the named column of `$c`. Exported for `row_projection!`.
#[doc(hidden)]
#[macro_export]
macro_rules! row_decode {
    (bool, $c:ident, $n:expr) => {
        $c.flag($n)?
    };
    (task_key, $c:ident, $n:expr) => {
        $crate::__private::TaskKey::parse(&$c.text($n)?)?
    };
    (id, $c:ident, $n:expr) => {
        $c.id($n)?
    };
    (opt_id, $c:ident, $n:expr) => {
        $c.opt_id($n)?
    };
    (text, $c:ident, $n:expr) => {
        $c.text($n)?
    };
    (opt_text, $c:ident, $n:expr) => {
        $c.opt_text($n)?
    };
    (int, $c:ident, $n:expr) => {
        $c.int($n)?
    };
    (opt_int, $c:ident, $n:expr) => {
        $c.opt_int($n)?
    };
    (real, $c:ident, $n:expr) => {
        $c.real($n)?
    };
    (opt_real, $c:ident, $n:expr) => {
        $c.opt_real($n)?
    };
    (ts, $c:ident, $n:expr) => {
        $c.ts($n)?
    };
    (opt_ts, $c:ident, $n:expr) => {
        $c.opt_ts($n)?
    };
    (object, $c:ident, $n:expr) => {
        $c.object($n)?
    };
    (opt_object, $c:ident, $n:expr) => {
        $c.opt_object($n)?
    };
    (strings, $c:ident, $n:expr) => {
        $c.strings($n)?
    };
    (objects, $c:ident, $n:expr) => {
        $c.objects($n)?
    };
    (values, $c:ident, $n:expr) => {
        $c.values($n)?
    };
    (decision_options, $c:ident, $n:expr) => {
        $c.decision_options($n)?
    };
    (opt_correlation, $c:ident, $n:expr) => {
        $c.opt_correlation($n)?
    };
    (blob, $c:ident, $n:expr) => {
        $c.blob($n)?
    };
    (parsed, $c:ident, $n:expr) => {
        $c.parsed($n)?
    };
}

/// `impl $t { COLUMNS, from_raw }` for a struct decoded from named columns, in field order. The
/// field name is the column name; codecs are those of the module docs.
#[macro_export]
macro_rules! row_projection {
    ($t:ident { $($f:ident : $codec:ident),+ $(,)? }) => {
        impl $t {
            /// The columns this row is decoded from, in order.
            pub const COLUMNS: &'static [&'static str] = &[$(stringify!($f)),+];

            /// Decodes a row whose values are in [`Self::COLUMNS`] order.
            pub fn from_raw(
                values: &[$crate::Raw],
            ) -> Result<Self, $crate::__private::WgError> {
                let c = $crate::Cols::new(Self::COLUMNS, values);
                Ok($t { $($f: $crate::row_decode!($codec, c, stringify!($f))),+ })
            }
        }
    };
}

/// The bound parameter of one primary-key value.
macro_rules! key_encode {
    (id, $v:expr) => {
        $crate::codec::id_sql(*$v)
    };
    (text, $v:expr) => {
        $crate::codec::text_sql($v)
    };
    (int, $v:expr) => {
        $crate::codec::Raw::Int(*$v)
    };
}

/// One `TableRow` implementation; see the module docs.
macro_rules! row_codec {
    ($t:ident key $key:tt { $($f:ident : $codec:ident),+ $(,)? }) => {
        impl $crate::rows::TableRow for $t {
            const TABLE: wg_model::Table = wg_model::Table::$t;
            const COLUMNS: &'static [&'static str] = &[$(stringify!($f)),+];
            const PRIMARY_KEY: &'static [&'static str] = row_codec!(@cols $key);

            fn values(&self) -> Result<Vec<$crate::codec::Raw>, wg_types::WgError> {
                Ok(vec![$(row_encode!($codec, self.$f)),+])
            }

            fn decode(c: &$crate::codec::Cols<'_>) -> Result<Self, wg_types::WgError> {
                Ok($t { $($f: row_decode!($codec, c, stringify!($f))),+ })
            }

            fn key_values(key: &wg_model::RowKey) -> Vec<$crate::codec::Raw> {
                row_codec!(@key $t key $key)
            }
        }
    };
    (@cols ($kf:ident : $kc:ident)) => {
        &[stringify!($kf)]
    };
    (@cols { $($kf:ident : $kc:ident),+ }) => {
        &[$(stringify!($kf)),+]
    };
    (@key $t:ident $k:ident ($kf:ident : $kc:ident)) => {
        match $k {
            wg_model::RowKey::$t($kf) => vec![key_encode!($kc, $kf)],
            _ => Vec::new(),
        }
    };
    (@key $t:ident $k:ident { $($kf:ident : $kc:ident),+ }) => {
        match $k {
            wg_model::RowKey::$t { $($kf),+ } => vec![$(key_encode!($kc, $kf)),+],
            _ => Vec::new(),
        }
    };
}

/// Every table's declaration, plus the functions that dispatch on a `Table`, `Row` or `RowKey`.
/// `Table` and `Row` are matched exhaustively, so a table missing here fails to compile.
macro_rules! table_rows {
    ($($t:ident key $key:tt { $($fields:tt)+ })+) => {
        $(row_codec! { $t key $key { $($fields)+ } })+

        /// The columns of `table`, in schema order.
        pub const fn columns(table: wg_model::Table) -> &'static [&'static str] {
            match table {
                $(wg_model::Table::$t => <$t as $crate::rows::TableRow>::COLUMNS,)+
            }
        }

        /// The primary-key columns of `table`, in key order (the order of `key_values`).
        pub const fn primary_key(table: wg_model::Table) -> &'static [&'static str] {
            match table {
                $(wg_model::Table::$t => <$t as $crate::rows::TableRow>::PRIMARY_KEY,)+
            }
        }

        /// The primary-key values of `key`, in the order of `primary_key`.
        pub fn key_values(key: &wg_model::RowKey) -> Vec<$crate::codec::Raw> {
            match key.table() {
                $(wg_model::Table::$t => <$t as $crate::rows::TableRow>::key_values(key),)+
            }
        }

        /// The bound parameters of any row, in the order of `columns` for its table.
        pub fn row_values(
            row: &wg_model::Row,
        ) -> Result<Vec<$crate::codec::Raw>, wg_types::WgError> {
            match row {
                $(wg_model::Row::$t(row) => $crate::rows::TableRow::values(row),)+
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use wg_model::{RowKey, Table};
    use wg_types::{Id, WgError};

    use crate::codec::{Cols, Raw};
    use crate::rows::TableRow;

    // A toy row named after an existing table, so the macro's `Table::TaskTag` and
    // `RowKey::TaskTag` paths resolve while `TaskTag` here is the local struct.
    mod toy {
        use wg_types::Id;

        #[derive(Debug, PartialEq)]
        pub struct TaskTag {
            pub task_id: Id,
            pub tag: String,
            pub weight: Option<i64>,
            pub pinned: bool,
        }

        row_codec! {
            TaskTag key{task_id: id, tag: text} {
                task_id: id,
                tag: text,
                weight: opt_int,
                pinned: bool,
            }
        }
    }
    use toy::TaskTag;

    #[derive(Debug, PartialEq)]
    struct Totals {
        path: String,
        changes: i64,
        reachable: bool,
        at: Option<wg_types::Timestamp>,
    }

    row_projection! {
        Totals {
            path: text,
            changes: int,
            reachable: bool,
            at: opt_ts,
        }
    }

    #[test]
    fn projection_decodes_named_columns_in_field_order() -> Result<(), WgError> {
        assert_eq!(Totals::COLUMNS, ["path", "changes", "reachable", "at"]);
        let raw = [
            Raw::Text("a.rs".to_owned()),
            Raw::Int(4),
            Raw::Int(1),
            Raw::Null,
        ];
        assert_eq!(
            Totals::from_raw(&raw)?,
            Totals {
                path: "a.rs".to_owned(),
                changes: 4,
                reachable: true,
                at: None
            }
        );
        let wrong = [Raw::Null, Raw::Int(4), Raw::Int(1), Raw::Null];
        assert_eq!(
            Totals::from_raw(&wrong).unwrap_err().to_string(),
            "column path: unexpected NULL (workgraph_error)"
        );
        Ok(())
    }

    #[test]
    fn declaration_gives_columns_key_values_and_round_trip() -> Result<(), WgError> {
        assert_eq!(TaskTag::COLUMNS, ["task_id", "tag", "weight", "pinned"]);
        assert_eq!(TaskTag::PRIMARY_KEY, ["task_id", "tag"]);
        assert_eq!(TaskTag::TABLE, Table::TaskTag);

        let id: Id = wg_types::parse_id("00000000-0000-4000-8000-000000000001")?;
        let key = RowKey::TaskTag {
            task_id: id,
            tag: "x".to_owned(),
        };
        assert_eq!(
            TaskTag::key_values(&key),
            [Raw::Text(id.to_string()), Raw::Text("x".to_owned())]
        );
        assert!(TaskTag::key_values(&RowKey::Task(id)).is_empty());

        let raw = [
            Raw::Text(id.to_string()),
            Raw::Text("x".to_owned()),
            Raw::Null,
            Raw::Int(1),
        ];
        let row = TaskTag::decode(&Cols::new(TaskTag::COLUMNS, &raw))?;
        assert_eq!(
            row,
            TaskTag {
                task_id: id,
                tag: "x".to_owned(),
                weight: None,
                pinned: true
            }
        );
        assert_eq!(row.values()?, raw);
        Ok(())
    }
}
