//! Shell: the `classify` reads and writes and the `search` command as entry points over `&mut App`.
//! Each loads its rows in one session and hands them to the pure shaping of [`crate::spec`].

use std::collections::BTreeSet;

use wg_json::Value;
use wg_registry::resolve;
use wg_runtime::App;
use wg_runtimespec::RequestContext;

use wg_types::{Id, WgError};

use crate::service::classify;
use crate::service::embed;
use crate::service::llm::Providers;
use crate::service::ops;
use crate::service::search;
use crate::spec::classify_out;
use crate::spec::llm::selection;
use crate::spec::plan::parse_id;
use crate::spec::requests::{ConfirmRequest, ReclassifyRequest, RunOptions, SearchRequest};
use crate::spec::search_out::{self, Candidate, SearchFilters, head_of, scope_of};
use crate::spec::search_sql;
use crate::spec::semantic_out::{self, Semantic};
use crate::spec::write_out;

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

/// `classify request`: queues a classification job for the task.
pub fn classify_request(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    options: &RunOptions,
) -> Result<Value, WgError> {
    let (job_id, task, status) = ops::request(app, context, workspace, reference, options)?;
    Ok(write_out::request_out(
        job_id,
        task.id,
        &task.key.to_string(),
        &status,
    ))
}

/// `classify request --now`: classifies the task in this process and returns the stored row.
pub fn classify_now(
    app: &mut App,
    context: &RequestContext,
    providers: &mut dyn Providers,
    workspace: Option<&str>,
    reference: &str,
    options: &RunOptions,
) -> Result<Value, WgError> {
    let ws = workspace.map(|value| app.workspace_id(value)).transpose()?;
    let task_id = app.runtime.query(
        |session| {
            let world = session.task_world(ws, &[])?;
            Ok(resolve::task(&world.tasks, ws, reference)?.id)
        },
        |id| Ok(*id),
    )?;
    let row = ops::run_classification(app, context, providers, task_id, options)?;
    Ok(classify_out::classification_out(&row))
}

/// `classify promote`: makes a succeeded classification current.
pub fn classify_promote(
    app: &mut App,
    context: &RequestContext,
    classification_id: &str,
) -> Result<Value, WgError> {
    let id = parse_id(
        classification_id,
        &format!("successful classification {classification_id} not found"),
    )?;
    let row = ops::promote(app, context, id)?;
    Ok(classify_out::classification_out(&row))
}

/// `classify confirm`: records a human classification.
pub fn classify_confirm(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    request: &ConfirmRequest,
) -> Result<Value, WgError> {
    let row = ops::confirm(app, context, workspace, reference, request)?;
    Ok(classify_out::classification_out(&row))
}

/// `classify reclassify`: queues a classification job for each matching task of the workspace.
pub fn classify_reclassify(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    request: &ReclassifyRequest,
) -> Result<Value, WgError> {
    let reclassified = ops::reclassify(app, context, workspace, request)?;
    Ok(write_out::reclassify_out(
        reclassified.batch_id,
        &reclassified.job_ids,
        reclassified.total_matching,
    ))
}

/// `classify gold --category`: adds a gold label from a task, or from a title and description.
pub fn classify_gold_add(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    category: &str,
    task_ref: Option<&str>,
    title: Option<&str>,
) -> Result<Value, WgError> {
    let label = ops::add_gold(app, context, workspace, category, task_ref, title, "")?;
    Ok(write_out::gold_added_out(
        label.id,
        &label.title,
        &label.category,
    ))
}

/// `classify evaluate`: runs a provider and model over the gold set.
pub fn classify_evaluate(
    app: &mut App,
    context: &RequestContext,
    providers: &mut dyn Providers,
    workspace: Option<&str>,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<Value, WgError> {
    let evaluated = ops::evaluate(app, context, providers, workspace, provider, model)?;
    Ok(write_out::evaluation_out(
        &evaluated.row,
        &evaluated.metrics,
        &evaluated.results,
    ))
}

/// `search --reindex`: queues an embedding job for every task of the workspace.
pub fn search_reindex(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<Value, WgError> {
    let enqueued = embed::rebuild_embeddings(app, context, workspace, provider, model)?;
    Ok(write_out::rebuild_out(enqueued, provider, model))
}

/// `search --similar-to`: the tasks similar to the one `reference` names.
pub fn search_similar(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
    limit: usize,
) -> Result<Value, WgError> {
    let similar = embed::similar(app, context, workspace, reference, limit, false)?;
    Ok(write_out::similar_out(
        similar.task_id,
        similar.method,
        similar.embedding_model.as_deref(),
        &similar.entries,
    ))
}

/// `SearchService.search`: the lexical leg (full-text), the semantic leg (cosine over the stored
/// vectors of the configured embedding model) or both fused by reciprocal rank.
///
/// When the semantic leg cannot answer (no stored vectors, or the provider is unavailable), the
/// result carries the note and the lexical hits, as Python does; in `semantic` mode it is empty.
pub fn search(
    app: &mut App,
    context: &RequestContext,
    providers: &mut dyn Providers,
    workspace: Option<&str>,
    request: &SearchRequest,
) -> Result<Value, WgError> {
    let (query, mode, limit, filters) = (
        request.query.as_str(),
        request.mode.as_str(),
        request.limit,
        &request.filters,
    );
    app.require_read(context)?;
    let ws = app.workspace_or_default(workspace)?;
    let lexical_wanted = matches!(mode, "hybrid" | "lexical");
    let semantic_wanted = matches!(mode, "hybrid" | "semantic");
    let lexical = if lexical_wanted {
        lexical_leg(app, ws, query, filters, search_out::lexical_widened(limit))?
    } else {
        Vec::new()
    };
    let semantic = if semantic_wanted {
        semantic_leg(
            app,
            providers,
            ws,
            query,
            filters,
            search_out::lexical_widened(limit),
        )
    } else {
        Ok(None)
    };
    match semantic {
        Err(error) => {
            let note = format!(
                "semantic search unavailable: {}; lexical results only",
                error.message
            );
            if mode == "semantic" {
                return Ok(semantic_out::result_value(
                    query,
                    mode,
                    false,
                    None,
                    &[],
                    Some(&note),
                ));
            }
            Ok(semantic_out::result_value(
                query,
                "lexical",
                false,
                None,
                &semantic_out::lexical_out_hits(&search_out::lexical_hits(&lexical, limit)),
                Some(&note),
            ))
        }
        Ok(None) => Ok(semantic_out::result_value(
            query,
            "lexical",
            false,
            None,
            &semantic_out::lexical_out_hits(&search_out::lexical_hits(&lexical, limit)),
            None,
        )),
        Ok(Some((model, hits))) if hits.is_empty() => {
            // No stored vector in scope: the lexical answer, with the model the leg used.
            let (shown_mode, shown) = if mode == "semantic" {
                ("semantic", Vec::new())
            } else {
                (
                    "lexical",
                    semantic_out::lexical_out_hits(&search_out::lexical_hits(&lexical, limit)),
                )
            };
            Ok(semantic_out::result_value(
                query,
                shown_mode,
                false,
                Some(&model),
                &shown,
                None,
            ))
        }
        Ok(Some((model, hits))) => {
            if mode == "semantic" {
                let shown = semantic_out::semantic_hits(&hits, limit);
                return Ok(semantic_out::result_value(
                    query,
                    mode,
                    true,
                    Some(&model),
                    &shown,
                    None,
                ));
            }
            let lexical_hits = semantic_out::lexical_out_hits(&search_out::lexical_hits(
                &lexical,
                search_out::lexical_widened(limit),
            ));
            let semantic_hits =
                semantic_out::semantic_hits(&hits, search_out::lexical_widened(limit));
            let fused = semantic_out::fuse(&lexical_hits, &semantic_hits, limit);
            Ok(semantic_out::result_value(
                query,
                "hybrid",
                true,
                Some(&model),
                &fused,
                Some(
                    "score = reciprocal rank fusion of lexical (bm25) and semantic (cosine) rankings",
                ),
            ))
        }
    }
}

/// The lexical candidates of `query` in the workspace, in rank order.
fn lexical_leg(
    app: &mut App,
    ws: Id,
    query: &str,
    filters: &SearchFilters,
    candidate_limit: i64,
) -> Result<Vec<Candidate>, WgError> {
    let Some(expr) = search_sql::fts_match(query, true, false) else {
        return Ok(Vec::new());
    };
    app.runtime.query(
        |session| {
            let world = session.task_world(Some(ws), &[])?;
            let scope = scope_of(&world, ws, filters)?;
            let in_scope: BTreeSet<Id> = search::scoped_task_ids(session, ws, &scope)?
                .into_iter()
                .collect();
            let rows = search::fts_candidates(session, &expr, ws, candidate_limit)?;
            Ok(rows
                .into_iter()
                .map(|row| Candidate {
                    task_id: row.task_id,
                    rank: row.rank,
                    snippet: row.snippet,
                    in_scope: in_scope.contains(&row.task_id),
                    task: head_of(&world, row.task_id),
                })
                .collect::<Vec<Candidate>>())
        },
        |candidates| Ok(candidates.clone()),
    )
}

/// The semantic leg: `None` when the mode has none, else the model and the ranked hits. The
/// error is the reason the leg could not answer.
fn semantic_leg(
    app: &mut App,
    providers: &mut dyn Providers,
    ws: Id,
    query: &str,
    filters: &SearchFilters,
    candidate_limit: i64,
) -> Result<Option<(String, Vec<Semantic>)>, WgError> {
    let slug = app.workspace_slug(&ws.to_string())?;
    let config = app.effective_config(Some(&slug))?;
    let (provider, model) = selection(&config, "embeddings")?;
    let (vectors, in_scope, tasks) = app.runtime.query(
        |session| {
            let world = session.task_world(Some(ws), &[])?;
            let scope = scope_of(&world, ws, filters)?;
            let in_scope: BTreeSet<Id> = search::scoped_task_ids(session, ws, &scope)?
                .into_iter()
                .collect();
            let vectors = embed::stored_vectors(session, ws, &provider, &model)?;
            Ok((vectors, in_scope, world.tasks.clone()))
        },
        |loaded| Ok(loaded.clone()),
    )?;
    if vectors.is_empty() {
        return Err(WgError::provider_unavailable(format!(
            "no embeddings stored for {provider}/{model}; run `workgraph search reindex`"
        )));
    }
    let (_, _, query_vector) = embed::query_vector(app, providers, ws, query)?;
    let mut scored: Vec<(f64, Id)> = vectors
        .iter()
        .filter(|(id, _)| in_scope.contains(id))
        .map(|(id, vector)| (wg_intel::cosine(&query_vector, vector), *id))
        .collect();
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.1.cmp(&a.1))
    });
    let hits: Vec<Semantic> = scored
        .into_iter()
        .take(usize::try_from(candidate_limit.max(0)).unwrap_or(usize::MAX))
        .filter_map(|(similarity, id)| {
            tasks.task(id).map(|task| Semantic {
                task_id: id,
                head: search_out::TaskHead {
                    key: task.key.to_string(),
                    title: task.title.clone(),
                    status: task.status.as_str().to_owned(),
                    category: task.category.clone(),
                },
                similarity,
            })
        })
        .collect();
    Ok(Some((format!("{provider}/{model}"), hits)))
}
