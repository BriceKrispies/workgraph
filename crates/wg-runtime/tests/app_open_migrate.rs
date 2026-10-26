//! `App::open` migrates the database to head as `Workgraph.__init__` does with `auto_migrate`:
//! a fresh home is created at head, an older database is upgraded in place with its rows kept, and
//! a database already at head is left alone.

use std::error::Error;
use std::path::{Path, PathBuf};

use wg_runtime::App;
use wg_store::{Location, Raw, Session, Statement, Store};
use wg_types::{ErrorCode, WgError};

type TestResult = Result<(), Box<dyn Error>>;

/// The head revision (`schema-0011`, the last of the migration chain).
const HEAD: &str = "0011";

/// A fresh scratch directory for one test.
fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("app_open_migrate")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// A Python-migrated golden database.
fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/db")
        .join(name)
}

/// Where `App::open(home)` keeps the database.
fn database(home: &Path) -> PathBuf {
    home.join("data").join("workgraph.db")
}

/// The revision a database file records, read through the store.
fn revision(path: &Path) -> Result<Option<String>, WgError> {
    let mut store = Store::open(&Location::File(path.to_path_buf()))?;
    store.current_revision()
}

fn workspace_count(app: &mut App) -> Result<i64, WgError> {
    app.runtime.store.read(|session: &mut Session<'_>| {
        match session
            .rows(&Statement {
                sql: "SELECT COUNT(*) FROM workspaces".to_owned(),
                params: Vec::new(),
            })?
            .first()
            .and_then(|row| row.first())
        {
            Some(Raw::Int(n)) => Ok(*n),
            _ => Err(WgError::new(ErrorCode::WorkgraphError, "no count")),
        }
    })
}

#[test]
fn fresh_home_is_created_at_head() -> TestResult {
    let home = scratch("fresh")?;
    let mut app = App::open(Some(&home))?;
    assert_eq!(app.runtime.store.current_revision()?.as_deref(), Some(HEAD));
    assert!(app.runtime.store.is_up_to_date()?);
    assert_eq!(workspace_count(&mut app)?, 0);
    drop(app);

    // Opening a database that is already at head runs no migration and keeps its rows.
    let mut app = App::open(Some(&home))?;
    assert!(app.runtime.store.is_up_to_date()?);
    assert_eq!(workspace_count(&mut app)?, 0);
    Ok(())
}

#[test]
fn older_database_is_migrated_to_head_with_its_rows() -> TestResult {
    let home = scratch("older")?;
    std::fs::create_dir_all(home.join("data"))?;
    std::fs::copy(golden("rev-0005.db"), database(&home))?;
    assert_eq!(revision(&database(&home))?.as_deref(), Some("0005"));

    let mut app = App::open(Some(&home))?;
    assert_eq!(app.runtime.store.current_revision()?.as_deref(), Some(HEAD));
    assert!(app.runtime.store.is_up_to_date()?);
    assert_eq!(workspace_count(&mut app)?, 2);
    Ok(())
}
