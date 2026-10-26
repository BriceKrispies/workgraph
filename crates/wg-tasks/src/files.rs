//! The files a task expects to change (`task_files`): the list a write gives, the adds that
//! extend it, and the rows that store it. Mirrors `_file_dicts`, `_set_files` and the file
//! handling of `_update_in_session` in `core/services/tasks.py`.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use wg_model::TaskSlice;
use wg_model::entity::TaskFile;
use wg_types::Id;

/// One file of a task: the path, an optional anchor inside it, and a note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInput {
    pub path: String,
    pub anchor: Option<String>,
    pub note: String,
}

impl FileInput {
    /// The file as the event payload and the `files` change record show it.
    pub fn to_json(&self) -> Value {
        json!({ "path": self.path, "anchor": self.anchor, "note": self.note })
    }
}

/// The files of a task in position order.
pub fn current_files(tasks: &TaskSlice, task_id: Id) -> Vec<FileInput> {
    tasks
        .files_of(task_id)
        .map(|row| FileInput {
            path: row.path.clone(),
            anchor: row.anchor.clone(),
            note: row.note.clone(),
        })
        .collect()
}

/// Appends each of `adds` whose `(path, anchor)` is not yet in `files`. A repeated pair in
/// `adds` is appended once.
pub fn add_files(mut files: Vec<FileInput>, adds: &[FileInput]) -> Vec<FileInput> {
    let mut known: BTreeSet<(String, Option<String>)> = files
        .iter()
        .map(|file| (file.path.clone(), file.anchor.clone()))
        .collect();
    for item in adds {
        if known.insert((item.path.clone(), item.anchor.clone())) {
            files.push(item.clone());
        }
    }
    files
}

/// The `task_files` rows for `files`, positioned in list order.
pub fn file_rows(task_id: Id, files: &[FileInput]) -> Vec<TaskFile> {
    files
        .iter()
        .enumerate()
        .map(|(position, file)| TaskFile {
            task_id,
            position: i64::try_from(position).unwrap_or(i64::MAX),
            path: file.path.clone(),
            anchor: file.anchor.clone(),
            note: file.note.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, anchor: Option<&str>) -> FileInput {
        FileInput {
            path: path.to_owned(),
            anchor: anchor.map(str::to_owned),
            note: String::new(),
        }
    }

    #[test]
    fn adds_skip_known_pairs_and_repeats() {
        let files = vec![file("a.rs", None)];
        let adds = [
            file("a.rs", None),
            file("a.rs", Some("fn x")),
            file("a.rs", Some("fn x")),
        ];
        let merged = add_files(files, &adds);
        assert_eq!(merged, vec![file("a.rs", None), file("a.rs", Some("fn x"))]);
    }

    #[test]
    fn rows_are_positioned_in_list_order() {
        let task = Id::from_u128(7);
        let rows = file_rows(task, &[file("a", None), file("b", None)]);
        assert_eq!(
            rows.iter().map(|r| r.position).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert_eq!(rows[1].path, "b");
    }
}
