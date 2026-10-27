//! The lifecycle decisions of an experiment (`ExperimentService.create, freeze, close`): a draft
//! is created, frozen into a running experiment and closed. Each decision takes the rows the
//! shell loaded and returns the new row, the write and the audit event.

use serde_json::{Map, Value as Json, json};
use wg_model::entity::Experiment;
use wg_model::event::experiment::{ExperimentClosed, ExperimentCreated, ExperimentFrozen};
use wg_model::{Decision, EventDraft, EventPayload, Stamp, Write, py_repr_str};
use wg_types::{Id, WgError};

use super::protocol::{arms, validate_protocol};

/// `ExperimentStatus`.
pub const DRAFT: &str = "draft";
pub const RUNNING: &str = "running";
pub const CLOSED: &str = "closed";

/// `SLUG_RE` (`^[a-z0-9][a-z0-9-]{0,62}$`). Python's `$` also matches before one trailing
/// newline, so a slug ending in a newline passes.
fn slug_matches(slug: &str) -> bool {
    let body = slug.strip_suffix('\n').unwrap_or(slug);
    let mut chars = body.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    first_ok
        && body.chars().count() <= 63
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The checks `create` makes before it opens a transaction: the slug, the title and the
/// protocol. Returns the protocol as an object.
///
/// # Errors
///
/// Returns `validation_failed` for a bad slug, an empty title or an invalid protocol.
pub fn check_create(
    slug: &str,
    title: &str,
    protocol: &Json,
) -> Result<Map<String, Json>, WgError> {
    if !slug_matches(slug) {
        return Err(WgError::validation(format!(
            "experiment slug {} must be lowercase letters, digits and hyphens",
            py_repr_str(slug)
        )));
    }
    if title.trim().is_empty() {
        return Err(WgError::validation("experiment title must not be empty"));
    }
    validate_protocol(protocol)
}

/// `ExperimentService.create`: a draft experiment. `exists` says whether the workspace already
/// has an experiment with this slug.
///
/// # Errors
///
/// Returns `conflict` when the slug is taken.
pub fn create(
    stamp: &Stamp,
    workspace_id: Id,
    slug: &str,
    title: &str,
    question: Option<&str>,
    protocol: Map<String, Json>,
    exists: bool,
) -> Result<Decision<Experiment>, WgError> {
    if exists {
        return Err(WgError::conflict(format!(
            "experiment {} already exists in this workspace",
            py_repr_str(slug)
        ))
        .detail("slug", slug));
    }
    let ([experiment_id, event_id], _) = stamp.ids().take::<2>();
    let mut names: Vec<String> = arms(&protocol).into_iter().map(|(name, _)| name).collect();
    names.sort();
    let experiment = Experiment {
        id: experiment_id,
        workspace_id,
        slug: slug.to_owned(),
        title: title.to_owned(),
        question: question.map(str::to_owned),
        status: DRAFT.to_owned(),
        protocol,
        frozen_at: None,
        created_at: stamp.at,
        updated_at: stamp.at,
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(workspace_id),
        experiment_id,
        EventPayload::ExperimentCreated(ExperimentCreated {
            arms: json!(names),
            slug: slug.to_owned(),
        }),
    );
    Ok(Decision::new(experiment.clone())
        .write(Write::insert(experiment))
        .event(event))
}

/// The `conflict` error of a lifecycle step the experiment's status does not allow.
fn wrong_status(experiment: &Experiment, message: String) -> WgError {
    WgError::conflict(message)
        .detail("experiment", &experiment.slug)
        .detail("status", &experiment.status)
}

/// `ExperimentService.freeze`: validates the protocol, freezes it and starts the experiment.
///
/// # Errors
///
/// Returns `conflict` unless the experiment is a draft, and `validation_failed` when its stored
/// protocol is invalid.
pub fn freeze(stamp: &Stamp, found: &Experiment) -> Result<Decision<Experiment>, WgError> {
    if found.status != DRAFT {
        return Err(wrong_status(
            found,
            format!(
                "{} is already {}; only a draft can be frozen",
                found.slug, found.status
            ),
        ));
    }
    validate_protocol(&Json::Object(found.protocol.clone()))?;
    let ([event_id], _) = stamp.ids().take::<1>();
    let experiment = Experiment {
        status: RUNNING.to_owned(),
        frozen_at: Some(stamp.at),
        updated_at: stamp.at,
        ..found.clone()
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(experiment.workspace_id),
        experiment.id,
        EventPayload::ExperimentFrozen(ExperimentFrozen {
            protocol: experiment.protocol.clone(),
            slug: experiment.slug.clone(),
        }),
    );
    Ok(Decision::new(experiment.clone())
        .write(Write::update(experiment))
        .event(event))
}

/// `ExperimentService.close`: a running experiment is closed. `trials` is the number of trials
/// the experiment has.
///
/// # Errors
///
/// Returns `conflict` unless the experiment is running.
pub fn close(
    stamp: &Stamp,
    found: &Experiment,
    trials: i64,
) -> Result<Decision<Experiment>, WgError> {
    if found.status != RUNNING {
        return Err(wrong_status(
            found,
            format!(
                "{} is {}; only a running experiment can be closed",
                found.slug, found.status
            ),
        ));
    }
    let ([event_id], _) = stamp.ids().take::<1>();
    let experiment = Experiment {
        status: CLOSED.to_owned(),
        updated_at: stamp.at,
        ..found.clone()
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(experiment.workspace_id),
        experiment.id,
        EventPayload::ExperimentClosed(ExperimentClosed {
            slug: experiment.slug.clone(),
            trials: json!(trials),
        }),
    );
    Ok(Decision::new(experiment.clone())
        .write(Write::update(experiment))
        .event(event))
}
