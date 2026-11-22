//! The statements of the task world.

use wg_model::Table;
use wg_rows::task_world_sql::statements;
use wg_types::Id;

#[test]
fn the_task_world_loads_the_workspaces_repository_remotes() {
    let workspace = Id::nil();
    let remotes: Vec<_> = statements(Some(workspace), &[])
        .into_iter()
        .filter(|(table, _)| *table == Table::RepositoryRemote)
        .collect();
    assert_eq!(
        remotes.len(),
        1,
        "a tracker sync resolves pull requests by remote"
    );
    assert!(
        remotes[0]
            .1
            .sql
            .contains("\"repository_id\" IN (SELECT \"id\" FROM \"repositories\"")
    );
}
