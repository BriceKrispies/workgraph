//! Workspace export and import (F14): `persistence/backup.py`'s `export_workspace` and
//! `import_workspace`. The file is a header line and one record per row, each on its own line,
//! in foreign-key order.

use std::path::{Path, PathBuf};

use wg_archivespec as spec;
use wg_store::{Session, Store};
use wg_types::{WgError, parse_id};

/// The rows per table, in the order the tables were written or first read.
pub type TableCounts = Vec<(String, usize)>;

/// What `export_workspace` wrote: the file and the rows per table (tables with rows only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportReport {
    pub path: PathBuf,
    pub tables: Vec<(String, usize)>,
}

/// What `import_workspace` inserted: the workspace id and the rows per table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImportReport {
    pub workspace_id: String,
    pub tables: Vec<(String, usize)>,
}

/// Writes every record of one workspace to `target` as JSON Lines.
pub fn export_workspace(
    store: &mut Store,
    workspace_id: &str,
    target: &Path,
    version: &str,
) -> Result<ExportReport, WgError> {
    let windows = cfg!(windows);
    let (lines, tables) = store.read(|session| collect_records(session, workspace_id))?;
    let eol = spec::line_end(windows);
    let mut text = spec::export_header(workspace_id, version, windows)?;
    for line in lines {
        text.push_str(&line);
        text.push_str(eol);
    }
    if let Some(parent) = target.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|err| spec::io_failure("create", parent, &err))?;
    }
    std::fs::write(target, text).map_err(|err| spec::io_failure("write", target, &err))?;
    Ok(ExportReport {
        path: target.to_path_buf(),
        tables,
    })
}

/// The export lines of a workspace and the nonzero row counts per table.
fn collect_records(
    session: &mut Session<'_>,
    workspace_id: &str,
) -> Result<(Vec<String>, TableCounts), WgError> {
    let mut exported = spec::ExportedIds::default();
    let mut lines = Vec::new();
    let mut tables = Vec::new();
    for table in spec::EXPORT_TABLES {
        let Some(statement) = spec::export_select(table, workspace_id, &exported) else {
            continue;
        };
        let rows = session.rows(&statement)?;
        exported = exported.record(table.name, spec::ids_of(table, &rows));
        for row in &rows {
            lines.push(spec::export_line(table, row)?);
        }
        if !rows.is_empty() {
            tables.push((table.name.to_string(), rows.len()));
        }
    }
    Ok((lines, tables))
}

/// Inserts the records of an export into `store`, in one transaction. A workspace already in the
/// database is refused and nothing is written. Tasks are re-indexed for search afterwards.
pub fn import_workspace(store: &mut Store, source: &Path) -> Result<ImportReport, WgError> {
    let text =
        std::fs::read_to_string(source).map_err(|err| spec::io_failure("read", source, &err))?;
    let mut lines = text.lines();
    let header = lines
        .next()
        .ok_or_else(|| WgError::validation("not a Workgraph workspace export"))?;
    let workspace_id = spec::parse_header(header)?;
    store.write(move |session| {
        session.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        if !session
            .rows(&spec::workspace_exists(&workspace_id))?
            .is_empty()
        {
            return Err(WgError::conflict(
                "this workspace already exists in the target database; nothing was imported",
            ));
        }
        let mut tables: Vec<(String, usize)> = Vec::new();
        for line in lines {
            let row = spec::parse_row(line)?;
            session.run(&row.statement())?;
            let name = row.table.name;
            match tables.iter_mut().find(|(seen, _)| seen.as_str() == name) {
                Some((_, count)) => *count += 1,
                None => tables.push((name.to_string(), 1)),
            }
        }
        for id in session.rows(&spec::task_ids_in(&workspace_id))? {
            if let Some(wg_rows::Raw::Text(task)) = id.first() {
                session.index_task(parse_id(task)?)?;
            }
        }
        Ok(ImportReport {
            workspace_id,
            tables,
        })
    })
}
