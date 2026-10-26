//! Shell: the `classify` reads and the lexical `search` as entry points over `&mut App`. Each loads
//! its rows in one session and hands them to the pure shaping of [`crate::spec`].

use std::collections::BTreeSet;

use serde_json::Value as Json;
use wg_json::Value;
use wg_registry::resolve;
use wg_runtime::App;
use wg_runtimespec::RequestContext;
use wg_store::Session;
use wg_types::{ErrorCode, Id, WgError};

use crate::service::{classify, search};
use crate::spec::classify_out;
use crate::spec::search_out::{self, Candidate, SearchFilters, head_of, scope_of};
use crate::spec::search_sql;

/// `ClassificationService.get`: the classifications of one task.
pub fn classify_show(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let scope = app.workspace_or_all(workspace)?;
    app.runtime.query(
        |session| {
            let world = session.task_world(scope, &[])?;
            let task = resolve::task(&world.tasks, scope, reference)?;
            let (id, key, current) = (
                task.id,
                task.key.to_string(),
                task.current_classification_id,
            );
            let history = classify::classification_history(session, id)?;
            Ok((id, key, current, history))
        },
        |(id, key, current, history)| Ok(classify_out::show_out(*id, key, *current, history)),
    )
}

/// `ClassificationService.gold`: the gold-standard set of the workspace.
pub fn classify_gold(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let ws = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| classify::gold_labels(session, ws),
        |rows| Ok(classify_out::gold_out(rows)),
    )
}

/// `ClassificationService.evaluations`: the evaluations of the workspace, newest first.
pub fn classify_evaluations(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let ws = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| classify::classification_evaluations(session, ws),
        |rows| Ok(classify_out::evaluations_out(rows)),
    )
}

/// `ClassificationService.disagreements`: the tasks whose succeeded classifications disagree.
pub fn classify_disagreements(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let ws = app.workspace_or_default(workspace)?;
    app.runtime.query(
        |session| classify::classifications_with_task_keys(session, ws),
        |rows| {
            let pairs: Vec<_> = rows
                .iter()
                .map(|row| (row.classification.clone(), row.task_key.clone()))
                .collect();
            Ok(classify_out::disagreements_out(&pairs))
        },
    )
}

/// The `provider/model` of the embeddings selection of the workspace (`llm.embeddings`).
fn embedding_selection(app: &mut App, ws: &str) -> Result<(String, String), WgError> {
    let slug = app.workspace_slug(ws)?;
    let config = app.effective_config(Some(&slug))?;
    let read = |field: &str| {
        Json::Object(config.clone())
            .pointer(&format!("/llm/embeddings/{field}"))
            .and_then(Json::as_str)
            .map(str::to_owned)
            .ok_or_else(|| WgError::new(ErrorCode::ValidationFailed, "llm.embeddings is not set"))
    };
    Ok((read("provider")?, read("model")?))
}

/// `SearchService.search` for the lexical leg and the modes that have no semantic leg here.
///
/// `hybrid` and `semantic` consult the stored embeddings of the configured model. When none are
/// stored the result carries the unavailable note and the lexical hits, as Python does. Stored
/// embeddings need a query embedding from the provider, which is not ported, so that case is an
/// error rather than a silent lexical answer.
pub fn search(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    query: &str,
    mode: &str,
    limit: i64,
    filters: &SearchFilters,
) -> Result<Value, WgError> {
    app.require_read(context)?;
    let ws = app.workspace_or_default(workspace)?;
    let lexical_wanted = matches!(mode, "hybrid" | "lexical");
    let semantic_wanted = matches!(mode, "hybrid" | "semantic");
    let expr = search_sql::fts_match(query, true, false).filter(|_| lexical_wanted);
    let selection = if semantic_wanted {
        Some(embedding_selection(app, &ws.to_string())?)
    } else {
        None
    };
    let needs_scope = semantic_wanted || expr.is_some();
    let candidate_limit = search_out::lexical_widened(limit);
    let (candidates, vectors) = app.runtime.query(
        |session: &mut Session<'_>| {
            let world = session.task_world(Some(ws), &[])?;
            let in_scope: BTreeSet<Id> = if needs_scope {
                let scope = scope_of(&world, ws, filters)?;
                search::scoped_task_ids(session, ws, &scope)?
                    .into_iter()
                    .collect()
            } else {
                BTreeSet::new()
            };
            let candidates = match &expr {
                Some(text) => search::fts_candidates(session, text, ws, candidate_limit)?
                    .into_iter()
                    .map(|row| Candidate {
                        task_id: row.task_id,
                        rank: row.rank,
                        snippet: row.snippet,
                        in_scope: in_scope.contains(&row.task_id),
                        task: head_of(&world, row.task_id),
                    })
                    .collect(),
                None => Vec::new(),
            };
            let vectors = match &selection {
                Some((provider, model)) => {
                    search::embedding_vectors(session, ws, provider, model)?.len()
                }
                None => 0,
            };
            Ok((candidates, vectors))
        },
        |(candidates, vectors)| Ok((candidates.clone(), *vectors)),
    )?;
    let unavailable = match &selection {
        Some((provider, model)) if vectors == 0 => {
            Some(search_out::unavailable_note(provider, model))
        }
        Some(_) => {
            return Err(WgError::new(
                ErrorCode::ProviderUnavailable,
                "semantic search over stored embeddings is not available in this build of the Rust rewrite yet (the query embedding needs the LLM provider); use --mode lexical",
            ));
        }
        None => None,
    };
    Ok(search_out::outcome(
        query,
        mode,
        &candidates,
        limit,
        unavailable.as_deref(),
    ))
}
