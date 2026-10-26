//! The pure half of commit attribution and explicit links (`git link`, `git unlink` and the
//! attribution step of `git sync`). Ported from `GitService._attribute`, `_add_link`,
//! `link_commit` and `unlink_commit` in `integrations/git/service.py`.

use serde_json::Value as Json;
use wg_gitcore::{
    Candidate, CandidateSource, CarriedLink, Evidence, Trailer, message_candidates,
    trailer_candidates,
};
use wg_json::Value;
use wg_model::entity::JsonObject;
use wg_model::entity::git::{Commit, CommitTaskLink};
use wg_model::entity::task::TaskRepository;
use wg_model::event::{EventPayload, TaskCommitLinked, TaskCommitUnlinked};
use wg_model::{Row, RowKey, Write};
use wg_types::{Id, Timestamp, parse_id};
use wg_wire::dto::commit_link;

use super::ingest::offset_time;

/// The source name of an explicit link (`AttributionSource.EXPLICIT`).
pub const EXPLICIT: &str = "explicit";
/// The source name of a worktree link (`AttributionSource.WORKTREE`).
pub const WORKTREE: &str = "worktree";
/// The confidence of a worktree link (`ATTRIBUTION_CONFIDENCE[WORKTREE]`).
pub const WORKTREE_CONFIDENCE: f64 = 0.9;
/// The actor of attribution writes (`system:git-attribution`).
pub const ATTRIBUTION_ACTOR: &str = "system:git-attribution";
/// The confidence of an explicit link (`ATTRIBUTION_CONFIDENCE[EXPLICIT]`).
pub const EXPLICIT_CONFIDENCE: f64 = 1.0;

/// The trailers of a stored commit, as the attribution reads them (`{key, value}` objects).
pub fn trailers_of(commit: &Commit) -> Vec<Trailer> {
    commit
        .trailers
        .iter()
        .map(|object| Trailer {
            key: string_field(object, "key"),
            value: string_field(object, "value"),
        })
        .collect()
}

fn string_field(object: &JsonObject, key: &str) -> String {
    object
        .get(key)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The task references a stored commit names: its task-naming trailers, then the keys its prose
/// mentions for any of the workspace's key prefixes.
pub fn candidates_of(commit: &Commit, key_prefixes: &[String]) -> Vec<Candidate> {
    let trailers = trailers_of(commit);
    let prefixes: Vec<&str> = key_prefixes.iter().map(String::as_str).collect();
    let mut found = trailer_candidates(&trailers);
    found.extend(message_candidates(&commit.message, &prefixes, &trailers));
    found
}

/// A candidate's evidence as the JSON object the link stores.
pub fn evidence_object(evidence: &Evidence) -> JsonObject {
    let mut object = JsonObject::new();
    match evidence {
        Evidence::Trailer { trailer } => {
            object.insert("trailer".to_owned(), Json::String(trailer.clone()));
        }
        Evidence::Message {
            label,
            matched,
            ambiguous,
            all_matches,
        } => {
            object.insert("label".to_owned(), Json::String((*label).to_owned()));
            object.insert("matched".to_owned(), Json::String(matched.clone()));
            object.insert("ambiguous".to_owned(), Json::Bool(*ambiguous));
            object.insert(
                "all_matches".to_owned(),
                Json::Array(all_matches.iter().cloned().map(Json::String).collect()),
            );
        }
    }
    object
}

/// The source name a candidate's link records.
pub fn source_name(source: CandidateSource) -> &'static str {
    source.as_str()
}

/// A new `commit_task_links` row, not yet written.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkDraft {
    pub id: Id,
    pub commit_id: Id,
    pub task_id: Id,
    pub source: String,
    pub confidence: f64,
    pub evidence: JsonObject,
    pub attempt_id: Option<Id>,
    pub created_by: String,
    pub created_at: Timestamp,
}

impl LinkDraft {
    /// The row to insert.
    pub fn row(&self) -> CommitTaskLink {
        CommitTaskLink {
            id: self.id,
            commit_id: self.commit_id,
            task_id: self.task_id,
            source: self.source.clone(),
            confidence: wg_model::entity::Real(self.confidence),
            evidence: self.evidence.clone(),
            attempt_id: self.attempt_id,
            created_by: self.created_by.clone(),
            created_at: self.created_at,
        }
    }
}

/// Whether a link of `source` from `commit_id` to `task_id` already exists (`_add_link`'s check).
pub fn link_exists(existing: &[CommitTaskLink], commit_id: Id, task_id: Id, source: &str) -> bool {
    existing
        .iter()
        .any(|link| link.commit_id == commit_id && link.task_id == task_id && link.source == source)
}

/// The links attribution adds to one commit: candidates whose task resolved (`resolved` maps a
/// reference to a task id) and whose link does not exist yet. A link already added for the same
/// task and source earlier in the list is skipped too, as the Python insert-then-check does.
pub fn attribution_drafts(
    commit: &Commit,
    candidates: &[(Candidate, Option<Id>)],
    existing: &[CommitTaskLink],
    worktree: Option<(Option<Id>, Id, &str)>,
    ids: wg_model::IdSeq,
    now: Timestamp,
) -> (wg_model::IdSeq, Vec<LinkDraft>) {
    let (seq, mut drafts, seen) = candidates.iter().fold(
        (ids, Vec::new(), existing.to_vec()),
        |(seq, mut drafts, mut seen), (candidate, task)| match task {
            None => (seq, drafts, seen),
            Some(task_id) => {
                let source = source_name(candidate.source);
                if link_exists(&seen, commit.id, *task_id, source) {
                    return (seq, drafts, seen);
                }
                let (id, next) = seq.next();
                let draft = LinkDraft {
                    id,
                    commit_id: commit.id,
                    task_id: *task_id,
                    source: source.to_owned(),
                    confidence: candidate.confidence,
                    evidence: evidence_object(&candidate.evidence),
                    attempt_id: None,
                    created_by: ATTRIBUTION_ACTOR.to_owned(),
                    created_at: offset_time(now, drafts.len()),
                };
                seen.push(draft.row());
                drafts.push(draft);
                (next, drafts, seen)
            }
        },
    );
    let (seq, drafts_after_worktree) = match worktree {
        None => (seq, Vec::new()),
        Some((attempt_id, task_id, branch)) => {
            if link_exists(&seen, commit.id, task_id, WORKTREE) {
                (seq, Vec::new())
            } else {
                let (id, next) = seq.next();
                let mut evidence = JsonObject::new();
                evidence.insert(
                    "worktree_branch".to_owned(),
                    Json::String(branch.to_owned()),
                );
                (
                    next,
                    vec![LinkDraft {
                        id,
                        commit_id: commit.id,
                        task_id,
                        source: WORKTREE.to_owned(),
                        confidence: WORKTREE_CONFIDENCE,
                        evidence,
                        attempt_id,
                        created_by: ATTRIBUTION_ACTOR.to_owned(),
                        created_at: offset_time(now, drafts.len()),
                    }],
                )
            }
        }
    };
    drafts.extend(drafts_after_worktree);
    (seq, drafts)
}

/// The writes of a set of drafts: one insert per link.
pub fn link_writes(drafts: &[LinkDraft]) -> Vec<Write> {
    drafts
        .iter()
        .map(|draft| Write::Insert(Row::CommitTaskLink(draft.row())))
        .collect()
}

/// The writes of an explicit link (`git link`): the link row with confidence 1.0 and evidence
/// `note` and `linked_by` (only when the commit has no explicit link to the task yet), and the
/// `TaskRepository` row when the task is not yet tied to the repository.
pub fn explicit_link_writes(
    commit: &Commit,
    task_id: Id,
    repository_id: Id,
    repository_access: &str,
    existing: &[CommitTaskLink],
    link: ExplicitLinkIds<'_>,
) -> Vec<Write> {
    let link_write = (!link_exists(existing, commit.id, task_id, EXPLICIT)).then(|| {
        let mut evidence = JsonObject::new();
        evidence.insert("note".to_owned(), Json::String(link.note.to_owned()));
        evidence.insert("linked_by".to_owned(), Json::String(link.actor.to_owned()));
        Write::Insert(Row::CommitTaskLink(
            LinkDraft {
                id: link.id,
                commit_id: commit.id,
                task_id,
                source: EXPLICIT.to_owned(),
                confidence: EXPLICIT_CONFIDENCE,
                evidence,
                attempt_id: link.attempt_id,
                created_by: link.actor.to_owned(),
                created_at: link.now,
            }
            .row(),
        ))
    });
    let task_repository_write = (!link.task_repository_exists).then(|| {
        Write::Insert(Row::TaskRepository(TaskRepository {
            task_id,
            repository_id,
            access: repository_access.to_owned(),
            created_at: link.now,
        }))
    });
    link_write
        .into_iter()
        .chain(task_repository_write)
        .collect()
}

/// The identifiers, clock and facts an explicit link needs beyond the commit and the task.
#[derive(Debug, Clone, Copy)]
pub struct ExplicitLinkIds<'a> {
    pub id: Id,
    pub note: &'a str,
    pub actor: &'a str,
    pub attempt_id: Option<Id>,
    pub task_repository_exists: bool,
    pub now: Timestamp,
}

/// The `task.commit_linked` event of an explicit link.
pub fn commit_linked_payload(commit: &Commit, repository_id: Id) -> EventPayload {
    EventPayload::TaskCommitLinked(TaskCommitLinked {
        commit_id: commit.id,
        sha: commit.sha.clone(),
        repository_id,
        source: EXPLICIT.to_owned(),
    })
}

/// The `task.commit_unlinked` event of an unlink that removed rows.
pub fn commit_unlinked_payload(sha: &str, removed: usize) -> EventPayload {
    EventPayload::TaskCommitUnlinked(TaskCommitUnlinked {
        sha: sha.to_owned(),
        removed: i64::try_from(removed).unwrap_or(i64::MAX),
    })
}

/// The explicit links of `task_id` whose commit SHA starts with `sha` (`unlink_commit`'s
/// selection). Returns the ids to delete, with their deletion writes.
pub fn unlink_selection(links: &[(CommitTaskLink, String)], task_id: Id, sha: &str) -> Vec<Write> {
    links
        .iter()
        .filter(|(link, commit_sha)| {
            link.task_id == task_id && commit_sha.starts_with(sha) && link.source == EXPLICIT
        })
        .map(|(link, _)| Write::Delete(RowKey::CommitTaskLink(link.id)))
        .collect()
}

/// The `CommitLinkOut` list of `git link`: the links from `commit` to `task_id`, in store order.
pub fn link_outputs(links: &[CommitTaskLink], commit: &Commit, task_id: Id) -> Value {
    Value::List(
        links
            .iter()
            .filter(|link| link.commit_id == commit.id && link.task_id == task_id)
            .map(|link| commit_link::from_model(link, &commit.sha, commit.repository_id))
            .collect(),
    )
}

/// The `commit_task_links` row of a carried link on the rewritten commit.
pub fn carried_link_row(
    id: Id,
    commit_id: Id,
    link: &CarriedLink,
    now: wg_types::Timestamp,
) -> CommitTaskLink {
    let mut evidence = serde_json::Map::new();
    evidence.insert("from_commit".to_owned(), link.from_commit.clone().into());
    evidence.insert(
        "original_source".to_owned(),
        link.original_source.clone().into(),
    );
    LinkDraft {
        id,
        commit_id,
        task_id: parse_id(&link.task_id).unwrap_or(commit_id),
        source: "rewrite_reconciliation".to_owned(),
        confidence: link.confidence,
        evidence,
        attempt_id: link
            .attempt_id
            .as_deref()
            .and_then(|text| parse_id(text).ok()),
        created_by: "system:git-reconciliation".to_owned(),
        created_at: now,
    }
    .row()
}
