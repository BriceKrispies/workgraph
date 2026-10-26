//! The pure half of `init` and `doctor` (`cli/commands/core.py`, IF-02): the first workspace as a
//! decision, the result `init` prints, and the health checks `doctor` runs.
//!
//! The shell gathers the facts (directories, the database, the process table) and calls these;
//! what a fact means, and the text and order of the checks, is decided here.

use serde_json::{Map, Value as Json, json};
use wg_json::Value;
use wg_model::entity::Workspace;
use wg_model::{Decision, Stamp};
use wg_registry::providers::ProviderHealth;
use wg_types::{Permission, WgError};
use wg_wire::convention::{dict, optional_text, text};

use crate::spec::registry::validate_workspace_create;

/// What `workgraph init` was asked for.
#[derive(Clone, Copy, Debug)]
pub struct InitRequest<'a> {
    pub workspace: &'a str,
    pub name: Option<&'a str>,
    pub prefix: &'a str,
    pub no_workspace: bool,
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
    let slug = dump
        .get("slug")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_owned();
    if existing.iter().any(|workspace| workspace.slug == slug) {
        return Ok(Decision::new(None));
    }
    Ok(wg_registry::workspaces::insert(stamp, dump).map(|row| Some(row.slug)))
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
    /// `LLMRegistry.health`: each configured LLM provider, in configuration order.
    pub llm: Vec<(String, ProviderHealth)>,
    /// `AgentRegistry.health`: each configured agent adapter, in configuration order.
    pub agents: Vec<(String, ProviderHealth)>,
    pub execution_enabled: bool,
    pub live_workers: usize,
    pub dead_jobs: usize,
    pub spooled_events: usize,
    /// The `events verify` result of each active workspace, when `--verify-events` was given.
    pub event_projections: Option<Vec<EventProjection>>,
}

/// `events verify` for one workspace: the counts and the keys of the tasks whose rows differ from
/// their replayed events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventProjection {
    pub slug: String,
    /// The tasks in the workspace's tables.
    pub tasks: usize,
    /// The task events replayed.
    pub events: usize,
    /// The mismatched task keys, in table order.
    pub mismatches: Vec<String>,
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

/// The checks of `doctor`, in Python's order: the storage and configuration checks, the tree-sitter
/// grammars, one row per LLM provider and per agent adapter, the agent execution switch, the
/// worker and spool checks, then the event projections. The checks that need Python itself and
/// the web build are not part of the list.
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
    let base = vec![
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
    ];
    let tail = vec![
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
    ];
    let llm = facts.llm.iter().map(|(name, health)| {
        check(
            &format!("llm provider {name}"),
            health.available,
            health.detail.clone(),
            "warning",
        )
    });
    let agents = facts.agents.iter().map(|(name, health)| {
        check(
            &format!("agent adapter {name}"),
            health.available,
            health.detail.clone(),
            "warning",
        )
    });
    base.into_iter()
        .chain(llm)
        .chain(agents)
        .chain(tail)
        .chain(
            facts
                .event_projections
                .iter()
                .flatten()
                .map(projection_check),
        )
        .collect()
}

/// `check("event projection <slug>", consistent, "<tasks> tasks / <events> events", ...)`, with
/// the first five mismatched keys listed as Python's list repr when there are any.
fn projection_check(projection: &EventProjection) -> Check {
    let counts = format!("{} tasks / {} events", projection.tasks, projection.events);
    let consistent = projection.mismatches.is_empty();
    let detail = if consistent {
        counts
    } else {
        let keys: Vec<String> = projection
            .mismatches
            .iter()
            .take(5)
            .map(|key| format!("'{key}'"))
            .collect();
        format!("{counts}; mismatches: [{}]", keys.join(", "))
    };
    check(
        &format!("event projection {}", projection.slug),
        consistent,
        detail,
        "error",
    )
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
