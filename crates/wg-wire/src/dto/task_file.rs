//! `TaskFileOut`.

use wg_model::entity::task::TaskFile;

use crate::wire_dto;

wire_dto! {
    /// One file the task expects to change.
    pub struct TaskFileOut<'a>("TaskFileOut") value from_model(f: &'a TaskFile) {
        path: &'a str = &f.path,
        anchor: Option<&'a str> = f.anchor.as_deref(),
        note: &'a str = &f.note,
    }
}
