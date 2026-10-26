//! Check selection: which enabled configurations apply to a task (by scope), which checks they
//! carry, and which of those a tier keeps. Port of `applicable_configs`, `applicable_checks`,
//! `checks_for_tier` and `parse_tier` in `verification/configs.py`.

use std::collections::BTreeMap;

use wg_model::entity::VerificationConfig;
use wg_types::{ErrorCode, Id, WgError};

use crate::config::{CheckSpec, CheckTier};

/// The facts about a task that decide which scopes apply to it. The shell reads them from the
/// slice; the core does no lookups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskScope {
    pub workspace_id: Id,
    pub task_id: Id,
    pub project_ids: Vec<Id>,
    pub repository_ids: Vec<Id>,
}

/// One check together with the configuration it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct ApplicableCheck {
    pub config_id: Id,
    pub config_name: String,
    pub scope_type: String,
    pub scope_id: Id,
    pub spec: CheckSpec,
}

/// Whether a configuration's scope covers the task: the workspace, the task itself, one of its
/// projects, or one of its repositories.
fn in_scope(config: &VerificationConfig, scope: &TaskScope) -> bool {
    match config.scope_type.as_str() {
        "workspace" => config.scope_id == scope.workspace_id,
        "task" => config.scope_id == scope.task_id,
        "project" => scope.project_ids.contains(&config.scope_id),
        "repository" => scope.repository_ids.contains(&config.scope_id),
        _ => false,
    }
}

/// The enabled configurations of the task's workspace whose scope covers the task, ordered by
/// scope type and then name (the order the Python query returns).
pub fn applicable_configs<'a>(
    configs: &'a [VerificationConfig],
    scope: &TaskScope,
) -> Vec<&'a VerificationConfig> {
    configs
        .iter()
        .filter(|config| {
            config.workspace_id == scope.workspace_id && config.enabled && in_scope(config, scope)
        })
        .map(|config| {
            (
                (config.scope_type.clone(), config.name.clone(), config.id),
                config,
            )
        })
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}

/// Every check of every applicable configuration, in configuration order then check order. A
/// repository-scoped check without its own `repository` runs in that repository.
pub fn applicable_checks(
    configs: &[VerificationConfig],
    scope: &TaskScope,
) -> Result<Vec<ApplicableCheck>, WgError> {
    applicable_configs(configs, scope)
        .into_iter()
        .flat_map(|config| {
            config
                .checks
                .iter()
                .map(move |raw| applicable_check(config, raw))
        })
        .collect()
}

/// One stored check of a configuration, validated and placed in its scope.
fn applicable_check(
    config: &VerificationConfig,
    raw: &wg_model::entity::JsonObject,
) -> Result<ApplicableCheck, WgError> {
    let spec = CheckSpec::from_json(raw)?;
    let repository = match spec.repository {
        None if config.scope_type == "repository" => Some(config.scope_id.to_string()),
        other => other,
    };
    Ok(ApplicableCheck {
        config_id: config.id,
        config_name: config.name.clone(),
        scope_type: config.scope_type.clone(),
        scope_id: config.scope_id,
        spec: CheckSpec { repository, ..spec },
    })
}

/// The checks of one tier, in order; `None` keeps every check.
pub fn checks_for_tier(
    checks: &[ApplicableCheck],
    tier: Option<CheckTier>,
) -> Vec<&ApplicableCheck> {
    checks
        .iter()
        .filter(|check| tier.is_none_or(|wanted| check.spec.tier == wanted))
        .collect()
}

/// Validates a tier from a CLI, API or job boundary; `None` selects every tier.
pub fn parse_tier(value: Option<&str>) -> Result<Option<CheckTier>, WgError> {
    match value {
        None => Ok(None),
        Some(text) => CheckTier::parse(text).map(Some).ok_or_else(|| {
            WgError::new(
                ErrorCode::ValidationFailed,
                format!("unknown check tier '{text}'; expected one of: fast, full"),
            )
        }),
    }
}
