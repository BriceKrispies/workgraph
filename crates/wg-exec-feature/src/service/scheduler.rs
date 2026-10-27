//! Shell: the scheduler pass and the child proposal (`Scheduler` of
//! `orchestration/scheduler.py`): which tasks may run on their own, one pass that starts them, why
//! a task may not run, and the decomposition of a task by the configured model.

use std::collections::BTreeMap;

use serde_json::{Map, Value as Json};
use wg_json::Value as WgJson;
use wg_model::entity::{AgentAttempt, RepositoryCheckout};
use wg_net::{Endpoint, Net, Ollama, OpenAiCompatible};
use wg_registry::resolve;
use wg_runtime::{App, index_written};
use wg_runtimespec::RequestContext;
use wg_store::Session;
use wg_storecodec::TableRow;
use wg_tasks::edit::enrichment_from;
use wg_types::{Id, WgError};
use wg_wire::dto::scheduler;

use crate::service::agents::AgentHost;
use crate::service::run_flow::run_start;
use crate::spec::config::{
    config_at, execution_enabled, execution_flag, execution_number, provider_failure,
};
use crate::spec::decompose::{
    DECOMPOSE_SYSTEM, Parent, create_children, json_schema, prompt, read_proposal,
    validate_decomposition,
};
use crate::spec::runs::StartRequest;
use crate::spec::schedule::{
    COUNTED_STATUSES, Eligible, World, capacity, eligible, explain as explain_task, idle_report,
    tick_report,
};
use crate::spec::sql::attempts_in;

/// The default base URL of an Ollama provider (`OllamaProviderConfig.base_url`).
const OLLAMA_URL: &str = "http://127.0.0.1:11434";

/// Everything the scheduler reads.
struct Loaded {
    world: wg_storecodec::task_world_sql::TaskWorld,
    attempts: Vec<AgentAttempt>,
    checkouts: BTreeMap<Id, Vec<RepositoryCheckout>>,
}

fn load(session: &mut Session<'_>, workspace: Option<Id>) -> Result<Loaded, WgError> {
    let world = session.task_world(workspace, &[])?;
    let attempts = session
        .rows(&attempts_in(&COUNTED_STATUSES))?
        .iter()
        .map(|values| AgentAttempt::from_raw(values))
        .collect::<Result<Vec<_>, _>>()?;
    let checkouts = world
        .tasks
        .registry
        .repositories
        .keys()
        .map(|id| session.checkouts(*id).map(|rows| (*id, rows)))
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    Ok(Loaded {
        world,
        attempts,
        checkouts,
    })
}

/// `Scheduler.eligible`: the tasks that may run on their own, fairly ordered, at most `limit`.
pub fn eligible_tasks(
    app: &mut App,
    workspace: Option<Id>,
    limit: usize,
) -> Result<Vec<Eligible>, WgError> {
    let max_attempts = execution_number(&app.config()?, "max_attempts_per_task", 3);
    let loaded = app.runtime.store.read(|session| load(session, workspace))?;
    Ok(eligible(
        &World {
            tasks: &loaded.world.tasks,
            stacks: &loaded.world.stacks,
            attempts: &loaded.attempts,
            checkouts: &loaded.checkouts,
        },
        workspace,
        max_attempts,
        limit,
    ))
}

/// The id of the task `task_ref` names in `workspace` (all workspaces when none), as `run
/// decompose` looks it up before it proposes children.
pub fn task_id_of(app: &mut App, workspace: Option<&str>, task_ref: &str) -> Result<Id, WgError> {
    let scope = app.workspace_or_all(workspace)?;
    app.runtime.store.read(|session| {
        let world = session.task_world(scope, &[])?;
        resolve::task(&world.tasks, scope, task_ref).map(|task| task.id)
    })
}

/// `Scheduler.explain`: whether one task is eligible and, when it is not, why.
pub fn explain(app: &mut App, workspace: Option<&str>, task_ref: &str) -> Result<Json, WgError> {
    let config = app.config()?;
    let scope = app.workspace_or_all(workspace)?;
    let loaded = app.runtime.store.read(|session| load(session, scope))?;
    let task = resolve::task(&loaded.world.tasks, scope, task_ref)?;
    Ok(explain_task(
        &World {
            tasks: &loaded.world.tasks,
            stacks: &loaded.world.stacks,
            attempts: &loaded.attempts,
            checkouts: &loaded.checkouts,
        },
        task,
        execution_enabled(&config),
        execution_number(&config, "max_attempts_per_task", 3),
    ))
}

/// `GET /ws/{ws}/scheduler` (`api/routes/ops.py`): the execution flags and the first 50 eligible
/// tasks of the workspace, as `SchedulerOut`. The caller has checked `task:read`.
pub fn overview(app: &mut App, workspace: Option<Id>) -> Result<WgJson, WgError> {
    let config = app.config()?;
    let eligible = eligible_tasks(app, workspace, 50)?;
    let entries: Vec<WgJson> = eligible
        .iter()
        .map(|item| {
            let project = item.project_id.map(|id| id.to_string());
            scheduler::eligible_entry(
                &item.task_id.to_string(),
                &item.key,
                item.priority.as_str(),
                project.as_deref(),
                item.agent.as_deref(),
                &item.workspace_id.to_string(),
            )
        })
        .collect();
    Ok(scheduler::from_parts(
        execution_enabled(&config),
        execution_flag(&config, "auto_schedule"),
        entries,
    ))
}

/// `Scheduler.tick`: starts as many eligible tasks as the concurrency limit leaves room for. A
/// task that cannot be started is reported in `errors` and does not stop the pass.
pub fn tick(
    app: &mut App,
    context: &RequestContext,
    host: &mut dyn AgentHost,
) -> Result<Json, WgError> {
    let config = app.config()?;
    if !execution_enabled(&config) {
        return Ok(idle_report("execution disabled"));
    }
    let attempts = app
        .runtime
        .store
        .read(|session| load(session, None).map(|loaded| loaded.attempts))?;
    let room = capacity(
        execution_number(&config, "max_concurrent_runs", 2),
        &attempts,
    );
    if room == 0 {
        return Ok(idle_report("at capacity"));
    }
    let mut started = Vec::new();
    let mut errors = Vec::new();
    for item in eligible_tasks(app, None, room * 3)? {
        if started.len() >= room {
            break;
        }
        let request = StartRequest {
            agent: item.agent.clone(),
            ..StartRequest::default()
        };
        match run_start(
            app,
            context,
            &mut *host,
            None,
            &item.task_id.to_string(),
            &request,
        ) {
            Ok(attempt) => {
                let id = wg_clispec::args::get(&attempt, "id")
                    .map(wg_clispec::args::python)
                    .and_then(|text| wg_types::parse_id(&text).ok());
                if let Some(id) = id {
                    started.push((item.key.clone(), id));
                }
            }
            Err(error) => errors.push((item.key.clone(), error.message.clone())),
        }
    }
    Ok(tick_report(started, errors))
}

/// The credential an entry names in `api_key_env`.
fn api_key(entry: &Map<String, Json>) -> Option<String> {
    entry
        .get("api_key_env")
        .and_then(Json::as_str)
        .and_then(|name| std::env::var(name).ok())
}

/// `provider.generate_json(model, system, prompt, schema)` on the configured provider `name`:
/// the structured data and the model that answered.
fn generate(
    config: &Map<String, Json>,
    name: &str,
    model: &str,
    text: &str,
) -> Result<(Map<String, Json>, String), WgError> {
    let providers = config_at(config, &["llm", "providers"])
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let Some(entry) = providers.get(name).and_then(Json::as_object) else {
        let mut configured: Vec<&String> = providers.keys().collect();
        configured.sort();
        return Err(WgError::not_found(format!(
            "inference provider {} is not configured",
            wg_model::py_repr_str(name)
        ))
        .detail(
            "configured",
            Json::Array(
                configured
                    .into_iter()
                    .map(|n| Json::String(n.clone()))
                    .collect(),
            ),
        ));
    };
    let schema = json_schema();
    match entry.get("type").and_then(Json::as_str) {
        Some("ollama") => {
            let mut client = Ollama {
                endpoint: Endpoint::from_config(entry, OLLAMA_URL, api_key(entry)),
                net: Net::default(),
            };
            let made = client
                .generate_json(model, DECOMPOSE_SYSTEM, text, &schema, 0.0)
                .map_err(|error| provider_failure(&error))?;
            Ok((made.output.data, made.output.model))
        }
        Some("openai_compatible") => {
            let mut client = OpenAiCompatible {
                name: name.to_owned(),
                endpoint: Endpoint::from_config(entry, "", api_key(entry)),
                net: Net::default(),
            };
            let made = client
                .generate_json(model, DECOMPOSE_SYSTEM, text, &schema, 0.0)
                .map_err(|error| provider_failure(&error))?;
            Ok((made.output.data, made.output.model))
        }
        _ => {
            let data = wg_intel::generate_json(Some("TaskDecomposition"), text)
                .map_err(|error| WgError::workgraph(error.to_string()))?;
            Ok((
                data.as_object().cloned().unwrap_or_default(),
                wg_intel::MODEL.to_owned(),
            ))
        }
    }
}

/// `Scheduler.decompose`: asks the configured model for child tasks, validates them and, with
/// `create`, creates them in dependency order under the task.
pub fn decompose(
    app: &mut App,
    context: &RequestContext,
    task_id: Id,
    create: bool,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<Json, WgError> {
    let (task, world) = app.runtime.store.read(|session| {
        let task = session
            .fetch::<wg_model::Task>(&wg_model::RowKey::Task(task_id))?
            .ok_or_else(|| WgError::not_found(format!("task {task_id} not found")))?;
        let world = session.task_world(Some(task.workspace_id), &[])?;
        Ok((task, world))
    })?;
    let slug = app.workspace_slug(&task.workspace_id.to_string())?;
    let effective = app.effective_config(Some(&slug))?;
    let repositories: Vec<String> = world
        .tasks
        .repositories_of(task.id)
        .filter_map(|link| world.tasks.registry.repository(link.repository_id))
        .map(|repo| repo.name.clone())
        .collect();
    let criteria: Vec<String> = task.acceptance_criteria.iter().cloned().collect();
    let asked = prompt(&repositories, &task.title, &task.description, &criteria);
    let selected = |field: &str| {
        config_at(&effective, &["llm", "summarization", field])
            .and_then(Json::as_str)
            .map(str::to_owned)
    };
    let provider_name = provider
        .map(str::to_owned)
        .or_else(|| selected("provider"))
        .unwrap_or_default();
    let model_name = model
        .map(str::to_owned)
        .or_else(|| selected("model"))
        .unwrap_or_default();
    let (data, answered) = generate(&effective, &provider_name, &model_name, &asked)?;
    let proposal = read_proposal(&data)?;
    let order = validate_decomposition(&proposal, &repositories)?;
    let key = task.key.to_string();
    let report = |created: Vec<String>| {
        Json::Object(Map::from_iter([
            ("task".to_owned(), Json::String(key.clone())),
            ("proposal".to_owned(), proposal.dump.clone()),
            (
                "order".to_owned(),
                Json::Array(order.iter().cloned().map(Json::String).collect()),
            ),
            ("provider".to_owned(), Json::String(provider_name.clone())),
            ("model".to_owned(), Json::String(answered.clone())),
            (
                "created".to_owned(),
                Json::Array(created.into_iter().map(Json::String).collect()),
            ),
        ]))
    };
    if !create {
        return Ok(report(Vec::new()));
    }
    let enrich = enrichment_from(&app.config()?);
    let parent = Parent {
        task: task.id,
        workspace: task.workspace_id,
        repositories: &repositories,
        provider: &provider_name,
        model: &answered,
    };
    let made = app.runtime.decide_then(
        context,
        |session| session.task_world(Some(task.workspace_id), &[]),
        |fresh, stamp| create_children(fresh, stamp, &parent, &proposal, &order, enrich),
        index_written,
    )?;
    Ok(report(made.reply))
}
