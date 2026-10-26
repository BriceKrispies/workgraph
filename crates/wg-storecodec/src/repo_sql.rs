//! SQL text and parameters for the workspace, project and repository family (SH-05): the lookups
//! the services make by slug, name, fingerprint, normalised remote URL and checkout path, and the
//! list queries that hang off a repository or a project.
//!
//! Every SELECT lists the table's [`columns`] in schema order, so its rows decode through the same
//! `TableRow` codecs as a fetch by key. Lists have a fixed `ORDER BY` so results are deterministic.

use wg_model::Table;
use wg_types::Id;

use crate::codec::{id_sql, text_sql};
use crate::rows::columns;
use crate::sql::Statement;

/// The table's columns, quoted and qualified by `alias` when one is given.
fn select_list(table: Table, alias: Option<&str>) -> String {
    columns(table)
        .iter()
        .map(|column| match alias {
            Some(alias) => format!("{alias}.\"{column}\""),
            None => format!("\"{column}\""),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `?1, ?2, ...` for `count` placeholders starting at `first`, or `NULL` when there are none (an
/// `IN (NULL)` list matches nothing, which is what an empty value list should match).
fn placeholders(first: usize, count: usize) -> String {
    if count == 0 {
        return "NULL".to_owned();
    }
    (first..first + count)
        .map(|n| format!("?{n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The workspace whose slug is `slug`.
pub fn workspace_by_slug(slug: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"workspaces\" WHERE \"slug\" = ?1",
            select_list(Table::Workspace, None)
        ),
        params: vec![text_sql(slug)],
    }
}

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

/// The project in `workspace_id` whose slug is `slug`.
pub fn project_by_slug(workspace_id: Id, slug: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"projects\" WHERE \"workspace_id\" = ?1 AND \"slug\" = ?2",
            select_list(Table::Project, None)
        ),
        params: vec![id_sql(workspace_id), text_sql(slug)],
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

/// The repository in `workspace_id` named `name`.
pub fn repository_by_name(workspace_id: Id, name: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"repositories\" WHERE \"workspace_id\" = ?1 AND \"name\" = ?2",
            select_list(Table::Repository, None)
        ),
        params: vec![id_sql(workspace_id), text_sql(name)],
    }
}

/// The repositories of `workspace_id` whose identity fingerprint is `fingerprint`, ordered by name.
pub fn repository_by_fingerprint(workspace_id: Id, fingerprint: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"repositories\" WHERE \"workspace_id\" = ?1 \
             AND \"identity_fingerprint\" = ?2 ORDER BY \"name\"",
            select_list(Table::Repository, None)
        ),
        params: vec![id_sql(workspace_id), text_sql(fingerprint)],
    }
}

/// The repositories of `workspace_id` with a remote whose normalised URL is one of `urls`, ordered
/// by name. Each repository appears once however many of its remotes match.
pub fn repository_by_remote_urls(workspace_id: Id, urls: &[String]) -> Statement {
    let mut params = vec![id_sql(workspace_id)];
    params.extend(urls.iter().map(|url| text_sql(url)));
    Statement {
        sql: format!(
            "SELECT {} FROM \"repositories\" AS r WHERE r.\"workspace_id\" = ?1 AND r.\"id\" IN \
             (SELECT \"repository_id\" FROM \"repository_remotes\" WHERE \"normalized_url\" IN ({})) \
             ORDER BY r.\"name\"",
            select_list(Table::Repository, Some("r")),
            placeholders(2, urls.len())
        ),
        params,
    }
}

/// The repository in `workspace_id` with a checkout at `path`, ordered by name (the first one, as
/// the service takes it).
pub fn repository_by_checkout_path(workspace_id: Id, path: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"repositories\" AS r JOIN \"repository_checkouts\" AS c \
             ON c.\"repository_id\" = r.\"id\" WHERE r.\"workspace_id\" = ?1 AND c.\"path\" = ?2 \
             ORDER BY r.\"name\"",
            select_list(Table::Repository, Some("r"))
        ),
        params: vec![id_sql(workspace_id), text_sql(path)],
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

/// The remotes of `repository_id`, ordered by name.
pub fn remotes(repository_id: Id) -> Statement {
    repository_child(
        Table::RepositoryRemote,
        "repository_id",
        repository_id,
        "name",
    )
}

/// The refs of `repository_id`, ordered by name.
pub fn refs(repository_id: Id) -> Statement {
    repository_child(Table::RepositoryRef, "repository_id", repository_id, "name")
}

/// The dependencies that start at `repository_id`, ordered by the repository they point at.
pub fn dependencies_from(repository_id: Id) -> Statement {
    repository_child(
        Table::RepositoryDependency,
        "from_repository_id",
        repository_id,
        "to_repository_id",
    )
}

/// The dependencies that point at `repository_id`, ordered by the repository they start from.
pub fn dependencies_to(repository_id: Id) -> Statement {
    repository_child(
        Table::RepositoryDependency,
        "to_repository_id",
        repository_id,
        "from_repository_id",
    )
}

/// The dependency from `from` to `to`, if one is recorded.
pub fn dependency_between(from: Id, to: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"repository_dependencies\" WHERE \"from_repository_id\" = ?1 \
             AND \"to_repository_id\" = ?2",
            select_list(Table::RepositoryDependency, None)
        ),
        params: vec![id_sql(from), id_sql(to)],
    }
}

/// `SELECT <table> WHERE <column> = ?1 ORDER BY <order>` for a table keyed under a repository.
fn repository_child(table: Table, column: &str, repository_id: Id, order: &str) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"{}\" WHERE \"{column}\" = ?1 ORDER BY \"{order}\"",
            select_list(table, None),
            table.name()
        ),
        params: vec![id_sql(repository_id)],
    }
}
