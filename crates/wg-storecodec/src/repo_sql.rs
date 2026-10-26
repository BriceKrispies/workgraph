//! SQL text and parameters for the workspace, project and repository reads that the other features
//! share (SH-05): the workspace, project and repository lists, the checkouts, refs and the rows the
//! registry slice is built from. The registry-only lookups and counts are the registry feature's
//! `spec::sql`.
//!
//! Every SELECT lists the table's [`columns`] in schema order, so its rows decode through the same
//! `TableRow` codecs as a fetch by key. Lists have a fixed `ORDER BY` so results are deterministic.

use wg_model::Table;
use wg_types::{ErrorCode, Id, WgError};

use crate::codec::{Raw, id_sql, text_sql};
use crate::sql::{Statement, select_list};

/// Every workspace, ordered by slug.
pub fn workspaces() -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"workspaces\" ORDER BY \"slug\"",
            select_list(Table::Workspace, None)
        ),
        params: Vec::new(),
    }
}

/// The projects of `workspace_id`, ordered by slug. Unless `include_archived`, only active ones
/// (a project's `status` other than `active` is archived).
pub fn projects(workspace_id: Id, include_archived: bool) -> Statement {
    let mut sql = format!(
        "SELECT {} FROM \"projects\" WHERE \"workspace_id\" = ?1",
        select_list(Table::Project, None)
    );
    let mut params = vec![id_sql(workspace_id)];
    if !include_archived {
        sql.push_str(" AND \"status\" = ?2");
        params.push(text_sql("active"));
    }
    sql.push_str(" ORDER BY \"slug\"");
    Statement { sql, params }
}

/// The project-to-repository links of `project_id`, ordered by repository id.
pub fn project_repositories(project_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"project_repositories\" WHERE \"project_id\" = ?1 \
             ORDER BY \"repository_id\"",
            select_list(Table::ProjectRepository, None)
        ),
        params: vec![id_sql(project_id)],
    }
}

/// The repositories of `workspace_id`, ordered by name. `project_id` narrows to the members of one
/// project; unless `include_archived`, archived repositories are left out.
pub fn repositories(workspace_id: Id, project_id: Option<Id>, include_archived: bool) -> Statement {
    let mut params = vec![id_sql(workspace_id)];
    let mut sql = format!(
        "SELECT {} FROM \"repositories\" AS r WHERE r.\"workspace_id\" = ?1",
        select_list(Table::Repository, Some("r"))
    );
    if !include_archived {
        sql.push_str(" AND r.\"archived_at\" IS NULL");
    }
    if let Some(project_id) = project_id {
        params.push(id_sql(project_id));
        sql.push_str(&format!(
            " AND r.\"id\" IN (SELECT \"repository_id\" FROM \"project_repositories\" \
             WHERE \"project_id\" = ?{})",
            params.len()
        ));
    }
    sql.push_str(" ORDER BY r.\"name\"");
    Statement { sql, params }
}

/// The checkouts of `repository_id`, ordered by path.
pub fn checkouts(repository_id: Id) -> Statement {
    repository_child(
        Table::RepositoryCheckout,
        "repository_id",
        repository_id,
        "path",
    )
}

/// The refs of `repository_id`, ordered by name.
pub fn refs(repository_id: Id) -> Statement {
    repository_child(Table::RepositoryRef, "repository_id", repository_id, "name")
}

/// `SELECT <table> WHERE <column> = ?1 ORDER BY <order>` for a table keyed under a repository.
pub fn repository_child(table: Table, column: &str, repository_id: Id, order: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"{}\" WHERE \"{column}\" = ?1 ORDER BY \"{order}\"",
            select_list(table, None),
            table.name()
        ),
        params: vec![id_sql(repository_id)],
    }
}

/// Decodes `(text, integer)` rows, such as the `GROUP BY` counts of the registry feature, in row
/// order.
pub fn pairs_from_rows(rows: &[Vec<Raw>]) -> Result<Vec<(String, i64)>, WgError> {
    rows.iter()
        .map(|row| match row.as_slice() {
            [Raw::Text(key), Raw::Int(count)] => Ok((key.clone(), *count)),
            _ => Err(WgError::new(
                ErrorCode::WorkgraphError,
                "unexpected row shape for a count query",
            )),
        })
        .collect()
}

/// Decodes the single integer of a `COUNT(*)` statement.
pub fn scalar_from_rows(rows: &[Vec<Raw>]) -> Result<i64, WgError> {
    match rows.first().map(Vec::as_slice) {
        Some([Raw::Int(count)]) => Ok(*count),
        _ => Err(WgError::new(
            ErrorCode::WorkgraphError,
            "unexpected row shape for a count query",
        )),
    }
}
