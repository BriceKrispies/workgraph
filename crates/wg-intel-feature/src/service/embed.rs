//! Shell: the embedding pipeline and similar-task search (`SearchService.embed_task`,
//! `rebuild_embeddings`, `similar`). The text, digest, packing and candidate decisions are
//! `wg-intel`'s; this module loads the rows, calls the provider between transactions, and writes.

use std::collections::BTreeSet;

use wg_intel::{
    Candidates, candidates, content_digest, embedding_text, pack, repository_overlap,
    same_category, similar_score, unpack,
};
use wg_jobs::queue::NewJob;
use wg_model::entity::TaskEmbedding;
use wg_model::{Decision, Row, Write};
use wg_registry::resolve;
use wg_runtime::App;
use wg_runtimespec::RequestContext;
use wg_store::Session;
use wg_types::{Id, Permission, WgError};

use crate::service::llm::Providers;
use crate::service::ops::{GPU_RESOURCE, load_task};
use crate::service::search;
use crate::spec::llm::{embedding_call_error, selection};
use crate::spec::plan::{ids_of, json_text};
use crate::spec::similar_sql;
use crate::spec::write_out::SimilarEntry;

/// The job kind of one embedding.
pub const EMBED_JOB: &str = "embed_task";

/// The number of characters of a description that a similar task's query text keeps.
const SIMILAR_DESCRIPTION_CHARS: usize = 500;

/// The candidate limit of the lexical leg of a similar-task lookup.
const SIMILAR_LEXICAL_LIMIT: i64 = 50;

/// What `embed_task` did: the status and the model, and the vector size when one was stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Embedded {
    pub task_id: Id,
    pub status: &'static str,
    pub provider: String,
    pub model: String,
    pub dimensions: Option<usize>,
}

/// `SearchService.similar`: the method, the embedding model when the lookup was semantic, and the
/// similar tasks, best first.
#[derive(Debug, Clone, PartialEq)]
pub struct Similar {
    pub task_id: Id,
    pub method: &'static str,
    pub embedding_model: Option<String>,
    pub entries: Vec<SimilarEntry>,
}

/// One vector for `text` from the provider `name` under `model`.
fn one_vector(
    providers: &mut dyn Providers,
    config: &serde_json::Map<String, serde_json::Value>,
    name: &str,
    model: &str,
    text: &str,
) -> Result<Vec<f64>, WgError> {
    let mut llm = providers.provider(name, config)?;
    llm.embed(model, &[text.to_owned()])
        .map_err(embedding_call_error)?
        .into_iter()
        .next()
        .ok_or_else(|| WgError::workgraph("embedding provider returned no vector"))
}

/// `SearchService.embed_task`: embeds the task's text under the embedding provider and model (or
/// the given ones), and stores the vector. An unchanged text under the same model is not embedded
/// again.
pub fn embed_task(
    app: &mut App,
    context: &RequestContext,
    providers: &mut dyn Providers,
    task_id: Id,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<Embedded, WgError> {
    let (task, tags) = app.runtime.query(
        |session| load_task(session, task_id),
        |loaded| Ok(loaded.clone()),
    )?;
    let slug = app.workspace_slug(&task.workspace_id.to_string())?;
    let config = app.effective_config(Some(&slug))?;
    let (default_provider, default_model) = selection(&config, "embeddings")?;
    let provider_name = provider.map_or(default_provider, str::to_owned);
    let model_name = model.map_or(default_model, str::to_owned);
    let content = embedding_text(
        &task.title,
        &task.description,
        task.acceptance_criteria.iter().map(String::as_str),
        tags.iter().map(String::as_str),
    );
    let digest = content_digest(&content);
    let existing = app.runtime.query(
        |session| search::task_embedding(session, task_id, &provider_name, &model_name),
        |row| Ok(row.clone()),
    )?;
    if existing
        .as_ref()
        .is_some_and(|row| row.input_hash == digest)
    {
        return Ok(Embedded {
            task_id,
            status: "unchanged",
            provider: provider_name,
            model: model_name,
            dimensions: None,
        });
    }
    let vector = one_vector(providers, &config, &provider_name, &model_name, &content)?;
    let dimensions = i64::try_from(vector.len()).unwrap_or(i64::MAX);
    app.runtime.decide(
        context,
        |session| search::task_embedding(session, task_id, &provider_name, &model_name),
        |existing, stamp| {
            let write = match existing {
                Some(row) => Write::update(Row::TaskEmbedding(TaskEmbedding {
                    vector: pack(&vector).into_iter().collect(),
                    dimensions,
                    input_hash: digest.clone(),
                    ..row.clone()
                })),
                None => Write::insert(Row::TaskEmbedding(TaskEmbedding {
                    id: ids_of(stamp, 1)[0],
                    task_id,
                    provider: provider_name.clone(),
                    model: model_name.clone(),
                    dimensions,
                    vector: pack(&vector).into_iter().collect(),
                    input_hash: digest.clone(),
                    created_at: stamp.at,
                })),
            };
            Ok(Decision::new(()).write(write))
        },
    )?;
    Ok(Embedded {
        task_id,
        status: "embedded",
        provider: provider_name,
        model: model_name,
        dimensions: usize::try_from(dimensions).ok(),
    })
}

/// `SearchService.rebuild_embeddings`: queues an `embed_task` job for every task of the workspace.
/// Returns how many were queued.
pub fn rebuild_embeddings(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<usize, WgError> {
    app.require_permission(context, Permission::ClassifyRequest)?;
    let ws = app.workspace_or_default(workspace)?;
    let ids = app.runtime.query(
        |session| {
            let world = session.task_world(Some(ws), &[])?;
            Ok(world
                .tasks
                .tasks
                .values()
                .map(|task| task.id)
                .collect::<Vec<Id>>())
        },
        |ids| Ok(ids.clone()),
    )?;
    let count = ids.len();
    let ws_short: String = ws.to_string().chars().take(6).collect();
    for task_id in &ids {
        let job = NewJob {
            payload: [
                ("task_id", serde_json::Value::String(task_id.to_string())),
                ("provider", json_text(provider)),
                ("model", json_text(model)),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value))
            .collect(),
            workspace_id: Some(ws),
            task_id: Some(*task_id),
            resource_keys: vec![GPU_RESOURCE.to_owned()],
            max_attempts: 3,
            idempotency_key: Some(format!(
                "embed-rebuild:{task_id}:{}:{}:{count}:{ws_short}",
                provider.unwrap_or("None"),
                model.unwrap_or("None"),
            )),
            created_by: context.actor.to_string(),
            ..NewJob::new(EMBED_JOB)
        };
        app.enqueue_as(context, &job)?;
    }
    Ok(count)
}

/// The query vector of semantic search: the selected embedding provider's vector for `query`.
/// Returns the provider and model it came from, with the vector.
pub fn query_vector(
    app: &mut App,
    providers: &mut dyn Providers,
    ws: Id,
    query: &str,
) -> Result<(String, String, Vec<f64>), WgError> {
    let slug = app.workspace_slug(&ws.to_string())?;
    let config = app.effective_config(Some(&slug))?;
    let (provider, model) = selection(&config, "embeddings")?;
    let vector = one_vector(providers, &config, &provider, &model, query)?;
    Ok((provider, model, vector))
}

/// The stored vectors of a workspace under `provider` and `model`, unpacked, with their task ids.
pub fn stored_vectors(
    session: &mut Session<'_>,
    ws: Id,
    provider: &str,
    model: &str,
) -> Result<Vec<(Id, Vec<f64>)>, WgError> {
    Ok(search::embedding_vectors(session, ws, provider, model)?
        .into_iter()
        .map(|row| {
            let bytes: Vec<u8> = row.vector.iter().copied().collect();
            (row.task_id, unpack(&bytes))
        })
        .collect())
}

/// `SearchService.similar`: the tasks comparable with `task_ref`, by embedding similarity when the
/// target has a stored vector and by full-text similarity otherwise, each with its repository
/// overlap, its linked commits and its attempt count.
pub fn similar(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    task_ref: &str,
    limit: usize,
    completed_only: bool,
) -> Result<Similar, WgError> {
    app.require_permission(context, Permission::TaskRead)?;
    let ws_filter = workspace
        .map(|reference| app.workspace_id(reference))
        .transpose()?;
    let (target, target_repos, lexical) = app.runtime.query(
        |session| {
            let world = session.task_world(ws_filter, &[])?;
            let target = resolve::task(&world.tasks, ws_filter, task_ref)?.clone();
            let repos: BTreeSet<Id> = world
                .tasks
                .repositories_of(target.id)
                .map(|link| link.repository_id)
                .collect();
            let text = format!(
                "{} {}",
                target.title,
                target
                    .description
                    .chars()
                    .take(SIMILAR_DESCRIPTION_CHARS)
                    .collect::<String>()
            );
            let lexical =
                search::lexical_ids(session, target.workspace_id, &text, SIMILAR_LEXICAL_LIMIT)?;
            Ok((target, repos, lexical))
        },
        |loaded| Ok(loaded.clone()),
    )?;
    let ws = target.workspace_id;
    let slug = app.workspace_slug(&ws.to_string())?;
    let config = app.effective_config(Some(&slug))?;
    let (provider, model) = selection(&config, "embeddings")?;
    let stored = app.runtime.query(
        |session| stored_vectors(session, ws, &provider, &model),
        |found| Ok(found.clone()),
    )?;
    let mine = stored
        .iter()
        .find(|(id, _)| *id == target.id)
        .map(|(_, vector)| vector.clone());
    let Candidates { method, similar } = candidates(target.id, mine.as_deref(), &stored, &lexical);
    let rows = app.runtime.query(
        |session| {
            let world = session.task_world(Some(ws), &[])?;
            similar
                .iter()
                .filter_map(|(id, similarity)| {
                    let task = world.tasks.task(*id)?.clone();
                    let repos: BTreeSet<Id> = world
                        .tasks
                        .repositories_of(*id)
                        .map(|link| link.repository_id)
                        .collect();
                    Some((task, repos, *similarity))
                })
                .map(|(task, repos, similarity)| {
                    let commits = session
                        .rows(&similar_sql::implementation_commits(task.id))?
                        .iter()
                        .map(|values| similar_sql::ImplementationCommit::from_raw(values))
                        .collect::<Result<Vec<_>, WgError>>()?;
                    let attempts = match session.rows(&similar_sql::attempt_count(task.id))?.first()
                    {
                        Some(values) => similar_sql::attempts_of(values)?,
                        None => 0,
                    };
                    Ok((task, repos, similarity, commits, attempts))
                })
                .collect::<Result<Vec<_>, WgError>>()
        },
        |rows| Ok(rows.clone()),
    )?;
    let mut entries: Vec<SimilarEntry> = rows
        .into_iter()
        .filter(|(task, _, _, _, _)| !completed_only || task.status.as_str() == "accepted")
        .map(|(task, repos, similarity, commits, attempts)| {
            let overlap = repository_overlap(&target_repos, &repos);
            let same = same_category(target.category.as_deref(), task.category.as_deref());
            SimilarEntry {
                task_id: task.id,
                key: task.key.to_string(),
                title: task.title.clone(),
                status: task.status.as_str().to_owned(),
                category: task.category.clone(),
                score: similar_score(similarity, same, overlap),
                similarity,
                same_category: same,
                overlap,
                commits,
                attempts,
            }
        })
        .collect();
    entries.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    entries.truncate(limit);
    Ok(Similar {
        task_id: target.id,
        method,
        embedding_model: (method == "semantic").then(|| format!("{provider}/{model}")),
        entries,
    })
}
