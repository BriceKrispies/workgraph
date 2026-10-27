//! The pure half of the structure analysis (`git analyze`) and of the change surface (`git
//! surface`): the rows a commit's analysis writes and the records a surface computes, from the
//! per-file results the shell gathered. Ported from `analyze_commit` and `compute_change_surface`
//! in `integrations/git/service.py`.

use serde_json::{Map, Value as Json};
use std::collections::BTreeMap;
use wg_codeanalysis::{ANALYZER_VERSION, FileStructure, Mode};
use wg_gitcore::{Provenance, SurfaceRecord};
use wg_json::{Value, dict};
use wg_model::entity::JsonObject;
use wg_model::entity::git::{Commit, CommitAnalysis, CommitSymbol, TaskChangeSurface};
use wg_model::entity::run::RevisionManifest;
use wg_model::{IdSeq, Row, Write};
use wg_types::pyval::prefix;
use wg_types::{Id, Timestamp, parse_id};
use wg_wire::convention::{id, int, json_value, text};

/// Python's `name[:500]` cut of a symbol name.
const SYMBOL_NAME_LIMIT: usize = 500;

/// What the analysis found in one file of a commit.
#[derive(Debug, Clone, PartialEq)]
pub struct FileResult {
    pub file_change_id: Id,
    /// The structure of a text file; `None` for a binary file, which is not analysed.
    pub structure: Option<FileStructure>,
}

/// The analysis rows of one commit: the `CommitAnalysis` row, its symbols, and the commit's
/// `structure_analyzed` flag set. Files that fell back to line counts record their reason in
/// `detail.fallback_reasons`, keyed by the file-change id.
pub fn analysis_writes(
    commit: &Commit,
    results: &[FileResult],
    ids: IdSeq,
    now: Timestamp,
) -> (IdSeq, Vec<Write>) {
    let mut parsed: i64 = 0;
    let mut fallback: i64 = 0;
    let mut functions: i64 = 0;
    let mut types: i64 = 0;
    let mut reasons = Map::new();
    let mut symbols: Vec<CommitSymbol> = Vec::new();
    let mut seq = ids;
    for result in results {
        match &result.structure {
            Some(structure) if structure.mode == Mode::Structural => {
                parsed += 1;
                functions += to_i64(structure.functions());
                types += to_i64(structure.types());
                for change in &structure.changes {
                    let (symbol_id, next) = seq.next();
                    seq = next;
                    symbols.push(CommitSymbol {
                        id: symbol_id,
                        file_change_id: result.file_change_id,
                        symbol_kind: change.kind.as_str().to_owned(),
                        name: prefix(&change.name, SYMBOL_NAME_LIMIT),
                        change_kind: change.change.as_str().to_owned(),
                        start_line: Some(to_i64(change.start_line)),
                        end_line: Some(to_i64(change.end_line)),
                        lines_changed: to_i64(change.lines_changed),
                    });
                }
            }
            Some(structure) => {
                fallback += 1;
                reasons.insert(
                    result.file_change_id.to_string(),
                    structure.reason.clone().map_or(Json::Null, Json::String),
                );
            }
            None => {
                fallback += 1;
                reasons.insert(
                    result.file_change_id.to_string(),
                    Json::String("binary".to_owned()),
                );
            }
        }
    }
    let mut detail = JsonObject::new();
    detail.insert("fallback_reasons".to_owned(), Json::Object(reasons));
    let analysis = CommitAnalysis {
        commit_id: commit.id,
        analyzer_version: ANALYZER_VERSION.to_owned(),
        files_parsed: parsed,
        files_fallback: fallback,
        functions_affected: functions,
        types_affected: types,
        detail,
        analyzed_at: now,
    };
    let writes = std::iter::once(Write::Insert(Row::CommitAnalysis(analysis)))
        .chain(
            symbols
                .into_iter()
                .map(|symbol| Write::Insert(Row::CommitSymbol(symbol))),
        )
        .chain(std::iter::once(Write::Update {
            row: Row::Commit(Commit {
                structure_analyzed: true,
                ..commit.clone()
            }),
            expected_version: None,
        }))
        .collect();
    (seq, writes)
}

fn to_i64<T: TryInto<i64>>(value: T) -> i64 {
    value.try_into().unwrap_or(i64::MAX)
}

/// The `task_change_surfaces` row of a computed record. The row is updated in place when the task
/// already has one for the repository, so `id` is the existing id or a fresh one.
pub fn surface_row(
    id: Id,
    task_id: Id,
    repository_id: Id,
    attempt_id: Option<Id>,
    record: &SurfaceRecord,
    detail: JsonObject,
    now: Timestamp,
) -> TaskChangeSurface {
    TaskChangeSurface {
        id,
        task_id,
        repository_id,
        attempt_id,
        base_sha: record.base_sha.clone(),
        final_sha: record.final_sha.clone(),
        files_changed: to_i64(record.files_changed),
        insertions: to_i64(record.insertions),
        deletions: to_i64(record.deletions),
        renames: to_i64(record.renames),
        binary_files: to_i64(record.binary_files),
        functions_affected: to_i64(record.functions_affected),
        types_affected: to_i64(record.types_affected),
        detail,
        computed_at: now,
        updated_at: now,
    }
}

/// The `detail` object of a surface record: provenance, the structural fallback count and the
/// first files with their classes.
pub fn surface_detail(record: &SurfaceRecord) -> JsonObject {
    let mut detail = JsonObject::new();
    detail.insert("provenance".to_owned(), provenance_json(&record.provenance));
    detail.insert(
        "structural_fallback_files".to_owned(),
        Json::from(record.structural_fallback_files),
    );
    detail.insert(
        "files".to_owned(),
        Json::Array(
            record
                .files
                .iter()
                .map(|file| {
                    let mut object = Map::new();
                    object.insert("path".to_owned(), Json::String(file.path.clone()));
                    object.insert(
                        "old_path".to_owned(),
                        file.old_path.clone().map_or(Json::Null, Json::String),
                    );
                    object.insert("insertions".to_owned(), Json::from(file.insertions));
                    object.insert("deletions".to_owned(), Json::from(file.deletions));
                    object.insert("binary".to_owned(), Json::Bool(file.binary));
                    object.insert("class".to_owned(), Json::String(file.class.to_owned()));
                    Json::Object(object)
                })
                .collect(),
        ),
    );
    detail
}

fn provenance_json(provenance: &Provenance) -> Json {
    let mut object = Map::new();
    object.insert(
        "base".to_owned(),
        provenance
            .base
            .map_or(Json::Null, |p| Json::String(p.to_owned())),
    );
    object.insert(
        "final".to_owned(),
        provenance
            .final_rev
            .map_or(Json::Null, |p| Json::String(p.to_owned())),
    );
    Json::Object(object)
}

/// One surface record as `git surface` prints it (`compute_change_surface`'s dict).
pub fn surface_value(repository_id: Id, record: &SurfaceRecord) -> Value {
    dict!({
        "repository_id": id(repository_id),
        "base_sha": text(&record.base_sha),
        "final_sha": text(&record.final_sha),
        "files_changed": int(to_i64(record.files_changed)),
        "insertions": int(to_i64(record.insertions)),
        "deletions": int(to_i64(record.deletions)),
        "renames": int(to_i64(record.renames)),
        "binary_files": int(to_i64(record.binary_files)),
        "functions_affected": int(to_i64(record.functions_affected)),
        "types_affected": int(to_i64(record.types_affected)),
        "detail": json_value(&Json::Object(surface_detail(record))),
    })
}

/// The `repository_id -> sha` map of a manifest's entries that carry a SHA.
pub fn manifest_shas(manifest: Option<RevisionManifest>) -> BTreeMap<Id, String> {
    let Some(manifest) = manifest else {
        return BTreeMap::new();
    };
    manifest
        .entries
        .iter()
        .filter_map(|entry| {
            let repository = entry.get("repository_id").and_then(Json::as_str)?;
            let sha = entry
                .get("sha")
                .and_then(Json::as_str)
                .filter(|sha| !sha.is_empty())?;
            Some((parse_id(repository).ok()?, sha.to_owned()))
        })
        .collect()
}
