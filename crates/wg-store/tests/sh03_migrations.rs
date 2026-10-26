//! Migration chain checks (SH-03): every CF-05 fixture database migrates to head with the schema
//! the Python chain produced, keeps its rows, and gets the `0009` link rows from its legacy
//! `external_refs`.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use wg_conformance::dbdump::canonical_dump;
use wg_store::{Location, Store, head_revision};

type TestResult = Result<(), Box<dyn Error>>;

const FIXTURES: [&str; 11] = [
    "0001", "0002", "0003", "0004", "0005", "0006", "0007", "0008", "0009", "0010", "0011",
];

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-sh03-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("scratch dir: {err}"));
    dir
}

/// Copies fixture `revision` to a scratch database and returns its path.
fn copy_fixture(revision: &str, dir: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let path = dir.join(format!("rev-{revision}.db"));
    std::fs::copy(root().join(format!("golden/db/rev-{revision}.db")), &path)?;
    Ok(path)
}

/// The canonical dump of `database` as JSON.
fn dump_of(database: &Path) -> Result<Value, Box<dyn Error>> {
    Ok(serde_json::from_slice(&canonical_dump(database)?)?)
}

/// The `schema` section of the Python-written head dump.
fn python_head_schema() -> Result<Value, Box<dyn Error>> {
    let text = std::fs::read(root().join("golden/db/dump/rev-0011.json"))?;
    let dump: Value = serde_json::from_slice(&text)?;
    Ok(dump["schema"].clone())
}

fn table<'a>(dump: &'a Value, name: &str) -> Result<&'a Value, Box<dyn Error>> {
    dump["tables"]
        .get(name)
        .ok_or_else(|| format!("table {name} missing").into())
}

fn columns_of(table: &Value) -> BTreeSet<String> {
    table["columns"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|column| column.as_str().map(str::to_owned))
        .collect()
}

#[test]
fn every_fixture_migrates_to_head_with_the_python_schema() -> TestResult {
    let expected = python_head_schema()?;
    let dir = scratch("head");
    for revision in FIXTURES {
        let path = copy_fixture(revision, &dir)?;
        let mut store = Store::open(&Location::File(path.clone()))?;
        assert_eq!(store.current_revision()?.as_deref(), Some(revision));
        let ran = store.upgrade("head")?;
        assert_eq!(store.current_revision()?.as_deref(), head_revision());
        assert!(store.is_up_to_date()?);
        drop(store);
        let after = (revision.parse::<u32>()? + 1..=11)
            .map(|n| format!("{n:04}"))
            .collect::<Vec<_>>();
        assert_eq!(ran, after, "revisions run from {revision}");
        let dump = dump_of(&path)?;
        assert_eq!(dump["schema"], expected, "schema from {revision} to head");
    }
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn head_fixture_is_untouched_by_upgrade() -> TestResult {
    let dir = scratch("untouched");
    let path = copy_fixture("0011", &dir)?;
    let mut store = Store::open(&Location::File(path.clone()))?;
    assert!(store.upgrade("head")?.is_empty());
    drop(store);
    let golden = std::fs::read(root().join("golden/db/dump/rev-0011.json"))?;
    assert_eq!(canonical_dump(&path)?, golden);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn migrations_keep_every_row_they_do_not_rewrite() -> TestResult {
    for revision in FIXTURES {
        let dir = scratch(&format!("rows-{revision}"));
        let source = dump_of(&root().join(format!("golden/db/rev-{revision}.db")))?;
        let path = copy_fixture(revision, &dir)?;
        let mut store = Store::open(&Location::File(path.clone()))?;
        store.upgrade("head")?;
        drop(store);
        let migrated = dump_of(&path)?;

        for (name, source_table) in source["tables"].as_object().into_iter().flatten() {
            let target = table(&migrated, name)?;
            // Only the columns both sides have: `0009` drops `tasks.external_refs`.
            let kept: BTreeSet<String> = columns_of(source_table)
                .intersection(&columns_of(target))
                .cloned()
                .collect();
            let project = |row: &Value| -> Value {
                let object: Map<String, Value> = row
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(column, _)| kept.contains(*column))
                    .map(|(column, value)| (column.clone(), value.clone()))
                    .collect();
                Value::Object(object)
            };
            let migrated_rows: Vec<Value> = target["rows"]
                .as_array()
                .into_iter()
                .flatten()
                .map(project)
                .collect();
            for row in source_table["rows"].as_array().into_iter().flatten() {
                assert!(
                    migrated_rows.contains(&project(row)),
                    "{revision}: row of {name} lost by migration: {row}"
                );
            }
        }
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

#[test]
fn external_refs_become_task_links_at_0009() -> TestResult {
    for revision in FIXTURES {
        let dir = scratch(&format!("links-{revision}"));
        let source = dump_of(&root().join(format!("golden/db/rev-{revision}.db")))?;
        // One link per distinct `system:id` of each task's legacy refs (none once the column is gone).
        let mut expected = 0usize;
        for row in table(&source, "tasks")?["rows"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let distinct: BTreeSet<String> = row
                .get("external_refs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|entry| {
                    let system = entry.get("system")?.as_str()?;
                    let id = entry.get("id")?.as_str()?;
                    (!system.is_empty() && !id.is_empty()).then(|| format!("{system}:{id}"))
                })
                .collect();
            expected += distinct.len();
        }

        let path = copy_fixture(revision, &dir)?;
        let mut store = Store::open(&Location::File(path.clone()))?;
        store.upgrade("head")?;
        drop(store);
        let migrated = dump_of(&path)?;

        assert!(!columns_of(table(&migrated, "tasks")?).contains("external_refs"));
        let links: Vec<&Value> = table(&migrated, "task_links")?["rows"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|row| row["created_by"] == "system:migration")
            .collect();
        assert_eq!(links.len(), expected, "link rows from {revision}");
        if revision <= "0008" {
            assert!(expected > 0, "fixture {revision} should carry legacy refs");
        }
        for link in links {
            assert_eq!(link["kind"], "external_ref");
            assert_eq!(link["sources"], Value::Array(vec!["migrated".into()]));
            assert_eq!(link["repo_full_name"], "");
            assert_eq!(
                link["normalized_url"],
                format!(
                    "{}:{}",
                    link["provider"].as_str().unwrap_or_default(),
                    link["title"].as_str().unwrap_or_default()
                )
            );
        }
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}
