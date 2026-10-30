//! Shell: `AnalyticsService.estimate` (`analytics/estimation.py`). The command resolves the target
//! task and the repositories, takes the text scores from the similar-task search of the intel
//! feature (or its lexical search of the title, when the search finds nothing), and hands them to
//! the pure estimate of `wg-analytics`. The decision itself is in
//! [`wg_analytics::core::estimate`].

use wg_analytics::build;
use wg_analytics::core::estimate::{Request, estimate as estimate_core};
use wg_intel_feature::service::{embed, search};
use wg_json::Value;
use wg_registry::resolve;
use wg_runtime::{App, RequestContext};
use wg_types::WgError;
use wg_types::pyval::prefix;
use wg_wire::convention::json_value;

use super::reads;
use super::require;

/// The candidate limit of the similar-task search and of the lexical fallback (`limit=50`).
const CANDIDATES: usize = 50;

/// The characters of the description the lexical fallback's query keeps (`description[:300]`).
const FALLBACK_CHARS: usize = 300;

/// The message of an estimate with neither a task nor a title (`WorkgraphError`).
const NO_TARGET: &str = "provide a task or a title to estimate";

/// The target task as the estimate reads it: its id, title, description and category, and the
/// repositories it is linked to.
struct Target {
    id: String,
    title: String,
    description: String,
    category: Option<String>,
    repositories: Vec<String>,
}

/// `AnalyticsService.estimate`: the estimate of `task` (or of `title` and `description`, which the
/// lexical fallback reads)
/// within `workspace`. `category` and `repositories` narrow the comparison; `repositories` are
/// names or ids, archived ones included.
#[allow(clippy::too_many_arguments)]
pub fn estimate(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task: Option<&str>,
    title: Option<&str>,
    description: &str,
    category: Option<&str>,
    repositories: &[String],
) -> Result<Value, WgError> {
    require(app, context)?;
    let ws = app.workspace_or_default(workspace)?;
    let (target, repository_ids, input) = app.runtime.store.read(|session| {
        let world = session.task_world(Some(ws), &[])?;
        let slice = session.registry_world(Some(ws))?;
        let target = task
            .map(|reference| {
                let row = resolve::task(&world.tasks, Some(ws), reference)?.clone();
                Ok::<_, WgError>(Target {
                    id: row.id.to_string(),
                    title: row.title.clone(),
                    description: row.description.clone(),
                    category: row.category.clone(),
                    repositories: Vec::new(),
                })
            })
            .transpose()?;
        let repository_ids = repositories
            .iter()
            .map(|reference| {
                resolve::repository(&slice, ws, reference, true).map(|found| found.id.to_string())
            })
            .collect::<Result<Vec<String>, WgError>>()?;
        let input = reads::analytics_input(session)?;
        let target = target.map(|found| {
            let repositories = input
                .task_repositories
                .iter()
                .filter(|link| link.task_id.as_deref() == Some(found.id.as_str()))
                .filter_map(|link| link.repository_id.clone())
                .collect();
            Target {
                repositories,
                ..found
            }
        });
        Ok((target, repository_ids, input))
    })?;
    // The target's own title and description replace the arguments, as the Python command does.
    let (target_id, title, description, category) = match &target {
        Some(found) => (
            Some(found.id.clone()),
            Some(found.title.clone()),
            found.description.clone(),
            category
                .filter(|wanted| !wanted.is_empty())
                .map(str::to_owned)
                .or_else(|| found.category.clone()),
        ),
        None => (
            None,
            title.map(str::to_owned),
            description.to_owned(),
            category.map(str::to_owned),
        ),
    };
    let title = title
        .filter(|text| !text.is_empty())
        .ok_or_else(|| WgError::workgraph(NO_TARGET))?;
    let (method, text_scores) = match similar_scores(app, context, workspace, target_id.as_deref())
    {
        Some((method, scores)) if !scores.is_empty() => (method, scores),
        found => {
            // Without similar tasks the title's lexical hits stand in; the method is the search's.
            let method = found.map_or_else(|| "lexical".to_owned(), |(method, _)| method);
            (method, lexical_scores(app, ws, &title, &description)?)
        }
    };
    let repositories: Vec<String> = target
        .iter()
        .flat_map(|found| found.repositories.iter().cloned())
        .chain(repository_ids)
        .collect();
    let facts = build(&input);
    let request = Request {
        target: target_id.as_deref(),
        category: category.as_deref(),
        repositories: &repositories,
        method: &method,
        text_scores: &text_scores,
    };
    let result = estimate_core(&input, &facts, &ws.to_string(), &request);
    Ok(json_value(&result))
}

/// The similar-task search of the target: its method and the scores of the similar tasks. `None`
/// when there is no target or the search refuses (the Python `WorkgraphError` that is caught).
fn similar_scores(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    target: Option<&str>,
) -> Option<(String, Vec<(String, f64)>)> {
    let id = target?;
    embed::similar(app, context, workspace, id, CANDIDATES, true)
        .ok()
        .map(|found| {
            let scores = found
                .entries
                .iter()
                .map(|entry| (entry.task_id.to_string(), entry.similarity))
                .collect();
            (found.method.to_owned(), scores)
        })
}

/// The lexical search of `title` and the start of `description`: each hit scores `1 / rank`.
fn lexical_scores(
    app: &mut App,
    workspace: wg_types::Id,
    title: &str,
    description: &str,
) -> Result<Vec<(String, f64)>, WgError> {
    let text = format!("{title} {}", prefix(description, FALLBACK_CHARS));
    let ids = app
        .runtime
        .store
        .read(|session| search::lexical_ids(session, workspace, &text, CANDIDATES as i64))?;
    Ok(ids
        .iter()
        .enumerate()
        .map(|(rank, id)| (id.to_string(), 1.0 / (rank as f64 + 1.0)))
        .collect())
}
