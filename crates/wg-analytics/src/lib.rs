//! wg-analytics: fact tables, metric definitions, query engine, reports.
//!
//! The fact tables of the analytics warehouse, built as pure functions of the source rows
//! ([`AnalyticsInput`]). Python copies the rows into an in-memory DuckDB database and runs SQL over
//! them (`analytics/warehouse.py`); here each table is a fold over persistent maps with the same
//! NULL, join and aggregate semantics (see [`agg`]). The metric queries come later and read these
//! tables.
//!
//! The modules: [`commits`] holds `task_commits`, `task_code` and `task_files`; [`tasks`] holds
//! `task_surface` and `task_facts`; [`attempts`] holds `attempt_facts`.
//!
//! `commit_facts` is listed in the module docstring of `warehouse.py` but never built there, so it
//! is not built here either.

pub mod agent_time;
mod agg;
pub mod attempts;
pub mod commits;
pub mod common;
pub mod source;
pub mod subagents;
pub mod tasks;

pub use attempts::AttemptFact;
pub use commits::{TaskCode, TaskCommit, TaskFile};
pub use source::AnalyticsInput;
pub use tasks::{TaskFact, TaskSurface};

/// Every fact table of one warehouse, built from one [`AnalyticsInput`].
#[derive(Debug, Clone, PartialEq)]
pub struct Facts {
    pub task_commits: Vec<TaskCommit>,
    pub task_code: Vec<TaskCode>,
    pub task_files: Vec<TaskFile>,
    pub task_surface: Vec<TaskSurface>,
    pub task_facts: Vec<TaskFact>,
    pub attempt_facts: Vec<AttemptFact>,
}

/// Builds the fact tables in the dependency order of `warehouse.py` `_build`.
pub fn build(input: &AnalyticsInput) -> Facts {
    let task_commits = commits::task_commits(input);
    let task_code = commits::task_code(input, &task_commits);
    let task_files = commits::task_files(input, &task_commits);
    let task_facts = tasks::task_facts(input, &task_code, &task_files);
    let projects = tasks::projects(input);
    let attempt_facts = attempts::attempt_facts(input, &projects);
    Facts {
        task_commits,
        task_code,
        task_files,
        task_surface: tasks::task_surface(input),
        task_facts,
        attempt_facts,
    }
}
