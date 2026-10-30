//! wg-rows checks: column lists against the schema contract, statement text, event
//! decoding and queries. No database is involved.

use std::error::Error;
use std::path::PathBuf;

use wg_model::entity::{TaskTag, Workspace};
use wg_model::{JsonObject, Row, RowKey, Table, Write};
use wg_rows::{
    EventQuery, Raw, TableRow, columns, decode_event, primary_key, select_by_key, update_failure,
    write_statement,
};
use wg_types::{ErrorCode, Timestamp, parse_id};

type TestResult = Result<(), Box<dyn Error>>;

/// The column names of `table` in `contracts/schema-0011.sql`, in declaration order.
fn schema_columns(table: Table) -> Result<Vec<String>, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/schema-0011.sql");
    let text = std::fs::read_to_string(path)?;
    let start = format!("CREATE TABLE {} (", table.name());
    let body = text
        .split("\n\n")
        .find(|block| {
            block.starts_with(&start)
                || block.starts_with(&format!("CREATE TABLE \"{}\" (", table.name()))
        })
        .ok_or_else(|| format!("table {} not in schema", table.name()))?;
    Ok(body
        .lines()
        .skip(1)
        .map(str::trim)
        .take_while(|line| !line.starts_with("CONSTRAINT") && !line.starts_with(')'))
        .filter_map(|line| line.split_whitespace().next())
        .map(|name| name.trim_matches('"').to_owned())
        .collect())
}

/// The `PRIMARY KEY (...)` columns of `table` in the schema.
fn schema_primary_key(table: Table) -> Result<Vec<String>, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/schema-0011.sql");
    let text = std::fs::read_to_string(path)?;
    let start = format!("CREATE TABLE {} (", table.name());
    let quoted = format!("CREATE TABLE \"{}\" (", table.name());
    let body = text
        .split(
            "

",
        )
        .find(|block| block.starts_with(&start) || block.starts_with(&quoted))
        .ok_or_else(|| format!("table {} not in schema", table.name()))?;
    let line = body
        .lines()
        .find(|line| line.contains("PRIMARY KEY ("))
        .ok_or("no primary key")?;
    let inner = line.split("PRIMARY KEY (").nth(1).ok_or("malformed")?;
    Ok(inner
        .trim_end_matches([')', ',', ' '])
        .split(", ")
        .map(str::to_owned)
        .collect())
}

#[test]
fn column_lists_follow_the_schema_contract() -> TestResult {
    for table in Table::ALL {
        let expected = schema_columns(*table)?;
        let actual: Vec<String> = columns(*table).iter().map(|c| (*c).to_owned()).collect();
        assert_eq!(actual, expected, "{}", table.name());
        assert_eq!(
            primary_key(*table)
                .iter()
                .map(|c| (*c).to_owned())
                .collect::<Vec<_>>(),
            schema_primary_key(*table)?,
            "{}",
            table.name()
        );
    }
    Ok(())
}

fn workspace() -> Result<Workspace, Box<dyn Error>> {
    let at = Timestamp::from_components(2026, 10, 25, 12, 0, 0, 7)?;
    Ok(Workspace {
        id: parse_id("11111111-1111-4111-8111-111111111111")?,
        slug: "s".into(),
        name: "n".into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 1,
        settings: JsonObject::new(),
        archived_at: None,
        created_at: at,
        updated_at: at,
    })
}

#[test]
fn rows_encode_and_decode_in_column_order() -> TestResult {
    let row = workspace()?;
    let values = row.values()?;
    assert_eq!(values.len(), columns(Table::Workspace).len());
    assert_eq!(values[6], Raw::Text("{}".into()));
    assert_eq!(
        values[9],
        Raw::Text("2026-10-25T12:00:00.000007+00:00".into())
    );
    assert_eq!(Workspace::from_raw(&values)?, row);
    assert!(Workspace::from_raw(&values[..3]).is_err());
    Ok(())
}

#[test]
fn write_statements_name_the_key_columns() -> TestResult {
    let row = workspace()?;
    let insert = write_statement(&Write::insert(row.clone()))?;
    assert_eq!(
        insert.sql,
        "INSERT INTO \"workspaces\" (\"id\", \"slug\", \"name\", \"description\", \"key_prefix\", \
         \"next_task_number\", \"settings\", \"archived_at\", \"created_at\", \"updated_at\") \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"
    );
    assert_eq!(insert.params.len(), 10);

    let update = write_statement(&Write::update_versioned(row.clone(), 4))?;
    assert!(
        update
            .sql
            .starts_with("UPDATE \"workspaces\" SET \"slug\" = ?2,")
    );
    assert!(
        update
            .sql
            .ends_with("WHERE \"id\" = ?1 AND \"version\" = ?11")
    );
    assert_eq!(update.params.last(), Some(&Raw::Int(4)));

    // A table made only of key columns still gets an UPDATE that reports whether the row exists.
    let tag = TaskTag {
        task_id: row.id,
        tag: "t".into(),
    };
    let update = write_statement(&Write::update(tag))?;
    assert_eq!(
        update.sql,
        "UPDATE \"task_tags\" SET \"task_id\" = \"task_id\" WHERE \"task_id\" = ?1 AND \"tag\" = ?2"
    );

    let key = RowKey::TaskTag {
        task_id: row.id,
        tag: "t".into(),
    };
    let delete = write_statement(&Write::delete(key.clone()))?;
    assert_eq!(
        delete.sql,
        "DELETE FROM \"task_tags\" WHERE \"task_id\" = ?1 AND \"tag\" = ?2"
    );
    assert_eq!(select_by_key(&key).params, delete.params);
    Ok(())
}

#[test]
fn failed_updates_are_classified() {
    let guarded = update_failure(Table::Task, Some(1), Some(2));
    assert_eq!(guarded.code, ErrorCode::VersionConflict);
    assert_eq!(
        update_failure(Table::Task, Some(1), None).code,
        ErrorCode::NotFound
    );
    assert_eq!(
        update_failure(Table::Task, None, None).code,
        ErrorCode::NotFound
    );
    let _ = Row::Workspace;
}

#[test]
fn event_queries_number_their_placeholders() -> TestResult {
    let ws = parse_id("11111111-1111-4111-8111-111111111111")?;
    let none = EventQuery::default().statement();
    assert!(none.sql.ends_with("FROM events ORDER BY seq ASC LIMIT ?1"));
    assert_eq!(none.params, vec![Raw::Int(200)]);

    let query = EventQuery {
        event_types: vec!["a".into(), "b".into()],
        descending: true,
        limit: 5,
        ..EventQuery::after(ws, 9)
    };
    let statement = query.statement();
    assert!(statement.sql.contains(
        "WHERE workspace_id = ?1 AND event_type IN (?2, ?3) AND seq > ?4 ORDER BY seq DESC LIMIT ?5"
    ));
    assert_eq!(statement.params.len(), 5);
    assert_eq!(statement.params[3], Raw::Int(9));
    Ok(())
}

#[test]
fn old_task_created_events_are_upcast() -> TestResult {
    let id = Raw::Text("22222222-2222-4222-8222-222222222222".into());
    let text = |s: &str| Raw::Text(s.into());
    let row = vec![
        Raw::Int(3),
        id.clone(),
        Raw::Null,
        text("task.created"),
        text("task"),
        id.clone(),
        text("human:b"),
        text("human"),
        text("2026-10-25T12:00:00.000000+00:00"),
        id,
        Raw::Null,
        Raw::Null,
        Raw::Int(1),
        text(r#"{"tags":" a, b ,,c"}"#),
    ];
    let record = decode_event(&row)?;
    assert_eq!(record.schema_version, 2);
    assert_eq!(
        serde_json::Value::Object(record.data),
        serde_json::json!({"tags": ["a", "b", "c"], "acceptance_criteria": []})
    );
    Ok(())
}

#[test]
fn an_entity_id_that_is_not_an_id_reads_back_as_text() -> TestResult {
    let id = Raw::Text("22222222-2222-4222-8222-222222222222".into());
    let text = |s: &str| Raw::Text(s.into());
    let row = vec![
        Raw::Int(4),
        id.clone(),
        Raw::Null,
        text("worktree.recovered"),
        text("system"),
        text("worktrees"),
        text("system:cli"),
        text("system"),
        text("2026-10-25T12:00:00.000000+00:00"),
        id,
        Raw::Null,
        Raw::Null,
        Raw::Int(1),
        text("{}"),
    ];
    let record = decode_event(&row)?;
    assert_eq!(record.entity_id_text(), "worktrees");
    assert_eq!(record.to_dict()["entity_id"], "worktrees");
    Ok(())
}
