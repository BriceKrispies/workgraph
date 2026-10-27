//! `TaskLinkOut`.

use rpds::VectorSync;
use wg_model::entity::task::TaskLink;
use wg_types::Id;

use crate::wire_dto;

wire_dto! {
    /// A link from the task to something outside Workgraph.
    pub struct TaskLinkOut<'a>("TaskLinkOut") value from_model(l: &'a TaskLink) {
        id: Id = l.id,
        kind: &'a str = &l.kind,
        provider: &'a str = &l.provider,
        url: &'a str = &l.url,
        normalized_url: &'a str = &l.normalized_url,
        repo_full_name: &'a str = &l.repo_full_name,
        number: Option<i64> = l.number,
        repository_id: Option<Id> = l.repository_id,
        sources: &'a VectorSync<String> = &l.sources,
        title: &'a str = &l.title,
    }
}
