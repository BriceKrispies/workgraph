//! Backup, restore, workspace export and import (SH-20) against real SQLite databases, with the
//! Python-written fixtures in `golden/archive/`.

use std::error::Error;
use std::path::{Path, PathBuf};

use wg_archive::{backup, transfer};
use wg_config::paths::{AppPaths, Environment, Os};
use wg_store::{Location, Store, head_revision};
use wg_types::{ErrorCode, Timestamp};

type TestResult = Result<(), Box<dyn Error>>;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/archive")
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("wg-sh20-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The paths of a Workgraph home, with its directories created as `App::open` creates them.
fn home_paths(home: &Path) -> Result<AppPaths, Box<dyn Error>> {
    let os = if cfg!(windows) {
        Os::Windows
    } else {
        Os::Linux
    };
    let paths = AppPaths::resolve(&Environment {
        os,
        workgraph_home: Some(home.to_string_lossy().into_owned()),
        home: None,
        local_appdata: None,
        xdg_config_home: None,
        xdg_data_home: None,
        xdg_state_home: None,
    })?;
    for dir in [
        paths.config_dir(),
        paths.workspace_config_dir(),
        paths.data_dir(),
        paths.artifacts_dir(),
        paths.backups_dir(),
    ] {
        std::fs::create_dir_all(dir.as_str())?;
    }
    Ok(paths)
}

fn open_at(path: &Path) -> Result<Store, Box<dyn Error>> {
    Ok(Store::open(&Location::File(path.to_path_buf()))?)
}

fn migrated_at(path: &Path) -> Result<Store, Box<dyn Error>> {
    let mut store = open_at(path)?;
    store.upgrade(head_revision().ok_or("no revisions")?)?;
    Ok(store)
}

fn at() -> Result<Timestamp, Box<dyn Error>> {
    Ok(Timestamp::parse_iso("2026-10-26T05:07:35+00:00")?)
}

/// The workspace id on the first line of the Python export fixture.
/// Compares an export with the Python fixture byte for byte. Python ends its lines with `\r\n` on
/// Windows, where the fixture was written, and with `\n` elsewhere, so off Windows the carriage
/// returns are ignored.
fn assert_same_export(actual: &Path) -> TestResult {
    let actual = std::fs::read(actual)?;
    let expected = std::fs::read(golden("workspace-export.jsonl"))?;
    if cfg!(windows) {
        assert_eq!(actual, expected);
    } else {
        let strip = |bytes: Vec<u8>| {
            bytes
                .into_iter()
                .filter(|b| *b != b'\r')
                .collect::<Vec<u8>>()
        };
        assert_eq!(strip(actual), strip(expected));
    }
    Ok(())
}

fn fixture_workspace() -> Result<String, Box<dyn Error>> {
    let text = std::fs::read_to_string(golden("workspace-export.jsonl"))?;
    let header = text.lines().next().ok_or("empty fixture")?;
    Ok(wg_archivespec::parse_header(header)?)
}

#[test]
fn python_export_imports_and_exports_back_byte_for_byte() -> TestResult {
    let dir = scratch("reexport");
    let mut store = migrated_at(&dir.join("wg.db"))?;
    let report = transfer::import_workspace(&mut store, &golden("workspace-export.jsonl"))?;
    let expected = [
        ("events", 6),
        ("jobs", 6),
        ("workspaces", 1),
        ("tasks", 3),
        ("task_files", 3),
        ("task_tags", 4),
    ];
    let counts: Vec<(String, usize)> = report.tables.clone().into_iter().collect();
    let mut want: Vec<(String, usize)> =
        expected.iter().map(|(n, c)| (n.to_string(), *c)).collect();
    want.sort();
    assert_eq!(counts, want);
    let indexed = store.read(|session| {
        session.rows(&wg_store::Statement {
            sql: "SELECT COUNT(*) FROM task_fts".to_string(),
            params: Vec::new(),
        })
    })?;
    assert_eq!(indexed, vec![vec![wg_store::Raw::Int(3)]]);
    let exported = transfer::export_workspace(
        &mut store,
        &report.workspace_id,
        &dir.join("again.jsonl"),
        "0.1.0",
    )?;
    assert_same_export(&exported.path)?;
    Ok(())
}

#[test]
fn importing_an_existing_workspace_is_refused_and_writes_nothing() -> TestResult {
    let dir = scratch("conflict");
    let mut store = migrated_at(&dir.join("wg.db"))?;
    transfer::import_workspace(&mut store, &golden("workspace-export.jsonl"))?;
    let error = match transfer::import_workspace(&mut store, &golden("workspace-export.jsonl")) {
        Err(error) => error,
        Ok(_) => return Err("second import must be refused".into()),
    };
    assert_eq!(error.code, ErrorCode::Conflict);
    Ok(())
}

#[test]
fn python_backup_restores_and_exports_the_same_records() -> TestResult {
    let dir = scratch("restore");
    let paths = home_paths(&dir.join("home"))?;
    // An empty database, as `init` leaves one, so the restore keeps a safety copy.
    drop(open_at(&PathBuf::from(paths.database_file().as_str()))?);
    let report = backup::restore_backup(&paths, &golden("backup-v1.zip"), true, false, at()?)?;
    assert_eq!(report.manifest["format"], "workgraph-backup-v1");
    assert_eq!(report.schema_after.as_deref(), Some("0011"));
    assert!(
        report
            .safety_copy
            .as_ref()
            .is_some_and(|path| path.exists())
    );
    assert!(PathBuf::from(paths.config_file().as_str()).exists());
    let mut store = open_at(&PathBuf::from(paths.database_file().as_str()))?;
    let workspace = fixture_workspace()?;
    transfer::export_workspace(&mut store, &workspace, &dir.join("restored.jsonl"), "0.1.0")?;
    assert_same_export(&dir.join("restored.jsonl"))?;
    Ok(())
}

#[test]
fn rust_backup_restores_into_another_home() -> TestResult {
    let dir = scratch("roundtrip");
    let source = home_paths(&dir.join("source"))?;
    drop(open_at(&PathBuf::from(source.database_file().as_str()))?);
    backup::restore_backup(&source, &golden("backup-v1.zip"), true, false, at()?)?;
    let mut store = open_at(&PathBuf::from(source.database_file().as_str()))?;
    let archive = dir.join("rust-backup.zip");
    let made = backup::create_backup(&source, &mut store, Some(&archive), false, at()?, "0.1.0")?;
    assert_eq!(made.schema_revision.as_deref(), Some("0011"));
    assert_eq!(made.stamp, "20261026T050735Z");
    drop(store);

    let names = {
        let zip = zip::ZipArchive::new(std::fs::File::open(&archive)?)?;
        zip.file_names().map(str::to_string).collect::<Vec<_>>()
    };
    for name in ["workgraph.db", "manifest.json", "config/config.toml"] {
        assert!(
            names.iter().any(|n| n == name),
            "missing {name} in {names:?}"
        );
    }

    let target = home_paths(&dir.join("target"))?;
    drop(open_at(&PathBuf::from(target.database_file().as_str()))?);
    let restored = backup::restore_backup(&target, &archive, false, false, at()?)?;
    assert_eq!(restored.manifest["workgraph_version"], "0.1.0");
    let mut store = open_at(&PathBuf::from(target.database_file().as_str()))?;
    transfer::export_workspace(
        &mut store,
        &fixture_workspace()?,
        &dir.join("again.jsonl"),
        "0.1.0",
    )?;
    assert_same_export(&dir.join("again.jsonl"))?;
    Ok(())
}

#[test]
fn a_file_that_is_not_an_archive_is_refused() -> TestResult {
    let dir = scratch("garbage");
    std::fs::create_dir_all(&dir)?;
    let paths = home_paths(&dir.join("home"))?;
    let fake = dir.join("fake.zip");
    std::fs::write(&fake, b"not a zip")?;
    let error = match backup::restore_backup(&paths, &fake, false, false, at()?) {
        Err(error) => error,
        Ok(_) => return Err("garbage must be refused".into()),
    };
    assert_eq!(error.code, ErrorCode::ValidationFailed);
    Ok(())
}
