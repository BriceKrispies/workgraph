//! wg-storecodec checks for structured decisions: the `task_decisions` row codec and the query
//! builders. No database is involved; the SQLite round trip lives in wg-store's tests.

use std::error::Error;

use wg_model::Table;
use wg_model::entity::{DecisionOption, TaskDecision};
use wg_storecodec::decision_sql::{
    DecisionFilter, decision_by_id, decisions_by_status, decisions_for_task, open_decision_count,
};
use wg_storecodec::{Raw, TableRow, columns};
use wg_types::{Id, Timestamp, parse_id};

type TestResult = Result<(), Box<dyn Error>>;

fn id(text: &str) -> Result<Id, Box<dyn Error>> {
    Ok(parse_id(text)?)
}

fn decision() -> Result<TaskDecision, Box<dyn Error>> {
    let at = Timestamp::from_components(2026, 10, 25, 12, 0, 0, 7)?;
    Ok(TaskDecision {
        id: id("22222222-2222-4222-8222-222222222222")?,
        workspace_id: id("11111111-1111-4111-8111-111111111111")?,
        task_id: id("33333333-3333-4333-8333-333333333333")?,
        question: "Which store?".into(),
        options: vec![
            DecisionOption {
                id: "a".into(),
                label: "SQLite".into(),
                detail: "Local".into(),
                recommended: true,
            },
            DecisionOption {
                id: "b".into(),
                label: "Postgres".into(),
                detail: String::new(),
                recommended: false,
            },
        ]
        .into_iter()
        .collect(),
        status: "open".into(),
        chosen_option_id: None,
        other_text: None,
        note: None,
        decided_by: None,
        decided_at: None,
        created_by: "human:brice".into(),
        created_at: at,
        updated_at: at,
        version: 1,
    })
}

#[test]
fn decision_rows_encode_options_as_a_json_list_in_column_order() -> TestResult {
    let row = decision()?;
    let values = row.values()?;
    assert_eq!(values.len(), columns(Table::TaskDecision).len());
    assert_eq!(
        values[4],
        Raw::Text(
            "[{\"detail\":\"Local\",\"id\":\"a\",\"label\":\"SQLite\",\"recommended\":true},\
             {\"detail\":\"\",\"id\":\"b\",\"label\":\"Postgres\",\"recommended\":false}]"
                .into()
        )
    );
    assert_eq!(TaskDecision::from_raw(&values)?, row);
    Ok(())
}

#[test]
fn decision_options_with_a_bad_shape_are_refused() -> TestResult {
    let mut values = decision()?.values()?;
    values[4] = Raw::Text("[{\"id\":\"a\"}]".into());
    assert!(TaskDecision::from_raw(&values).is_err());
    values[4] = Raw::Text("{}".into());
    assert!(TaskDecision::from_raw(&values).is_err());
    Ok(())
}

#[test]
fn decision_rows_keep_the_nullable_answer_columns() -> TestResult {
    let mut row = decision()?;
    row.status = "decided".into();
    row.chosen_option_id = Some("b".into());
    row.decided_by = Some("human:brice".into());
    row.decided_at = Some(Timestamp::from_components(2026, 10, 25, 13, 0, 0, 0)?);
    row.version = 2;
    let back = TaskDecision::from_raw(&row.values()?)?;
    assert_eq!(back, row);
    Ok(())
}

#[test]
fn decisions_for_a_task_are_oldest_first() -> TestResult {
    let statement = decisions_for_task(id("33333333-3333-4333-8333-333333333333")?);
    assert_eq!(
        statement.sql,
        "SELECT d.\"id\", d.\"workspace_id\", d.\"task_id\", d.\"question\", d.\"options\", \
         d.\"status\", d.\"chosen_option_id\", d.\"other_text\", d.\"note\", d.\"decided_by\", \
         d.\"decided_at\", d.\"created_by\", d.\"created_at\", d.\"updated_at\", d.\"version\" \
         FROM \"task_decisions\" AS d WHERE d.\"task_id\" = ?1 \
         ORDER BY d.\"created_at\" ASC, d.\"id\" ASC"
    );
    assert_eq!(statement.params.len(), 1);
    Ok(())
}

#[test]
fn open_decisions_order_by_task_priority_then_age_and_skip_archived_or_cancelled_tasks()
-> TestResult {
    let ws = id("11111111-1111-4111-8111-111111111111")?;
    let statement = decisions_by_status(ws, DecisionFilter::Open, 200);
    assert!(
        statement
            .sql
            .contains("JOIN \"tasks\" AS t ON t.\"id\" = d.\"task_id\"")
    );
    assert!(
        statement
            .sql
            .contains("t.\"archived_at\" IS NULL AND t.\"status\" <> ?3")
    );
    assert!(statement.sql.contains(
        "CASE t.\"priority\" WHEN 'critical' THEN 0 WHEN 'high' THEN 1 \
         WHEN 'medium' THEN 2 WHEN 'low' THEN 3 ELSE 9 END ASC, d.\"created_at\" ASC"
    ));
    assert!(statement.sql.ends_with("LIMIT ?4"));
    assert_eq!(statement.params.len(), 4);
    assert_eq!(statement.params[1], Raw::Text("open".into()));
    assert_eq!(statement.params[2], Raw::Text("cancelled".into()));
    assert_eq!(statement.params[3], Raw::Int(200));
    Ok(())
}

#[test]
fn decided_decisions_are_newest_answer_first() -> TestResult {
    let ws = id("11111111-1111-4111-8111-111111111111")?;
    let statement = decisions_by_status(ws, DecisionFilter::Decided, 50);
    assert!(
        statement
            .sql
            .ends_with("ORDER BY d.\"decided_at\" DESC, d.\"id\" ASC LIMIT ?3")
    );
    assert_eq!(
        statement.params,
        [
            Raw::Text(ws.to_string()),
            Raw::Text("decided".into()),
            Raw::Int(50),
        ]
    );
    Ok(())
}

#[test]
fn withdrawn_and_all_decisions_are_newest_first_and_all_has_no_status_filter() -> TestResult {
    let ws = id("11111111-1111-4111-8111-111111111111")?;
    let withdrawn = decisions_by_status(ws, DecisionFilter::Withdrawn, 10);
    assert!(withdrawn.sql.contains("d.\"status\" = ?2"));
    assert!(
        withdrawn
            .sql
            .ends_with("ORDER BY d.\"created_at\" DESC, d.\"id\" ASC LIMIT ?3")
    );
    assert_eq!(withdrawn.params.len(), 3);

    let all = decisions_by_status(ws, DecisionFilter::All, 10);
    assert!(!all.sql.contains("d.\"status\" = ?"));
    assert!(all.sql.ends_with("LIMIT ?2"));
    assert_eq!(all.params, [Raw::Text(ws.to_string()), Raw::Int(10)]);
    Ok(())
}

#[test]
fn get_by_id_is_scoped_to_the_workspace_and_open_count_to_the_task() -> TestResult {
    let ws = id("11111111-1111-4111-8111-111111111111")?;
    let did = id("22222222-2222-4222-8222-222222222222")?;
    let by_id = decision_by_id(ws, did);
    assert!(
        by_id
            .sql
            .ends_with("WHERE d.\"id\" = ?1 AND d.\"workspace_id\" = ?2")
    );
    assert_eq!(
        by_id.params,
        [Raw::Text(did.to_string()), Raw::Text(ws.to_string())]
    );

    let count = open_decision_count(did);
    assert_eq!(
        count.sql,
        "SELECT COUNT(*) FROM \"task_decisions\" WHERE \"task_id\" = ?1 AND \"status\" = ?2"
    );
    assert_eq!(
        count.params,
        [Raw::Text(did.to_string()), Raw::Text("open".into())]
    );
    Ok(())
}

#[test]
fn brief_rows_decode_the_decision_then_the_task_brief() -> TestResult {
    use wg_storecodec::decision_sql::{
        DecisionFilter, attach_tags, decisions_by_status_with_task, decode_decision_with_task,
        decode_task_tag, task_tags_of,
    };
    let ws = id("11111111-1111-4111-8111-111111111111")?;
    let statement = decisions_by_status_with_task(ws, DecisionFilter::Open, 5);
    assert!(
        statement
            .sql
            .contains("t.\"key\", t.\"title\", t.\"status\", t.\"priority\", t.\"description\"")
    );
    assert!(statement.sql.ends_with("LIMIT ?4"));

    let mut values = decision()?.values()?;
    values.extend([
        Raw::Text("WG-7".into()),
        Raw::Text("title".into()),
        Raw::Text("ready".into()),
        Raw::Text("high".into()),
        Raw::Text("body".into()),
    ]);
    let rows = vec![decode_decision_with_task(&values)?];
    assert_eq!(rows[0].task.key.to_string(), "WG-7");
    assert_eq!(rows[0].task.status, wg_types::TaskStatus::Ready);
    assert_eq!(rows[0].task.priority, wg_types::TaskPriority::High);
    assert!(rows[0].task.tags.is_empty());

    let tag_row = [
        Raw::Text(decision()?.task_id.to_string()),
        Raw::Text("ops".into()),
    ];
    let tag = decode_task_tag(&tag_row)?;
    let rows = attach_tags(&rows, &[tag]);
    assert_eq!(rows[0].task.tags, vec!["ops".to_owned()]);

    let empty = task_tags_of(&[]);
    assert!(empty.sql.contains("1 = 0") && empty.params.is_empty());
    let two = task_tags_of(&[ws, id("22222222-2222-4222-8222-222222222222")?]);
    assert!(
        two.sql
            .ends_with("IN (?1, ?2) ORDER BY \"task_id\", \"tag\"")
    );
    Ok(())
}
