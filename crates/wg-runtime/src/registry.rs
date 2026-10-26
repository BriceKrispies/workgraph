//! The workspace and project entry points of the application (IF-03): the services behind
//! `workspace create, list, show, use, archive, set` and `project create, list, show, update,
//! add-repo, remove-repo, rename-term`.
//!
//! Each one resolves the workspace, loads the registry slice in the transaction of the call,
//! hands it to the decision of `wg_registry` and applies what it returns. The configured default
//! workspace lives in `config.toml`, so the entry points that change it also write that file.

use std::path::Path;

use serde_json::{Map, Value as Json, json};
use wg_json::Value;
use wg_model::entity::VerificationConfig;
use wg_registry::{projects, renames, resolve, workspaces};
use wg_runtimespec::RequestContext;
use wg_runtimespec::registry::{
    checks_of, missing_paths, project_out, project_summary_out, project_update_body, rename_inputs,
    rename_report, task_update_body, validate_project_create, validate_project_update,
    validate_workspace_create, workspace_counts, workspace_out, workspace_summary_out,
};
use wg_tasks::rename::{RenamePlan, Term, plan_rename};
use wg_types::{Id, Permission, WgError};

use crate::app::App;

/// The `(target, message)` of an update the rename could not apply.
type Failure = (String, String);

impl App {
    /// A permission check against a fresh stamp.
    fn require(&mut self, context: &RequestContext, permission: Permission) -> Result<(), WgError> {
        self.runtime.stamps.stamp(context)?.require(&[permission])
    }

    /// `config.default_workspace`.
    pub fn default_workspace_slug(&mut self) -> Result<Option<String>, WgError> {
        Ok(self
            .config()?
            .get("default_workspace")
            .and_then(Json::as_str)
            .map(str::to_owned))
    }

    /// `WorkspaceService.create`: the workspace, its override file, and the default when asked or
    /// when there is none.
    pub fn workspace_create(
        &mut self,
        context: &RequestContext,
        body: &Json,
        make_default: bool,
    ) -> Result<Value, WgError> {
        let dump = validate_workspace_create(body)?;
        let row = self
            .runtime
            .decide(
                context,
                |session| session.registry_world(None),
                |slice, stamp| workspaces::create(slice, stamp, &dump),
            )?
            .reply;
        self.adopt_workspace(&row.slug, make_default)?;
        Ok(workspace_out(&row))
    }

    /// `WorkspaceService.list`: by slug, archived ones only when asked.
    pub fn workspace_list(
        &mut self,
        context: &RequestContext,
        include_archived: bool,
    ) -> Result<Value, WgError> {
        self.require(context, Permission::WorkspaceRead)?;
        let rows = self.runtime.store.read(|session| session.workspaces())?;
        Ok(Value::List(
            rows.iter()
                .filter(|row| include_archived || row.archived_at.is_none())
                .map(workspace_out)
                .collect(),
        ))
    }

    /// `WorkspaceService.summary`: the named workspace, else the default one.
    pub fn workspace_summary(
        &mut self,
        context: &RequestContext,
        reference: Option<&str>,
    ) -> Result<Value, WgError> {
        self.require(context, Permission::WorkspaceRead)?;
        let reference = match reference {
            Some(reference) => reference.to_owned(),
            None => self.default_workspace_id()?.to_string(),
        };
        self.runtime.query(
            |session| {
                let slice = session.registry_world(None)?;
                let row = resolve::workspace(&slice, &reference)?.clone();
                let counts = workspace_counts(
                    session.projects(row.id, true)?.len(),
                    session.repositories(row.id, None, false)?.len(),
                    session.workspace_task_counts(row.id)?,
                    session.workspace_active_attempts(row.id)?,
                    &session.workspace_job_counts(row.id)?,
                );
                Ok((row, counts))
            },
            |(row, counts)| Ok(workspace_summary_out(row, counts)),
        )
    }

    /// `WorkspaceService.set_default`: records the workspace as the configured default.
    pub fn workspace_use(
        &mut self,
        context: &RequestContext,
        reference: &str,
    ) -> Result<Value, WgError> {
        let stamp = self.runtime.stamps.stamp(context)?;
        let row = self.runtime.query(
            |session| session.registry_world(None),
            |slice| workspaces::named(slice, &stamp, reference),
        )?;
        self.save_default_workspace(&row.slug)?;
        Ok(workspace_out(&row))
    }

    /// `WorkspaceService.archive`.
    pub fn workspace_archive(
        &mut self,
        context: &RequestContext,
        reference: &str,
    ) -> Result<Value, WgError> {
        let row = self
            .runtime
            .decide(
                context,
                |session| session.registry_world(None),
                |slice, stamp| workspaces::archive(slice, stamp, reference),
            )?
            .reply;
        Ok(workspace_out(&row))
    }

    /// `WorkspaceService.update_settings` with `allowed_roots`.
    pub fn workspace_set_roots(
        &mut self,
        context: &RequestContext,
        reference: &str,
        roots: &[String],
    ) -> Result<Value, WgError> {
        let update: Map<String, Json> = [("allowed_roots".to_owned(), json!(roots))]
            .into_iter()
            .collect();
        let row = self
            .runtime
            .decide(
                context,
                |session| session.registry_world(None),
                |slice, stamp| workspaces::update_settings(slice, stamp, reference, &update),
            )?
            .reply;
        Ok(workspace_out(&row))
    }

    /// `ProjectService.create`.
    pub fn project_create(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        body: &Json,
    ) -> Result<Value, WgError> {
        let dump = validate_project_create(body)?;
        let scope = self.workspace_or_default(workspace)?;
        let view = self
            .runtime
            .decide(
                context,
                |session| session.registry_world(Some(scope)),
                |slice, stamp| projects::create(slice, stamp, scope, &dump),
            )?
            .reply;
        Ok(project_out(&view))
    }

    /// `ProjectService.list`: by slug, archived projects only when asked.
    pub fn project_list(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        include_archived: bool,
    ) -> Result<Value, WgError> {
        self.require(context, Permission::WorkspaceRead)?;
        let scope = self.workspace_or_default(workspace)?;
        self.runtime.query(
            |session| {
                let slice = session.registry_world(Some(scope))?;
                let rows = session.projects(scope, include_archived)?;
                rows.iter()
                    .map(|row| projects::view(&slice, scope, &row.id.to_string()))
                    .collect::<Result<Vec<_>, _>>()
            },
            |views| Ok(Value::List(views.iter().map(project_out).collect())),
        )
    }

    /// `ProjectService.summary`.
    pub fn project_summary(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
    ) -> Result<Value, WgError> {
        self.require(context, Permission::WorkspaceRead)?;
        let scope = self.workspace_or_default(workspace)?;
        self.runtime.query(
            |session| {
                let view = projects::view(&session.registry_world(Some(scope))?, scope, reference)?;
                let id = view.project.id;
                Ok((
                    view,
                    session.repositories(scope, Some(id), true)?,
                    session.project_task_counts(id)?,
                    session.project_recent_tasks(id, 10)?,
                ))
            },
            |(view, repositories, counts, recent)| {
                Ok(project_summary_out(view, repositories, counts, recent))
            },
        )
    }

    /// `ProjectService.update`.
    pub fn project_update(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        body: &Json,
    ) -> Result<Value, WgError> {
        let dump = validate_project_update(body)?;
        let scope = self.workspace_or_default(workspace)?;
        let view = self
            .runtime
            .decide(
                context,
                |session| session.registry_world(Some(scope)),
                |slice, stamp| projects::update(slice, stamp, scope, reference, &dump),
            )?
            .reply;
        Ok(project_out(&view))
    }

    /// `ProjectService.add_repository`.
    pub fn project_add_repository(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        repository: &str,
        role: &str,
    ) -> Result<Value, WgError> {
        let scope = self.workspace_or_default(workspace)?;
        let view = self
            .runtime
            .decide(
                context,
                |session| session.registry_world(Some(scope)),
                |slice, stamp| {
                    projects::add_repository(slice, stamp, scope, reference, repository, role)
                },
            )?
            .reply;
        Ok(project_out(&view))
    }

    /// `ProjectService.remove_repository`.
    pub fn project_remove_repository(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        repository: &str,
    ) -> Result<Value, WgError> {
        let scope = self.workspace_or_default(workspace)?;
        let view = self
            .runtime
            .decide(
                context,
                |session| session.registry_world(Some(scope)),
                |slice, stamp| {
                    projects::remove_repository(slice, stamp, scope, reference, repository)
                },
            )?
            .reply;
        Ok(project_out(&view))
    }

    /// `rename.rename_term`: renames a word across a project's repositories, verification
    /// configs and tasks, each change its own transaction. With `dry_run` nothing is written.
    pub fn project_rename_term(
        &mut self,
        context: &RequestContext,
        workspace: Option<&str>,
        reference: &str,
        term: &Term,
        dry_run: bool,
    ) -> Result<Value, WgError> {
        self.require(context, Permission::WorkspaceRead)?;
        let scope = self.workspace_or_default(workspace)?;
        let (inputs, configs) = self.runtime.query(
            |session| {
                let slice = session.registry_world(Some(scope))?;
                let view = projects::view(&slice, scope, reference)?;
                let configs = session.verification_configs(scope)?;
                let world = session.task_world(Some(scope), &[])?;
                Ok((rename_inputs(&view, &slice, &configs, &world), configs))
            },
            |(inputs, configs)| Ok((inputs.clone(), configs.clone())),
        )?;
        let plan = plan_rename(
            term,
            &inputs.project,
            &inputs.repositories,
            &inputs.configs,
            &inputs.tasks,
        );
        let exists = |path: &str| Path::new(path).exists();
        let missing: Vec<Vec<String>> = plan
            .verification_configs
            .iter()
            .map(|config| missing_paths(&config.path_rewrites, &exists))
            .collect();
        let errors = if dry_run {
            Vec::new()
        } else {
            self.apply_rename(context, scope, term, &plan, &configs)
        };
        Ok(rename_report(term, dry_run, &plan, &missing, &errors))
    }

    /// Applies a rename plan in Python's order (repositories, project, verification configs,
    /// tasks); an update that fails is reported and the rest carry on.
    fn apply_rename(
        &mut self,
        context: &RequestContext,
        scope: Id,
        term: &Term,
        plan: &RenamePlan,
        configs: &[VerificationConfig],
    ) -> Vec<Failure> {
        let repositories = self.rename_repositories(context, scope, plan);
        let project = self.rename_project(context, scope, plan);
        let verification = self.rename_configs(context, scope, plan, configs);
        let tasks = self.rename_tasks(context, scope, term, plan);
        [repositories, project, verification, tasks].concat()
    }

    fn rename_repositories(
        &mut self,
        context: &RequestContext,
        scope: Id,
        plan: &RenamePlan,
    ) -> Vec<Failure> {
        plan.repositories
            .iter()
            .filter_map(|change| {
                self.runtime
                    .decide(
                        context,
                        |session| session.registry_world(Some(scope)),
                        |slice, stamp| {
                            renames::rename_repository(
                                slice,
                                stamp,
                                scope,
                                change.id,
                                change.name.as_deref(),
                                change.description.as_deref(),
                            )
                        },
                    )
                    .err()
                    .map(|error| (format!("repository {}", change.old_name), error.message))
            })
            .collect()
    }

    fn rename_project(
        &mut self,
        context: &RequestContext,
        scope: Id,
        plan: &RenamePlan,
    ) -> Vec<Failure> {
        plan.project
            .iter()
            .filter_map(|change| {
                let body = project_update_body(change);
                self.project_update(
                    context,
                    Some(&scope.to_string()),
                    &change.id.to_string(),
                    &body,
                )
                .err()
                .map(|error| (format!("project {}", change.old_slug), error.message))
            })
            .collect()
    }

    fn rename_configs(
        &mut self,
        context: &RequestContext,
        scope: Id,
        plan: &RenamePlan,
        configs: &[VerificationConfig],
    ) -> Vec<Failure> {
        plan.verification_configs
            .iter()
            .filter_map(|change| {
                let config = configs.iter().find(|config| {
                    config.name == change.name
                        && config.scope_type == change.scope_type
                        && config.scope_id == change.scope_id
                })?;
                let checks = checks_of(&change.checks);
                self.runtime
                    .decide(
                        context,
                        |session| session.registry_world(Some(scope)),
                        |slice, stamp| renames::resave_config(slice, stamp, config, &checks),
                    )
                    .err()
                    .map(|error| {
                        (
                            format!("verification config {}", change.name),
                            error.message,
                        )
                    })
            })
            .collect()
    }

    fn rename_tasks(
        &mut self,
        context: &RequestContext,
        scope: Id,
        term: &Term,
        plan: &RenamePlan,
    ) -> Vec<Failure> {
        plan.tasks
            .iter()
            .filter_map(|change| {
                let body = task_update_body(change, term);
                self.task_update_plain(
                    context,
                    Some(&scope.to_string()),
                    &change.id.to_string(),
                    &body,
                )
                .err()
                .map(|error| (change.key.clone(), error.message))
            })
            .collect()
    }
}
