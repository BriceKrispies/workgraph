//! Backups and restores (F10): `persistence/backup.py`'s `create_backup` and `restore_backup`.
//!
//! A backup is a zip holding `workgraph.db` (an online copy of the database), `config/config.toml`,
//! `config/workspaces/*.toml`, optionally `artifacts/`, and `manifest.json`. A restore validates
//! the archive and the database, keeps the current database as `pre-restore-<stamp>.db`, replaces
//! the database file and migrates it forward to the head revision.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde_json::Value as JsonValue;
use wg_archivespec as spec;
use wg_config::paths::AppPaths;
use wg_store::{Location, Store, head_revision};
use wg_types::{Timestamp, WgError};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::scratch::Scratch;

/// What `create_backup` wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct BackupReport {
    pub path: PathBuf,
    pub bytes: u64,
    pub stamp: String,
    pub schema_revision: Option<String>,
}

/// What `restore_backup` did.
#[derive(Clone, Debug, PartialEq)]
pub struct RestoreReport {
    pub restored_from: PathBuf,
    pub manifest: JsonValue,
    pub safety_copy: Option<PathBuf>,
    pub schema_before: Option<String>,
    pub schema_after: Option<String>,
}

/// Writes a backup of the store and its configuration. `target` defaults to a file under
/// `<data>/backups/`. `at` stamps the file name and the manifest.
pub fn create_backup(
    paths: &AppPaths,
    store: &mut Store,
    target: Option<&Path>,
    include_artifacts: bool,
    at: Timestamp,
    version: &str,
) -> Result<BackupReport, WgError> {
    let stamp = spec::stamp(at);
    let target = match target {
        Some(path) => path.to_path_buf(),
        None => PathBuf::from(paths.backups_dir().as_str()).join(spec::backup_file_name(&stamp)),
    };
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|err| spec::io_failure("create", parent, &err))?;
    }
    let scratch = Scratch::new("wg-backup")?;
    let db_copy = scratch.path.join("workgraph.db");
    store.backup_to(&db_copy)?;
    let schema_revision = {
        let mut copy = Store::open(&Location::File(db_copy.clone()))?;
        copy.current_revision()?
    };
    let manifest = spec::manifest_text(
        &stamp,
        version,
        schema_revision.as_deref(),
        include_artifacts,
    )?;

    let partial = target.with_extension("partial");
    let file = std::fs::File::create(&partial)
        .map_err(|err| spec::io_failure("create", &partial, &err))?;
    let mut zip = ZipWriter::new(std::io::BufWriter::new(file));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    add_file(&mut zip, options, &db_copy, "workgraph.db", &target)?;
    let config_file = PathBuf::from(paths.config_file().as_str());
    if config_file.exists() {
        add_file(
            &mut zip,
            options,
            &config_file,
            "config/config.toml",
            &target,
        )?;
    }
    let workspace_dir = PathBuf::from(paths.workspace_config_dir().as_str());
    for file in toml_files(&workspace_dir)? {
        let name = format!("config/workspaces/{}", spec::file_name_of(&file));
        add_file(&mut zip, options, &file, &name, &target)?;
    }
    let artifacts = PathBuf::from(paths.artifacts_dir().as_str());
    if include_artifacts && artifacts.exists() {
        for file in files_under(&artifacts)? {
            let name = format!("artifacts/{}", spec::entry_name(&artifacts, &file));
            add_file(&mut zip, options, &file, &name, &target)?;
        }
    }
    zip.start_file("manifest.json", options)
        .map_err(|err| spec::zip_failure(&target, &err.to_string()))?;
    zip.write_all(manifest.as_bytes())
        .map_err(|err| spec::io_failure("write", &partial, &err))?;
    let mut writer = zip
        .finish()
        .map_err(|err| spec::zip_failure(&target, &err.to_string()))?;
    writer
        .flush()
        .map_err(|err| spec::io_failure("write", &partial, &err))?;
    drop(writer);
    std::fs::rename(&partial, &target).map_err(|err| spec::io_failure("move", &partial, &err))?;
    let bytes = std::fs::metadata(&target)
        .map_err(|err| spec::io_failure("stat", &target, &err))?
        .len();
    Ok(BackupReport {
        path: target,
        bytes,
        stamp,
        schema_revision,
    })
}

/// Restores a backup over the database at `paths`. `at` stamps the safety copy's name. Refuses
/// while a worker is heartbeating unless `force`. The configuration is restored only with
/// `with_config`; artifacts are restored whenever the archive holds them.
pub fn restore_backup(
    paths: &AppPaths,
    archive: &Path,
    with_config: bool,
    force: bool,
    at: Timestamp,
) -> Result<RestoreReport, WgError> {
    if !archive.exists() {
        return Err(WgError::validation(format!(
            "{} does not exist",
            archive.display()
        )));
    }
    let scratch = Scratch::new("wg-restore")?;
    let tmp = scratch.path.as_path();
    let not_a_backup =
        || WgError::validation(format!("{} is not a Workgraph backup", archive.display()));
    let file =
        std::fs::File::open(archive).map_err(|err| spec::io_failure("open", archive, &err))?;
    let mut zip = ZipArchive::new(file).map_err(|_| not_a_backup())?;
    let names: Vec<String> = zip.file_names().map(str::to_string).collect();
    if !names.iter().any(|n| n == "manifest.json") || !names.iter().any(|n| n == "workgraph.db") {
        return Err(not_a_backup());
    }
    for name in &names {
        if !spec::is_safe_entry(name) {
            return Err(WgError::validation(format!(
                "unsafe path in archive: {name}"
            )));
        }
    }
    let manifest = {
        let mut entry = zip.by_name("manifest.json").map_err(|_| not_a_backup())?;
        let mut text = String::new();
        entry
            .read_to_string(&mut text)
            .map_err(|err| spec::io_failure("read", archive, &err))?;
        spec::parse_manifest(&text)?
    };
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|_| not_a_backup())?;
        if entry.is_dir() {
            continue;
        }
        let target = tmp.join(entry.name());
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| spec::io_failure("create", parent, &err))?;
        }
        let mut out = std::fs::File::create(&target)
            .map_err(|err| spec::io_failure("create", &target, &err))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|err| spec::io_failure("extract", &target, &err))?;
    }

    let restored_db = tmp.join("workgraph.db");
    let integrity = {
        let mut copy = Store::open(&Location::File(restored_db.clone()))?;
        let rows = copy.read(|session| session.rows(&spec::integrity_check()))?;
        spec::first_text(&rows).unwrap_or_default()
    };
    if integrity != "ok" {
        return Err(WgError::validation(format!(
            "backup database failed integrity check: {integrity}"
        )));
    }

    let database = PathBuf::from(paths.database_file().as_str());
    if database.exists() && !force {
        let mut live = Store::open(&Location::File(database.clone()))?;
        // A probe that fails (no `workers` table yet) counts as no running worker.
        let running = live
            .read(|session| session.rows(&spec::live_workers()))
            .ok()
            .and_then(|rows| spec::first_int(&rows))
            .unwrap_or(0);
        if running > 0 {
            return Err(WgError::conflict(
                "a worker is running; stop Workgraph processes before restoring (or pass --force)",
            ));
        }
    }

    let mut safety = None;
    if database.exists() {
        let path = PathBuf::from(paths.backups_dir().as_str())
            .join(spec::pre_restore_file_name(&spec::stamp(at)));
        let mut live = Store::open(&Location::File(database.clone()))?;
        live.backup_to(&path)?;
        drop(live);
        safety = Some(path);
    }
    for sidecar in spec::sidecar_files(&database) {
        if sidecar.exists() {
            std::fs::remove_file(&sidecar)
                .map_err(|err| spec::io_failure("remove", &sidecar, &err))?;
        }
    }
    let staged = database.with_extension("restoring");
    std::fs::copy(&restored_db, &staged).map_err(|err| spec::io_failure("copy", &staged, &err))?;
    std::fs::rename(&staged, &database)
        .map_err(|err| spec::io_failure("replace", &database, &err))?;

    if with_config {
        let config_dir = tmp.join("config");
        let config = config_dir.join("config.toml");
        if config.exists() {
            let target = PathBuf::from(paths.config_file().as_str());
            std::fs::copy(&config, &target)
                .map_err(|err| spec::io_failure("copy", &target, &err))?;
        }
        let target_dir = PathBuf::from(paths.workspace_config_dir().as_str());
        for file in toml_files(&config_dir.join("workspaces"))? {
            let target = target_dir.join(spec::file_name_of(&file));
            std::fs::copy(&file, &target).map_err(|err| spec::io_failure("copy", &target, &err))?;
        }
    }
    let artifacts = tmp.join("artifacts");
    if artifacts.exists() {
        copy_tree(&artifacts, Path::new(paths.artifacts_dir().as_str()))?;
    }

    let mut store = Store::open(&Location::File(database))?;
    let schema_before = store.current_revision()?;
    let head =
        head_revision().ok_or_else(|| WgError::validation("no migration revisions".to_string()))?;
    store.upgrade(head)?;
    let schema_after = store.current_revision()?;
    Ok(RestoreReport {
        restored_from: archive.to_path_buf(),
        manifest,
        safety_copy: safety,
        schema_before,
        schema_after,
    })
}

fn add_file(
    zip: &mut ZipWriter<std::io::BufWriter<std::fs::File>>,
    options: SimpleFileOptions,
    source: &Path,
    name: &str,
    target: &Path,
) -> Result<(), WgError> {
    zip.start_file(name, options)
        .map_err(|err| spec::zip_failure(target, &err.to_string()))?;
    let mut file =
        std::fs::File::open(source).map_err(|err| spec::io_failure("open", source, &err))?;
    std::io::copy(&mut file, zip).map_err(|err| spec::io_failure("read", source, &err))?;
    Ok(())
}

/// The `*.toml` files of a directory, sorted by name; a missing directory has none.
fn toml_files(dir: &Path) -> Result<Vec<PathBuf>, WgError> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|err| spec::io_failure("read", dir, &err))? {
        let path = entry
            .map_err(|err| spec::io_failure("read", dir, &err))?
            .path();
        if spec::is_toml(&path) && path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Every file under `dir`, recursively, in sorted order.
fn files_under(dir: &Path) -> Result<Vec<PathBuf>, WgError> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in
            std::fs::read_dir(&current).map_err(|err| spec::io_failure("read", &current, &err))?
        {
            let path = entry
                .map_err(|err| spec::io_failure("read", &current, &err))?
                .path();
            if path.is_dir() {
                pending.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Copies a directory tree over an existing one (`shutil.copytree(dirs_exist_ok=True)`).
fn copy_tree(from: &Path, to: &Path) -> Result<(), WgError> {
    for file in files_under(from)? {
        let target = to.join(spec::entry_name(from, &file));
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| spec::io_failure("create", parent, &err))?;
        }
        std::fs::copy(&file, &target).map_err(|err| spec::io_failure("copy", &target, &err))?;
    }
    Ok(())
}
