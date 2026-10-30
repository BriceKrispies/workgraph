//! Migration runner checks (SH-02): a database migrated by the runner has the `sqlite_master` the
//! Python-migrated fixture has, the runner reads and writes `alembic_version` as Alembic does, and
//! it opens a database Python migrated.

use std::error::Error;
use std::path::{Path, PathBuf};

use serde_json::Value;
use wg_conformance::dbdump::canonical_dump;
use wg_store::{Location, Store, head_revision};
use wg_types::ErrorCode;

type TestResult = Result<(), Box<dyn Error>>;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-sh02-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The `schema` section of a canonical dump.
fn schema_of(database: &Path) -> Result<Value, Box<dyn Error>> {
    let dump: Value = serde_json::from_slice(&canonical_dump(database)?)?;
    Ok(dump["schema"].clone())
}

#[test]
fn fresh_database_matches_the_python_schema_at_0001() -> TestResult {
    let dir = scratch("fresh");
    let path = dir.join("wg.db");
    let mut store = Store::open(&Location::File(path.clone()))?;
    assert_eq!(store.current_revision()?, None);
    assert_eq!(store.upgrade("0001")?, vec!["0001"]);
    assert_eq!(store.current_revision()?.as_deref(), Some("0001"));
    drop(store);

    let expected: Value =
        serde_json::from_slice(&std::fs::read(root().join("golden/db/dump/rev-0001.json"))?)?;
    assert_eq!(schema_of(&path)?, expected["schema"]);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}

#[test]
fn upgrade_is_idempotent_and_head_resolves() -> TestResult {
    let mut store = Store::open(&Location::Memory)?;
    let all: Vec<String> = (1..=11).map(|n| format!("{n:04}")).collect();
    assert_eq!(
        store.upgrade("head")?,
        all.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert!(store.upgrade("head")?.is_empty());
    assert!(store.is_up_to_date()?);
    assert_eq!(head_revision(), Some("0011"));
    Ok(())
}

#[test]
fn unknown_revision_is_an_error_and_changes_nothing() -> TestResult {
    let mut store = Store::open(&Location::Memory)?;
    let err = store.upgrade("9999").err().ok_or("expected an error")?;
    assert_eq!(err.code, ErrorCode::WorkgraphError);
    assert_eq!(store.current_revision()?, None);
    Ok(())
}

#[test]
fn opens_a_database_python_migrated() -> TestResult {
    let dir = scratch("python");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("wg.db");
    std::fs::copy(root().join("golden/db/rev-0001.db"), &path)?;
    let mut store = Store::open(&Location::File(path))?;
    assert_eq!(store.current_revision()?.as_deref(), Some("0001"));
    assert!(store.upgrade("0001")?.is_empty());
    assert_eq!(store.upgrade("head")?.len(), 10);
    assert!(store.is_up_to_date()?);
    drop(store);
    std::fs::remove_dir_all(&dir)?;
    Ok(())
}
