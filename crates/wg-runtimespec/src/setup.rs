//! The pure half of `init` and `doctor` (`cli/commands/core.py`, IF-02): the first workspace as a
//! decision, the result `init` prints, and the health checks `doctor` runs.
//!
//! The shell gathers the facts (directories, the database, the process table) and calls these;
//! what a fact means, and the text and order of the checks, is decided here.

use serde_json::{Map, Value as Json, json};
use wg_json::Value;
use wg_model::entity::Workspace;
use wg_model::event::WorkspaceCreated;
use wg_model::{Decision, EventDraft, EventPayload, Stamp, Write};
use wg_types::{Permission, WgError};
use wg_wire::convention::{dict, optional_text, text};

use crate::tasks::requests::{component, run};

/// What `workgraph init` was asked for.
#[derive(Clone, Copy, Debug)]
pub struct InitRequest<'a> {
    pub workspace: &'a str,
    pub name: Option<&'a str>,
    pub prefix: &'a str,
    pub no_workspace: bool,
}

/// `WorkspaceCreate`, validated: the dump in declaration order.
fn validate_workspace_create(body: &Json) -> Result<Map<String, Json>, WgError> {
    run(
        &component(
            "WorkspaceCreate",
            &["slug", "name", "description", "key_prefix"],
        )?,
        body,
    )
}

/// `str.title()`: the first letter of every run of letters upper case, the rest lower case.
#[must_use]
pub fn py_title(text: &str) -> String {
    text.chars()
        .fold((String::new(), false), |(out, in_word), c| {
            let word = c.is_alphabetic();
            let next = if !word {
                c.to_string()
            } else if in_word {
                c.to_lowercase().collect()
            } else {
                c.to_uppercase().collect()
            };
            (format!("{out}{next}"), word)
        })
        .0
}

/// The validated `WorkspaceCreate` of `init`: the slug, the name (`--name`, else the title-cased
/// slug) and the key prefix.
pub fn init_workspace_request(request: &InitRequest<'_>) -> Result<Map<String, Json>, WgError> {
    let name = request
        .name
        .filter(|name| !name.is_empty())
        .map_or_else(|| py_title(request.workspace), str::to_owned);
    validate_workspace_create(&json!({
        "slug": request.workspace,
        "name": name,
        "key_prefix": request.prefix,
    }))
}

/// `init`'s first workspace: created unless a workspace with that slug exists (archived ones
/// count). The reply is the slug of the workspace created.
pub fn init_workspace(
    existing: &[Workspace],
    stamp: &Stamp,
    dump: &Map<String, Json>,
) -> Result<Decision<Option<String>>, WgError> {
    stamp.require(&[Permission::WorkspaceAdmin])?;
    let field = |key: &str| {
        dump.get(key)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let slug = field("slug");
    if existing.iter().any(|workspace| workspace.slug == slug) {
        return Ok(Decision::new(None));
    }
    let ([workspace_id, event_id], _) = stamp.ids().take::<2>();
    let row = Workspace {
        id: workspace_id,
        slug: slug.clone(),
        name: field("name"),
        description: field("description"),
        key_prefix: field("key_prefix"),
        next_task_number: 1,
        settings: Default::default(),
        archived_at: None,
        created_at: stamp.at,
        updated_at: stamp.at,
    };
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(workspace_id),
        workspace_id,
        EventPayload::WorkspaceCreated(WorkspaceCreated {
            slug: row.slug.clone(),
            name: row.name.clone(),
            key_prefix: row.key_prefix.clone(),
        }),
    );
    Ok(Decision::new(Some(slug))
        .write(Write::insert(row))
        .event(event))
}

/// Where `init` put things.
#[derive(Clone, Copy, Debug)]
pub struct InitFacts<'a> {
    pub config_dir: &'a str,
    pub data_dir: &'a str,
    pub database: &'a str,
    pub schema_revision: Option<&'a str>,
    pub workspace_created: Option<&'a str>,
    pub default_workspace: Option<&'a str>,
}

/// The result of `init`.
#[must_use]
pub fn init_result(facts: &InitFacts<'_>) -> Value {
    dict(vec![
        ("config_dir", text(facts.config_dir)),
        ("data_dir", text(facts.data_dir)),
        ("database", text(facts.database)),
        ("schema_revision", optional_text(facts.schema_revision)),
        ("workspace_created", optional_text(facts.workspace_created)),
        ("default_workspace", optional_text(facts.default_workspace)),
    ])
}

/// What `doctor` observed. The shell fills it; [`doctor_checks`] turns it into the checks.
#[derive(Clone, Debug, Default)]
pub struct DoctorFacts {
    /// The output of `git --version`, or `None` when git is not on the path.
    pub git: Option<String>,
    pub current_revision: Option<String>,
    pub head_revision: Option<String>,
    pub integrity: String,
    pub journal_mode: String,
    /// The problems `config validate` reports.
    pub config_problems: Vec<String>,
    pub config_file: String,
    pub api_token_exists: bool,
    pub api_token_file: String,
    /// The tree-sitter grammars that load.
    pub languages: Vec<String>,
    pub execution_enabled: bool,
    pub live_workers: usize,
    pub dead_jobs: usize,
    pub spooled_events: usize,
}

/// One health check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    /// What a failure counts as: `error` or `warning`.
    pub level: &'static str,
    pub detail: String,
}

fn check(name: &str, ok: bool, detail: impl Into<String>, level: &'static str) -> Check {
    Check {
        name: name.to_owned(),
        ok,
        level,
        detail: detail.into(),
    }
}

fn shown(revision: Option<&String>) -> String {
    revision.map_or_else(|| "None".to_owned(), Clone::clone)
}

/// The checks of `doctor`, in Python's order. The checks that need Python itself, the LLM
/// providers, the agent adapters and the web build are not part of the list.
#[must_use]
pub fn doctor_checks(facts: &DoctorFacts) -> Vec<Check> {
    let schema = format!(
        "revision {} (head {})",
        shown(facts.current_revision.as_ref()),
        shown(facts.head_revision.as_ref())
    );
    let configuration = if facts.config_problems.is_empty() {
        facts.config_file.clone()
    } else {
        facts.config_problems.join("; ")
    };
    vec![
        check("python", true, "not required (native binary)", "error"),
        match &facts.git {
            Some(version) => check("git", true, version.clone(), "error"),
            None => check("git", false, "git not found on PATH", "error"),
        },
        check(
            "schema",
            facts.current_revision == facts.head_revision,
            schema,
            "error",
        ),
        check(
            "database integrity",
            facts.integrity == "ok",
            facts.integrity.clone(),
            "error",
        ),
        check(
            "journal mode",
            facts.journal_mode == "wal",
            facts.journal_mode.clone(),
            "warning",
        ),
        check(
            "configuration",
            facts.config_problems.is_empty(),
            configuration,
            "error",
        ),
        check(
            "api token",
            facts.api_token_exists,
            facts.api_token_file.clone(),
            "warning",
        ),
        check(
            "tree-sitter grammars",
            !facts.languages.is_empty(),
            facts.languages.join(", "),
            "warning",
        ),
        check(
            "agent execution",
            true,
            if facts.execution_enabled {
                "enabled"
            } else {
                "disabled (opt in with `workgraph config set execution.enabled true`)"
            },
            "error",
        ),
        check(
            "workers",
            facts.live_workers > 0,
            format!("{} running", facts.live_workers),
            "warning",
        ),
        check(
            "dead-letter jobs",
            facts.dead_jobs == 0,
            format!("{} dead jobs", facts.dead_jobs),
            "warning",
        ),
        check(
            "hook spool",
            facts.spooled_events == 0,
            format!(
                "{} spooled hook events awaiting replay",
                facts.spooled_events
            ),
            "warning",
        ),
    ]
}

/// `all(c["ok"] or c["level"] != "error")`.
#[must_use]
pub fn healthy(checks: &[Check]) -> bool {
    checks.iter().all(|c| c.ok || c.level != "error")
}

/// The result of `doctor`: `{"healthy": ..., "checks": [...]}`.
#[must_use]
pub fn doctor_result(checks: &[Check]) -> Value {
    dict(vec![
        ("healthy", Value::Bool(healthy(checks))),
        (
            "checks",
            Value::List(
                checks
                    .iter()
                    .map(|c| {
                        dict(vec![
                            ("check", text(&c.name)),
                            ("ok", Value::Bool(c.ok)),
                            ("level", text(if c.ok { "ok" } else { c.level })),
                            ("detail", text(&c.detail)),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

/// `{"rewritten": [...]}` of `config migrate`.
#[must_use]
pub fn migrate_result(rewritten: &[String]) -> Value {
    dict(vec![(
        "rewritten",
        Value::List(rewritten.iter().map(|p| text(p)).collect()),
    )])
}

/// `ConfigStore.workspace_file`: `<config>/workspaces/<slug>.toml`.
#[must_use]
pub fn workspace_file(paths: &wg_config::paths::AppPaths, slug: &str) -> String {
    std::path::Path::new(paths.workspace_config_dir().as_str())
        .join(format!("{slug}.toml"))
        .display()
        .to_string()
}

/// The paths of `config path`.
#[must_use]
pub fn paths_result(paths: &[(&'static str, String)]) -> Value {
    dict(paths.iter().map(|(key, path)| (*key, text(path))).collect())
}

/// The result of `config show` as JSON: the workspace, the configuration and the files.
#[must_use]
pub fn show_json(
    slug: Option<&str>,
    config: &Map<String, Json>,
    global_file: &str,
    workspace_file: Option<&str>,
) -> Json {
    json!({
        "workspace": slug,
        "config": config,
        "files": {"global": global_file, "workspace": workspace_file},
    })
}
