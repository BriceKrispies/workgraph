//! The workspace, project and repository tools (M01 to M08): `workspace_list`, `workspace_get`,
//! `project_list`, `project_get`, `repository_list`, `repository_get`, `repository_register` and
//! `repository_status`.
//!
//! Each tool resolves its references through `wg-registry` over a slice loaded from the store,
//! reads the rows its answer needs, and shapes them with the `wg-wire` DTOs. A registration is one
//! `wg-registry` decision applied through the runtime; the Git facts it needs are read by `wg-git`
//! before the decision. The answer is the `wg-json` value the DTOs build, converted once into the
//! structured output.

use serde_json::Value;
use wg_json::Value as Dto;
use wg_mcpspec::{structured_output, text_argument};
use wg_model::RegistrySlice;
use wg_model::entity::{Project, Repository};
use wg_registry::resolve;
use wg_runtime::{App, ToolRun, agent_context};

use crate::service::repos::repository_out;
use crate::service::{self, reads};
use crate::spec::dto::{project, project_summary, workspace, workspace_summary};
use crate::spec::mcp_shapes;
use crate::spec::repos::RegisterInput;
use wg_store::Session;
use wg_types::{ErrorCode, Id, WgError};
use wg_wire::dto::task_brief;

use wg_mcpspec::options::Agent;

/// Answers tool `name` when this module owns it; `None` leaves the call to the backend's fallback.
pub fn call(
    app: &mut App,
    agent: &Agent,
    name: &str,
    arguments: &Value,
) -> Option<Result<Value, WgError>> {
    let answer = match name {
        "workspace_list" => workspace_list(app),
        "workspace_get" => workspace_get(app, arguments),
        "project_list" => project_list(app, arguments),
        "project_get" => project_get(app, arguments),
        "repository_list" => repository_list(app, arguments),
        "repository_get" => repository_get(app, arguments),
        "repository_register" => repository_register(app, agent, arguments),
        "repository_status" => repository_status(app, agent, arguments),
        _ => return None,
    };
    Some(answer.and_then(|dto| structured_output(&dto)))
}

/// The workspace a call names, or the default one when it names none.
fn workspace_of(app: &mut App, arguments: &Value) -> Result<Id, WgError> {
    match text_argument(arguments, "workspace") {
        Some(reference) => app.workspace_id(reference),
        None => app.default_workspace_id(),
    }
}

/// The rows that references in workspace `ws` resolve against (see `Session::registry_slice`).
fn registry(session: &mut Session<'_>, ws: Id) -> Result<RegistrySlice, WgError> {
    reads::registry_slice(session, ws)
}

/// `ProjectOut` for `project`: its repository ids in the order the store lists them.
fn project_dto(session: &mut Session<'_>, project_row: &Project) -> Result<Dto, WgError> {
    let links = session.project_repositories(project_row.id)?;
    Ok(project::from_model(
        project_row,
        links.iter().map(|link| link.repository_id),
    ))
}

/// `RepositoryOut` for `repo` (see `service::repos::repository_out`).
fn repository_dto(
    session: &mut Session<'_>,
    registry: &RegistrySlice,
    repo: &Repository,
) -> Result<Dto, WgError> {
    repository_out(session, registry, repo)
}

fn workspace_list(app: &mut App) -> Result<Dto, WgError> {
    let rows = app.runtime.store.read(|session| session.workspaces())?;
    let items = rows
        .iter()
        .filter(|row| row.archived_at.is_none())
        .map(workspace::from_model)
        .collect();
    Ok(mcp_shapes::workspaces(items))
}

fn workspace_get(app: &mut App, arguments: &Value) -> Result<Dto, WgError> {
    let ws = workspace_of(app, arguments)?;
    app.runtime.store.read(|session| {
        let row = session.workspace_by_id(ws)?.ok_or_else(|| {
            WgError::new(ErrorCode::NotFound, format!("workspace {ws} not found"))
        })?;
        let jobs = reads::workspace_job_counts(session, ws)?;
        let job_count = |status: &str| {
            jobs.iter()
                .find(|(key, _)| key == status)
                .map_or(0, |(_, count)| *count)
        };
        let counts = workspace_summary::Counts {
            projects: i64::try_from(session.projects(ws, true)?.len()).unwrap_or(i64::MAX),
            repositories: i64::try_from(session.repositories(ws, None, false)?.len())
                .unwrap_or(i64::MAX),
            tasks_by_status: reads::workspace_task_counts(session, ws)?,
            running_attempts: reads::workspace_active_attempts(session, ws)?,
            queued_jobs: job_count("queued"),
            dead_jobs: job_count("dead"),
        };
        Ok(workspace_summary::from_model(&row, &counts))
    })
}

fn project_list(app: &mut App, arguments: &Value) -> Result<Dto, WgError> {
    let ws = workspace_of(app, arguments)?;
    let items = app.runtime.store.read(|session| {
        let mut items = Vec::new();
        for project_row in session.projects(ws, false)? {
            items.push(project_dto(session, &project_row)?);
        }
        Ok(items)
    })?;
    Ok(mcp_shapes::projects(items))
}

fn project_get(app: &mut App, arguments: &Value) -> Result<Dto, WgError> {
    let ws = workspace_of(app, arguments)?;
    let reference = text_argument(arguments, "project").unwrap_or_default();
    app.runtime.store.read(|session| {
        let registry = registry(session, ws)?;
        let project_row = resolve::project(&registry, ws, reference)?.clone();
        let repositories = session.repositories(ws, Some(project_row.id), true)?;
        let tasks_by_status = reads::project_task_counts(session, project_row.id)?;
        let mut recent = Vec::new();
        for task_row in reads::project_recent_tasks(session, project_row.id, RECENT_TASKS)? {
            recent.push(task_brief::from_model(&task_row));
        }
        let project_value = project_dto(session, &project_row)?;
        Ok(project_summary::from_model(
            project_value,
            &repositories,
            &tasks_by_status,
            recent,
        ))
    })
}

fn repository_list(app: &mut App, arguments: &Value) -> Result<Dto, WgError> {
    let ws = workspace_of(app, arguments)?;
    let project_ref = text_argument(arguments, "project");
    let items = app.runtime.store.read(|session| {
        let registry = registry(session, ws)?;
        let project_id = match project_ref {
            Some(reference) => Some(resolve::project(&registry, ws, reference)?.id),
            None => None,
        };
        let mut items = Vec::new();
        for repo in session.repositories(ws, project_id, false)? {
            items.push(repository_dto(session, &registry, &repo)?);
        }
        Ok(items)
    })?;
    Ok(mcp_shapes::repositories(items))
}

fn repository_get(app: &mut App, arguments: &Value) -> Result<Dto, WgError> {
    let ws = workspace_of(app, arguments)?;
    let reference = text_argument(arguments, "repository").unwrap_or_default();
    app.runtime.store.read(|session| {
        let registry = registry(session, ws)?;
        let repo = resolve::repository(&registry, ws, reference, true)?.clone();
        repository_dto(session, &registry, &repo)
    })
}

/// `repository_register`: `RepositoryService.register`, run by the application.
fn repository_register(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Dto, WgError> {
    let input = RegisterInput {
        path: text_argument(arguments, "path")
            .unwrap_or_default()
            .to_owned(),
        name: text_argument(arguments, "name").map(str::to_owned),
        read_only: arguments
            .get("read_only")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        ..RegisterInput::default()
    };
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::repo_register(app, &context, text_argument(arguments, "workspace"), &input)
}

/// `repository_status`: the working tree of the repository's primary checkout.
fn repository_status(app: &mut App, agent: &Agent, arguments: &Value) -> Result<Dto, WgError> {
    let reference = text_argument(arguments, "repository").unwrap_or_default();
    let context = agent_context(&agent.name, Some(agent.permissions.clone()))?;
    service::repo_status(
        app,
        &context,
        text_argument(arguments, "workspace"),
        reference,
    )
}

/// Recent tasks a project summary lists (`ProjectService.summary` takes ten).
const RECENT_TASKS: i64 = 10;

/// The feature's tool descriptor: one [`call`] that answers the registry tool names.
pub const TOOLS: &[ToolRun] = &[call];
