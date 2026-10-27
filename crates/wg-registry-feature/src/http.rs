//! The workspace and settings routes of the registry feature: `GET /api/v1/workspaces` (H003),
//! `POST /api/v1/workspaces` (H004) and `GET /api/v1/workspaces/{ws}` (H005) from
//! `api/routes/core.py`, `GET /api/v1/settings` (H117) and `PATCH /api/v1/settings` (H118) from
//! `api/routes/ops.py`, and the provider routes `GET /api/v1/agents` (H119) and
//! `GET /api/v1/llm/health` (H120). The health and meta routes stay in the server's base group.
//! The project routes (H006 to H011) and the repository routes (H012 to H021) of `core.py` are
//! here too; the worktree listing (H022) is the git feature's.
//!
//! [`SPECS`], [`PROVIDER_SPECS`], [`PROJECT_SPECS`] and [`REPOSITORY_SPECS`] are the tables of
//! operations; the server binds each one to its group's `run` function, which reads the validated
//! request and calls the feature's [`crate::service`] entry point, the rules of
//! [`crate::spec::settings`] and [`crate::service::setup`] on the application, or the git feature's
//! services for the git operations of a repository. The workspace routes take no permission check
//! of their own: the service makes it, as the Python service does.

use serde_json::Value as Json;
use wg_json::Value;
use wg_runtime::{App, RouteGroup, RouteRequest};
use wg_runtimespec::text_param;
use wg_serverspec::groups::{BodyRef, RouteSpec};
use wg_types::{ErrorCode, Permission, WgError};
use wg_wire::convention::json_object;

use crate::service;
use crate::service::setup::{agents_document, llm_health_document};
use crate::spec::dto::settings as settings_dto;
use crate::spec::query::{body_text, flag, no_handler, number};
use crate::spec::repos;
use crate::spec::settings::{patch_values, patched_config, settings_workspace};

/// The operations of the group, in registration order.
pub const SPECS: [RouteSpec; 5] = [
    RouteSpec {
        id: "H003",
        method: "GET",
        path: "/api/v1/workspaces",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H004",
        method: "POST",
        path: "/api/v1/workspaces",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: "WorkspaceCreate",
            required: true,
            strip: true,
        }),
    },
    RouteSpec {
        id: "H005",
        method: "GET",
        path: "/api/v1/workspaces/{ws}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H117",
        method: "GET",
        path: "/api/v1/settings",
        status: 200,
        query: &["workspace"],
        body: None,
    },
    RouteSpec {
        id: "H118",
        method: "PATCH",
        path: "/api/v1/settings",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "SettingsPatch",
            required: true,
            strip: false,
        }),
    },
];

/// The group as the feature descriptor takes it.
pub const GROUP: RouteGroup = RouteGroup { specs: &SPECS, run };

/// The provider routes of the registry feature (`api/routes/ops.py`): `GET /api/v1/agents`
/// (H119) and `GET /api/v1/llm/health` (H120).
pub const PROVIDER_SPECS: [RouteSpec; 2] = [
    RouteSpec {
        id: "H119",
        method: "GET",
        path: "/api/v1/agents",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H120",
        method: "GET",
        path: "/api/v1/llm/health",
        status: 200,
        query: &[],
        body: None,
    },
];

/// The provider group as the feature descriptor takes it.
pub const PROVIDER_GROUP: RouteGroup = RouteGroup {
    specs: &PROVIDER_SPECS,
    run: run_providers,
};

/// The project routes (`api/routes/core.py`): the list and creation of projects (H006, H007), one
/// project (H008, H009) and its repository links (H010, H011), in registration order.
pub const PROJECT_SPECS: [RouteSpec; 6] = [
    RouteSpec {
        id: "H006",
        method: "GET",
        path: "/api/v1/ws/{ws}/projects",
        status: 200,
        query: &["include_archived"],
        body: None,
    },
    RouteSpec {
        id: "H007",
        method: "POST",
        path: "/api/v1/ws/{ws}/projects",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: "ProjectCreate",
            required: true,
            strip: true,
        }),
    },
    RouteSpec {
        id: "H008",
        method: "GET",
        path: "/api/v1/ws/{ws}/projects/{ref}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H009",
        method: "PATCH",
        path: "/api/v1/ws/{ws}/projects/{ref}",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "ProjectUpdate",
            required: true,
            strip: true,
        }),
    },
    RouteSpec {
        id: "H010",
        method: "POST",
        path: "/api/v1/ws/{ws}/projects/{ref}/repositories",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "RepoLink",
            required: true,
            strip: false,
        }),
    },
    RouteSpec {
        id: "H011",
        method: "DELETE",
        path: "/api/v1/ws/{ws}/projects/{ref}/repositories/{repository}",
        status: 200,
        query: &[],
        body: None,
    },
];

/// The project group as the feature descriptor takes it.
pub const PROJECT_GROUP: RouteGroup = RouteGroup {
    specs: &PROJECT_SPECS,
    run: run_projects,
};

/// The repository routes (`api/routes/core.py`): the list and registration of repositories (H012,
/// H013), one repository (H014 to H016), its refresh and status (H017, H018), its sync and commits
/// (H019 to H021). H022, the worktree listing, is the git feature's.
pub const REPOSITORY_SPECS: [RouteSpec; 10] = [
    RouteSpec {
        id: "H012",
        method: "GET",
        path: "/api/v1/ws/{ws}/repositories",
        status: 200,
        query: &["include_archived", "project"],
        body: None,
    },
    RouteSpec {
        id: "H013",
        method: "POST",
        path: "/api/v1/ws/{ws}/repositories",
        status: 201,
        query: &[],
        body: Some(BodyRef {
            component: "RepositoryRegister",
            required: true,
            strip: true,
        }),
    },
    RouteSpec {
        id: "H014",
        method: "GET",
        path: "/api/v1/ws/{ws}/repositories/{ref}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H015",
        method: "PATCH",
        path: "/api/v1/ws/{ws}/repositories/{ref}",
        status: 200,
        query: &[],
        body: Some(BodyRef {
            component: "RepositoryUpdate",
            required: true,
            strip: true,
        }),
    },
    RouteSpec {
        id: "H016",
        method: "DELETE",
        path: "/api/v1/ws/{ws}/repositories/{ref}",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H017",
        method: "POST",
        path: "/api/v1/ws/{ws}/repositories/{ref}/refresh",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H018",
        method: "GET",
        path: "/api/v1/ws/{ws}/repositories/{ref}/status",
        status: 200,
        query: &[],
        body: None,
    },
    RouteSpec {
        id: "H019",
        method: "POST",
        path: "/api/v1/ws/{ws}/repositories/{ref}/sync",
        status: 200,
        query: &["mode"],
        body: None,
    },
    RouteSpec {
        id: "H020",
        method: "GET",
        path: "/api/v1/ws/{ws}/repositories/{ref}/commits",
        status: 200,
        query: &["limit", "offset", "linked_only", "include_unreachable"],
        body: None,
    },
    RouteSpec {
        id: "H021",
        method: "GET",
        path: "/api/v1/ws/{ws}/repositories/{ref}/commits/{sha}",
        status: 200,
        query: &[],
        body: None,
    },
];

/// The repository group as the feature descriptor takes it.
pub const REPOSITORY_GROUP: RouteGroup = RouteGroup {
    specs: &REPOSITORY_SPECS,
    run: run_repositories,
};

/// The project operations: each makes its permission check in the service, as the Python service
/// does.
fn run_projects(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let params = &request.params;
    let ws = Some(text_param(params, "ws"));
    let context = &request.context;
    let reference = text_param(params, "ref");
    match id {
        "H006" => service::project_list(app, context, ws, flag(params, "include_archived")),
        "H007" => service::project_create(app, context, ws, &request.body),
        "H008" => service::project_summary(app, context, ws, reference),
        "H009" => service::project_update(app, context, ws, reference, &request.given),
        "H010" => {
            let repository = body_text(&request.body, "repository");
            let role = body_text(&request.body, "role");
            let role = if role.is_empty() { "member" } else { role };
            service::project_add_repository(app, context, ws, reference, repository, role)
        }
        "H011" => service::project_remove_repository(
            app,
            context,
            ws,
            reference,
            text_param(params, "repository"),
        ),
        other => Err(no_handler(other)),
    }
}

/// The repository operations: each makes its permission check in the service, and the git reads
/// and writes run through the git feature's services.
fn run_repositories(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    let params = &request.params;
    let ws = Some(text_param(params, "ws"));
    let context = &request.context;
    let reference = text_param(params, "ref");
    match id {
        "H012" => service::repo_list(
            app,
            context,
            ws,
            flag(params, "include_archived"),
            params.get("project").and_then(Json::as_str),
        )
        .map(Value::List),
        "H013" => service::repo_register(app, context, ws, &repos::register_input(&request.body)),
        "H014" => service::repo_get(app, context, ws, reference),
        "H015" => service::repo_update(
            app,
            context,
            ws,
            reference,
            &repos::repository_patch(&request.body)?,
        ),
        "H016" => service::repo_remove(app, context, ws, reference),
        "H017" => service::repo_refresh(app, context, ws, reference),
        "H018" => service::repo_status(app, context, ws, reference),
        "H019" => {
            let full = repos::sync_is_full(text_param(params, "mode"))?;
            match wg_git_feature::service::git_sync(app, context, ws, Some(reference), full)? {
                Value::List(mut results) if !results.is_empty() => Ok(results.swap_remove(0)),
                other => Ok(other),
            }
        }
        "H020" => wg_git_feature::service::git_log(
            app,
            context,
            ws,
            reference,
            number(params, "limit", 50),
            flag(params, "linked_only"),
            number(params, "offset", 0),
            flag(params, "include_unreachable"),
        ),
        "H021" => wg_git_feature::service::git_show(
            app,
            context,
            ws,
            reference,
            text_param(params, "sha"),
        ),
        other => Err(no_handler(other)),
    }
}

/// The provider operations: both read the workspace permission, then the adapters or providers
/// of the configuration.
fn run_providers(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    match id {
        "H119" => {
            request.context.require(&[Permission::WorkspaceRead])?;
            agents_document(app)
        }
        "H120" => {
            request.context.require(&[Permission::WorkspaceRead])?;
            llm_health_document(app)
        }
        other => Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("route {other} has no handler"),
        )),
    }
}

/// One operation on the application context: the request in, the document out. The permission
/// checks come first, as the Python routes make them.
fn run(app: &mut App, id: &str, request: RouteRequest) -> Result<Value, WgError> {
    match id {
        "H003" => service::workspace_list(app, &request.context, false),
        "H004" => service::workspace_create(app, &request.context, &request.body, false),
        "H005" => service::workspace_summary(
            app,
            &request.context,
            Some(text_param(&request.params, "ws")),
        ),
        "H117" => {
            request.context.require(&[Permission::WorkspaceRead])?;
            let config = app.config()?;
            let slug = settings_workspace(&request.params, &config);
            let effective = app.effective_config(slug.as_deref())?;
            Ok(settings_dto::settings(
                json_object(&effective),
                app.paths.config_file().as_str(),
                app.paths.data_dir().as_str(),
            ))
        }
        "H118" => {
            request.context.require(&[Permission::ConfigManage])?;
            let current = app.config()?;
            let next = patched_config(&current, &patch_values(&request.body))?;
            app.save_config(&next)?;
            Ok(settings_dto::patched(json_object(&next)))
        }
        other => Err(WgError::new(
            ErrorCode::WorkgraphError,
            format!("route {other} has no handler"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_the_operations_of_its_group() {
        let ids: Vec<&str> = SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids, ["H003", "H004", "H005", "H117", "H118"]);
        for spec in &SPECS {
            let expected = match spec.id {
                "H117" | "H118" => "meta-settings",
                _ => "IF-42",
            };
            assert_eq!(
                wg_serverspec::operations::group_of(spec.id),
                expected,
                "{}",
                spec.id
            );
        }
    }

    #[test]
    fn the_workspace_create_body_strips_whitespace() {
        let create = SPECS.iter().find(|spec| spec.id == "H004");
        let body = create.and_then(|spec| spec.body.as_ref());
        assert!(body.is_some_and(|body| body.strip && body.required));
    }

    #[test]
    fn the_provider_table_has_the_agent_and_llm_health_routes() {
        let ids: Vec<&str> = PROVIDER_SPECS.iter().map(|spec| spec.id).collect();
        assert_eq!(ids, ["H119", "H120"]);
        for spec in &PROVIDER_SPECS {
            assert_eq!(wg_serverspec::operations::group_of(spec.id), "IF-42");
            assert_eq!(spec.method, "GET");
            assert!(spec.body.is_none() && spec.query.is_empty());
        }
    }

    #[test]
    fn the_settings_patch_body_loads_from_the_snapshot() {
        let openapi = wg_wire::contract::openapi().unwrap_or_default();
        for spec in &SPECS {
            if let Some(body) = &spec.body {
                assert!(
                    wg_serverspec::groups::body_spec(&openapi, body).is_some(),
                    "{}",
                    spec.id
                );
            }
        }
    }
}
