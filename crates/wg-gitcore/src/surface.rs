//! The final change surface of a task in one repository: the diff between the attempt baseline
//! and the accepted revision, or between the parent of the earliest linked commit and the latest
//! linked commit where a manifest is missing.
//!
//! Ported from `compute_change_surface` in `integrations/git/service.py`. The git questions
//! (object existence, ancestry, parents) come in as closures, so the decision and the record are
//! pure; the shell runs `git` for the numstat and the structural analysis of each file.

use crate::{FileChange, file_class};

/// Git's well-known empty tree, the base of a repository's first commit.
pub const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
/// Detail files kept per record.
pub const DETAIL_FILE_LIMIT: usize = 500;

/// What the task knows about one repository.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceRepo<'a> {
    /// The repository's entry in the attempt's baseline manifest, when it has one.
    pub manifest_base: Option<&'a str>,
    /// The repository's entry in the accepted revision manifest, when it has one.
    pub manifest_final: Option<&'a str>,
    /// The non-merge commits linked to the task in this repository, in the order the shell
    /// loaded them.
    pub linked_shas: &'a [String],
}

/// Where a base or final revision came from, as the record's provenance reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provenance {
    pub base: Option<&'static str>,
    pub final_rev: Option<&'static str>,
}

/// The decision for one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfacePlan {
    /// Nothing to measure (no revisions, equal revisions, or no linked commit present).
    Skip,
    /// The range cannot be measured because a revision object is missing.
    Gap { base: String, final_rev: String },
    /// Measure `base..final_rev`.
    Measure {
        base: String,
        final_rev: String,
        provenance: Provenance,
    },
}

/// Chooses the revisions to measure.
///
/// `exists` tells whether a git object is present. `is_ancestor(a, b)` is true when `a` is an
/// ancestor of `b` or the same commit. `parent_of(sha)` is the first parent, if any.
pub fn plan_surface(
    repo: &SurfaceRepo<'_>,
    exists: impl Fn(&str) -> bool,
    is_ancestor: impl Fn(&str, &str) -> bool,
    parent_of: impl Fn(&str) -> Option<String>,
) -> SurfacePlan {
    let mut base = non_empty(repo.manifest_base).map(str::to_owned);
    let mut final_rev = non_empty(repo.manifest_final).map(str::to_owned);
    let mut provenance = Provenance {
        base: base.as_ref().map(|_| "attempt baseline manifest"),
        final_rev: final_rev.as_ref().map(|_| "accepted revision manifest"),
    };

    if (base.is_none() || final_rev.is_none()) && !repo.linked_shas.is_empty() {
        let shas: Vec<&str> = repo
            .linked_shas
            .iter()
            .map(String::as_str)
            .filter(|sha| exists(sha))
            .collect();
        let (Some(first), Some(last)) = (shas.first().copied(), shas.last().copied()) else {
            return SurfacePlan::Skip;
        };
        if final_rev.is_none() {
            let chosen = shas
                .iter()
                .copied()
                .find(|s| shas.iter().all(|o| is_ancestor(o, s)))
                .unwrap_or(last);
            final_rev = Some(chosen.to_owned());
            provenance.final_rev = Some("latest linked commit");
        }
        if base.is_none() {
            let earliest = shas
                .iter()
                .copied()
                .find(|s| shas.iter().all(|o| is_ancestor(s, o)))
                .unwrap_or(first);
            let parent = parent_of(earliest)
                .filter(|p| !p.is_empty())
                .unwrap_or_else(|| EMPTY_TREE.to_owned());
            base = Some(parent);
            provenance.base = Some("parent of earliest linked commit");
        }
    }

    let (Some(base), Some(final_rev)) = (base, final_rev) else {
        return SurfacePlan::Skip;
    };
    if base.is_empty() || final_rev.is_empty() || base == final_rev {
        return SurfacePlan::Skip;
    }
    if !(exists(&final_rev) && (exists(&base) || base == EMPTY_TREE)) {
        return SurfacePlan::Gap { base, final_rev };
    }
    SurfacePlan::Measure {
        base,
        final_rev,
        provenance,
    }
}

/// One file of the detail list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailFile {
    pub path: String,
    pub old_path: Option<String>,
    pub insertions: u64,
    pub deletions: u64,
    pub binary: bool,
    pub class: &'static str,
}

/// The measured surface of one repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SurfaceRecord {
    pub base_sha: String,
    pub final_sha: String,
    pub files_changed: u64,
    pub insertions: u64,
    pub deletions: u64,
    pub renames: u64,
    pub binary_files: u64,
    pub functions_affected: u64,
    pub types_affected: u64,
    pub structural_fallback_files: u64,
    pub provenance: Provenance,
    /// The first [`DETAIL_FILE_LIMIT`] files in git's order.
    pub files: Vec<DetailFile>,
}

/// Totals for a measured range. `structures[i]` is the structural analysis of `files[i]`:
/// `Some((functions, types))` when analysed structurally, `None` when it fell back to line
/// counts. Binary files are not analysed and are ignored there.
pub fn surface_record(
    base: &str,
    final_rev: &str,
    provenance: Provenance,
    files: &[FileChange],
    structures: &[Option<(u64, u64)>],
) -> SurfaceRecord {
    let mut functions = 0;
    let mut types = 0;
    let mut fallback = 0;
    for (index, file) in files.iter().enumerate() {
        if file.binary {
            continue;
        }
        match structures.get(index).copied().flatten() {
            Some((f, t)) => {
                functions += f;
                types += t;
            }
            None => fallback += 1,
        }
    }
    SurfaceRecord {
        base_sha: base.to_owned(),
        final_sha: final_rev.to_owned(),
        files_changed: files.len() as u64,
        insertions: files.iter().map(|f| f.insertions).sum(),
        deletions: files.iter().map(|f| f.deletions).sum(),
        renames: files.iter().filter(|f| f.old_path.is_some()).count() as u64,
        binary_files: files.iter().filter(|f| f.binary).count() as u64,
        functions_affected: functions,
        types_affected: types,
        structural_fallback_files: fallback,
        provenance,
        files: files
            .iter()
            .take(DETAIL_FILE_LIMIT)
            .map(|f| DetailFile {
                path: f.path.clone(),
                old_path: f.old_path.clone(),
                insertions: f.insertions,
                deletions: f.deletions,
                binary: f.binary,
                class: file_class(&f.path),
            })
            .collect(),
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.is_empty())
}
