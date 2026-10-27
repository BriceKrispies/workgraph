//! `RepositoryBrief`.

use wg_model::entity::repository::Repository;
use wg_types::Id;

use crate::wire_dto;

wire_dto! {
    /// The short form of a repository, as project summaries list it.
    pub struct RepositoryBrief<'a>("RepositoryBrief") value from_model(r: &'a Repository) {
        id: Id = r.id,
        name: &'a str = &r.name,
        access_mode: &'a str = r.access_mode.as_str(),
        health_status: &'a str = &r.health_status,
    }
}
