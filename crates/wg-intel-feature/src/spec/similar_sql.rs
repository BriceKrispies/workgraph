//! SQL text and typed rows for similar-task search (`SearchService.similar`): the commits a task
//! is linked to with confidence at least 0.5 (at most five), and the number of agent attempts a
//! task has.

use wg_model::Table;
use wg_rows::codec::{Cols, Raw, id_sql};
use wg_rows::sql::Statement;
use wg_types::{Id, WgError};

/// The implementation evidence a similar task shows: the linked commits, five at most.
pub fn implementation_commits(task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT c.\"sha\", c.\"subject\", c.\"repository_id\" FROM \"{commits}\" AS c \
             JOIN \"{links}\" AS l ON l.\"commit_id\" = c.\"id\" \
             WHERE l.\"task_id\" = ?1 AND l.\"confidence\" >= 0.5 LIMIT 5",
            commits = Table::Commit.name(),
            links = Table::CommitTaskLink.name(),
        ),
        params: vec![id_sql(task_id)],
    }
}

/// The number of agent attempts recorded for `task_id`.
pub fn attempt_count(task_id: Id) -> Statement {
    Statement {
        sql: format!(
            "SELECT COUNT(*) AS \"n\" FROM \"{attempts}\" WHERE \"task_id\" = ?1",
            attempts = Table::AgentAttempt.name(),
        ),
        params: vec![id_sql(task_id)],
    }
}

/// One linked commit of a similar task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImplementationCommit {
    pub sha: String,
    pub subject: String,
    pub repository_id: Id,
}

wg_rows::row_projection! {
    ImplementationCommit {
        sha: text,
        subject: text,
        repository_id: id,
    }
}

/// The attempt count of a row of [`attempt_count`].
pub fn attempts_of(values: &[Raw]) -> Result<i64, WgError> {
    Cols::new(&["n"], values).int("n")
}
