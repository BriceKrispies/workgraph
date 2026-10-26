//! SQL text and parameters of the registry's lookups (SH-05): the lookups the services make by
//! slug, name, fingerprint, normalised remote URL and checkout path, the dependency reads, and the
//! counts and recent-task lists the workspace and project summaries read.
//!
//! The statements that other features read (workspaces, projects, repositories, checkouts, refs)
//! stay in `wg_storecodec::repo_sql`. Every SELECT lists the table's columns in schema order, so
//! its rows decode through the same `TableRow` codecs as a fetch by key.

use wg_model::Table;
use wg_storecodec::codec::{Raw, id_sql, text_sql};
use wg_storecodec::repo_sql::repository_child;
use wg_storecodec::sql::{Statement, select_list};
use wg_types::Id;

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

/// The remotes of `repository_id`, ordered by name.
pub fn remotes(repository_id: Id) -> Statement {
    repository_child(
        Table::RepositoryRemote,
        "repository_id",
        repository_id,
        "name",
    )
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

/// Live (not archived) tasks of a workspace, counted by status, in status order. The rows are
/// `(status, count)`; see `wg_storecodec::repo_sql::pairs_from_rows`.
pub fn workspace_task_counts(workspace_id: Id) -> Statement {
    Statement {
        sql: "SELECT \"status\", COUNT(*) FROM \"tasks\" WHERE \"workspace_id\" = ?1 \
              AND \"archived_at\" IS NULL GROUP BY \"status\" ORDER BY \"status\""
            .to_owned(),
        params: vec![id_sql(workspace_id)],
    }
}

/// Live (not archived) tasks of a project, counted by status, in status order.
pub fn project_task_counts(project_id: Id) -> Statement {
    Statement {
        sql: "SELECT t.\"status\", COUNT(*) FROM \"tasks\" AS t \
              JOIN \"task_projects\" AS tp ON tp.\"task_id\" = t.\"id\" \
              WHERE tp.\"project_id\" = ?1 AND t.\"archived_at\" IS NULL \
              GROUP BY t.\"status\" ORDER BY t.\"status\""
            .to_owned(),
        params: vec![id_sql(project_id)],
    }
}

/// The `limit` most recently updated live tasks of a project, newest first.
pub fn project_recent_tasks(project_id: Id, limit: i64) -> Statement {
    Statement {
        sql: format!(
            "SELECT {} FROM \"tasks\" AS t \
             JOIN \"task_projects\" AS tp ON tp.\"task_id\" = t.\"id\" \
             WHERE tp.\"project_id\" = ?1 AND t.\"archived_at\" IS NULL \
             ORDER BY t.\"updated_at\" DESC LIMIT ?2",
            select_list(Table::Task, Some("t"))
        ),
        params: vec![id_sql(project_id), Raw::Int(limit)],
    }
}

/// The attempts of a workspace that are still preparing, running or verifying, as one count.
pub fn workspace_active_attempts(workspace_id: Id) -> Statement {
    Statement {
        sql: "SELECT COUNT(*) FROM \"agent_attempts\" WHERE \"workspace_id\" = ?1 \
              AND \"status\" IN ('preparing', 'running', 'verifying')"
            .to_owned(),
        params: vec![id_sql(workspace_id)],
    }
}

/// Jobs whose workspace is `workspace_id`, counted by status, in status order.
pub fn workspace_job_counts(workspace_id: Id) -> Statement {
    Statement {
        sql: "SELECT \"status\", COUNT(*) FROM \"jobs\" WHERE \"workspace_id\" = ?1 \
              GROUP BY \"status\" ORDER BY \"status\""
            .to_owned(),
        params: vec![id_sql(workspace_id)],
    }
}
