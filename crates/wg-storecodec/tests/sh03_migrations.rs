//! Migration chain values (SH-03): the revision list links each revision to its predecessor, every
//! DDL file splits into statements, and the `0009` data step turns legacy `external_refs` into
//! `task_links` insert statements. No database is involved.

use std::error::Error;

use wg_storecodec::Raw;
use wg_storecodec::migrations::{REVISIONS, external_ref_links, head_revision, upgrade_path};
use wg_types::{Id, Timestamp, WgError};

type TestResult = Result<(), Box<dyn Error>>;

const TASK: &str = "tasks.id.1";

fn now() -> Result<Timestamp, WgError> {
    Timestamp::from_components(2026, 1, 2, 3, 4, 5, 6)
}

/// A fresh id per call, counting from 1.
fn counter() -> impl FnMut() -> Id {
    let mut next = 0u128;
    move || {
        next += 1;
        Id::from_u128(next)
    }
}

/// The `(title, normalized_url)` of each statement's parameters, in order.
fn links(statements: &[wg_storecodec::Statement]) -> Vec<(String, String)> {
    statements
        .iter()
        .map(
            |statement| match (&statement.params[10], &statement.params[5]) {
                (Raw::Text(title), Raw::Text(normalized)) => (title.clone(), normalized.clone()),
                other => panic!("unexpected parameters {other:?}"),
            },
        )
        .collect()
}

#[test]
fn revisions_chain_from_0001_to_0011() -> TestResult {
    assert_eq!(REVISIONS.len(), 11);
    assert_eq!(REVISIONS[0].down, None);
    for pair in REVISIONS.windows(2) {
        assert_eq!(pair[1].down, Some(pair[0].id), "down of {}", pair[1].id);
    }
    assert_eq!(head_revision(), Some("0011"));
    let data: Vec<&str> = REVISIONS
        .iter()
        .filter(|revision| revision.data.is_some())
        .map(|revision| revision.id)
        .collect();
    assert_eq!(data, ["0009"]);
    Ok(())
}

#[test]
fn every_revision_has_statements_and_a_path_from_the_one_before() -> TestResult {
    for revision in REVISIONS {
        assert!(
            !revision.statements().is_empty(),
            "{} has no statements",
            revision.id
        );
    }
    let path = upgrade_path(Some("0008"), "head")?;
    assert_eq!(
        path.iter().map(|r| r.id).collect::<Vec<_>>(),
        ["0009", "0010", "0011"]
    );
    assert!(upgrade_path(Some("0011"), "head")?.is_empty());
    assert!(upgrade_path(Some("0005"), "0003").is_err());
    Ok(())
}

#[test]
fn external_refs_yield_one_link_per_distinct_system_and_id() -> TestResult {
    let raw = r#"[
        {"system": "github", "id": "1", "url": "https://github.example/1"},
        {"system": "github", "id": "1", "url": "https://github.example/1"},
        {"system": "jira", "id": "PROJ-2"},
        {"system": "", "id": "3"},
        {"system": "github", "id": null},
        "not an object",
        {"system": "github", "id": 7, "url": null}
    ]"#;
    let tasks = [(TASK.to_owned(), Some(raw.to_owned()))];
    let statements = external_ref_links(&tasks, now()?, &mut counter())?;
    assert_eq!(
        links(&statements),
        [
            ("1".to_owned(), "github:1".to_owned()),
            ("PROJ-2".to_owned(), "jira:PROJ-2".to_owned()),
            ("7".to_owned(), "github:7".to_owned()),
        ]
    );
    let first = &statements[0];
    assert_eq!(first.params[1], Raw::Text(TASK.to_owned()));
    assert_eq!(
        first.params[4],
        Raw::Text("https://github.example/1".to_owned())
    );
    assert_eq!(first.params[7], Raw::Null);
    assert_eq!(first.params[8], Raw::Null);
    assert_eq!(first.params[9], Raw::Text(r#"["migrated"]"#.to_owned()));
    assert_eq!(first.params[11], Raw::Text("system:migration".to_owned()));
    assert_eq!(
        first.params[12],
        Raw::Text("2026-01-02T03:04:05.000006+00:00".to_owned())
    );
    assert_eq!(statements[2].params[4], Raw::Text(String::new()));
    Ok(())
}

#[test]
fn empty_and_non_list_refs_yield_nothing() -> TestResult {
    let tasks = [
        (TASK.to_owned(), None),
        (TASK.to_owned(), Some(String::new())),
        (TASK.to_owned(), Some("[]".to_owned())),
        (
            TASK.to_owned(),
            Some(r#"{"system": "github", "id": "1"}"#.to_owned()),
        ),
    ];
    assert!(external_ref_links(&tasks, now()?, &mut counter())?.is_empty());
    Ok(())
}

#[test]
fn malformed_refs_fail_the_migration() -> TestResult {
    let tasks = [(TASK.to_owned(), Some("[oops".to_owned()))];
    assert!(external_ref_links(&tasks, now()?, &mut counter()).is_err());
    let tasks = [(TASK.to_owned(), Some("7".to_owned()))];
    assert!(external_ref_links(&tasks, now()?, &mut counter()).is_err());
    Ok(())
}
