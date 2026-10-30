//! The pure half of the `workspace` and `project` commands (`cli/commands/core.py`): the clap
//! commands and the human views of the results.

use wg_json::Value;
use wg_render::Console;

use wg_iface::cli::Snapshot;
use wg_iface::cli::args::{get, items, opt, python, s};
use wg_iface::cli::tree::{SNAPSHOT_JSON, placeholder};

fn snapshot_command(name: &str) -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, name),
        Err(_) => wg_iface::cli::tree::command(name),
    }
}

/// The clap command of the `workspace` group.
#[must_use]
pub fn workspace_command() -> clap::Command {
    snapshot_command("workspace")
}

/// The clap command of the `project` group.
#[must_use]
pub fn project_command() -> clap::Command {
    snapshot_command("project")
}

/// `when()`: `YYYY-MM-DD HH:MM`, or nothing.
fn when(value: Option<&Value>) -> String {
    match value {
        Some(Value::DateTime(at)) => at.format_display(),
        _ => String::new(),
    }
}

/// The value of a `kv` line: nothing for an absent, null, empty list or empty dict value.
fn shown(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::List(members)) if members.is_empty() => None,
        Some(Value::Dict(entries)) if entries.is_empty() => None,
        Some(other) => Some(python(other)),
    }
}

fn kv(console: &Console, pairs: &[(&str, Option<String>)], title: Option<&str>) -> String {
    let borrowed: Vec<(&str, Option<&str>)> = pairs
        .iter()
        .map(|(key, value)| (*key, value.as_deref()))
        .collect();
    console.kv(&borrowed, title).text
}

/// `workspace create`.
#[must_use]
pub fn workspace_created_line(console: &Console, workspace: &Value) -> String {
    console
        .print(&format!(
            "created workspace [bold]{}[/bold] ({})",
            s(get(workspace, "slug")),
            s(get(workspace, "id"))
        ))
        .text
}

/// `workspace list`: the default workspace is marked with `*`.
#[must_use]
pub fn workspace_list_view(console: &Console, list: &Value, default: Option<&str>) -> String {
    let rows: Vec<Vec<String>> = match list {
        Value::List(rows) => rows
            .iter()
            .map(|row| {
                let slug = s(get(row, "slug"));
                vec![
                    if Some(slug.as_str()) == default {
                        "*"
                    } else {
                        ""
                    }
                    .to_owned(),
                    slug,
                    s(get(row, "name")),
                    s(get(row, "key_prefix")),
                    when(get(row, "created_at")),
                ]
            })
            .collect(),
        _ => Vec::new(),
    };
    console
        .table(&rows, &["", "slug", "name", "prefix", "created"], None)
        .text
}

/// `workspace show`.
#[must_use]
pub fn workspace_show_view(console: &Console, summary: &Value) -> String {
    let workspace = get(summary, "workspace");
    let count = |key: &str| opt(get(summary, key));
    kv(
        console,
        &[
            ("id", workspace.and_then(|w| opt(get(w, "id")))),
            ("slug", workspace.and_then(|w| opt(get(w, "slug")))),
            ("name", workspace.and_then(|w| opt(get(w, "name")))),
            ("projects", count("projects")),
            ("repositories", count("repositories")),
            ("ready tasks", count("ready_tasks")),
            ("running attempts", count("running_attempts")),
            ("queued jobs", count("queued_jobs")),
            ("dead jobs", count("dead_jobs")),
            ("tasks by status", shown(get(summary, "tasks_by_status"))),
        ],
        Some("workspace"),
    )
}

/// `workspace use`.
#[must_use]
pub fn workspace_use_line(console: &Console, workspace: &Value) -> String {
    console
        .print(&format!(
            "default workspace is now [bold]{}[/bold]",
            s(get(workspace, "slug"))
        ))
        .text
}

/// `workspace archive`.
#[must_use]
pub fn workspace_archived_line(console: &Console, workspace: &Value) -> String {
    console
        .print(&format!("archived {}", s(get(workspace, "slug"))))
        .text
}

/// `workspace set`: the settings as `key: value` lines.
#[must_use]
pub fn workspace_settings_view(console: &Console, workspace: &Value) -> String {
    let pairs: Vec<(String, Option<String>)> = match get(workspace, "settings") {
        Some(Value::Dict(entries)) => entries
            .iter()
            .map(|(key, value)| (python(&Value::Str(key.clone())), shown(Some(value))))
            .collect(),
        _ => Vec::new(),
    };
    let borrowed: Vec<(&str, Option<String>)> = pairs
        .iter()
        .map(|(key, value)| (key.as_str(), value.clone()))
        .collect();
    kv(console, &borrowed, None)
}

/// `project create`.
#[must_use]
pub fn project_created_line(console: &Console, project: &Value) -> String {
    console
        .print(&format!(
            "created project [bold]{}[/bold] with {} repositories",
            s(get(project, "slug")),
            items(get(project, "repository_ids")).len()
        ))
        .text
}

/// `project list`.
#[must_use]
pub fn project_list_view(console: &Console, list: &Value) -> String {
    let rows: Vec<Vec<String>> = match list {
        Value::List(rows) => rows
            .iter()
            .map(|row| {
                vec![
                    s(get(row, "slug")),
                    s(get(row, "name")),
                    s(get(row, "status")),
                    items(get(row, "repository_ids")).len().to_string(),
                ]
            })
            .collect(),
        _ => Vec::new(),
    };
    console
        .table(&rows, &["slug", "name", "status", "repos"], None)
        .text
}

/// `project show`.
#[must_use]
pub fn project_show_view(console: &Console, summary: &Value) -> String {
    let project = get(summary, "project");
    let names: Vec<String> = items(get(summary, "repositories"))
        .iter()
        .map(|repo| s(get(repo, "name")))
        .collect();
    let slug = project.map(|p| s(get(p, "slug")));
    kv(
        console,
        &[
            ("id", project.and_then(|p| opt(get(p, "id")))),
            ("name", project.and_then(|p| opt(get(p, "name")))),
            ("status", project.and_then(|p| opt(get(p, "status")))),
            ("repositories", Some(names.join(", "))),
            ("open tasks", opt(get(summary, "open_tasks"))),
            ("accepted", opt(get(summary, "accepted_tasks"))),
            ("by status", shown(get(summary, "tasks_by_status"))),
        ],
        slug.as_deref(),
    )
}

/// `project update`.
#[must_use]
pub fn project_updated_line(console: &Console, project: &Value) -> String {
    console
        .print(&format!("updated {}", s(get(project, "slug"))))
        .text
}

/// `project add-repo` and `remove-repo`.
#[must_use]
pub fn project_spans_line(console: &Console, project: &Value) -> String {
    console
        .print(&format!(
            "{} now spans {} repositories",
            s(get(project, "slug")),
            items(get(project, "repository_ids")).len()
        ))
        .text
}

fn number(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Int(n)) => i64::try_from(*n).unwrap_or(0),
        _ => 0,
    }
}

/// `_render_rename`.
#[must_use]
pub fn rename_view(console: &Console, report: &Value) -> String {
    let dry_run = matches!(get(report, "dry_run"), Some(Value::Bool(true)));
    let verb = if dry_run { "would change" } else { "changed" };
    let lines = |markup: Vec<String>| -> String {
        markup.iter().map(|line| console.print(line).text).collect()
    };
    let project = match get(report, "project") {
        Some(Value::Null) | None => Vec::new(),
        Some(project) => vec![format!(
            "project: {} -> {} ({} -> {})",
            s(get(project, "slug")),
            s(get(project, "new_slug")),
            s(get(project, "name")),
            s(get(project, "new_name"))
        )],
    };
    let repositories: Vec<String> = items(get(report, "repositories"))
        .iter()
        .map(|repo| {
            format!(
                "repository: {} -> {} (checkout paths unchanged)",
                s(get(repo, "name")),
                s(get(repo, "new_name"))
            )
        })
        .collect();
    let configs: Vec<String> = items(get(report, "verification_configs"))
        .iter()
        .flat_map(|config| {
            std::iter::once(format!(
                "verification config {}: {} site(s)",
                s(get(config, "name")),
                s(get(config, "sites"))
            ))
            .chain(items(get(config, "missing_paths")).iter().map(|path| {
                format!(
                    "  [yellow]names a path that does not exist:[/yellow] {}",
                    python(path)
                )
            }))
        })
        .collect();
    let tasks = items(get(report, "tasks"));
    let open: Vec<&Value> = tasks
        .iter()
        .filter(|task| !matches!(get(task, "closed"), Some(Value::Bool(true))))
        .collect();
    let mut sites: Vec<(String, i64)> = Vec::new();
    for task in &open {
        if let Some(Value::Dict(fields)) = get(task, "fields") {
            for (name, count) in fields
                .iter()
                .filter(|(name, _)| python(&Value::Str((*name).clone())) != "tags")
            {
                let name = python(&Value::Str(name.clone()));
                match sites.iter().position(|(known, _)| *known == name) {
                    Some(at) => sites[at].1 += number(Some(count)),
                    None => sites.push((name, number(Some(count)))),
                }
            }
        }
    }
    sites.sort();
    let detail = if sites.is_empty() {
        String::new()
    } else {
        let listed: Vec<String> = sites
            .iter()
            .map(|(name, n)| format!("{n} {name}"))
            .collect();
        format!(" ({} sites)", listed.join(", "))
    };
    let tagged = tasks
        .iter()
        .filter(|task| match get(task, "fields") {
            Some(Value::Dict(fields)) => fields
                .iter()
                .any(|(name, _)| python(&Value::Str((*name).clone())) == "tags"),
            _ => false,
        })
        .count();
    let mut out = lines(project);
    out.push_str(&lines(repositories));
    out.push_str(&lines(configs));
    out.push_str(&lines(vec![
        format!("open tasks: {verb} {}{detail}", open.len()),
        format!(
            "tags: {verb} {} on {tagged} task(s)",
            s(get(report, "tags_renamed"))
        ),
        format!(
            "closed tasks whose text still says {}, kept as records: {}",
            s(get(report, "from")),
            s(get(report, "closed_kept"))
        ),
    ]));
    let errors: Vec<String> = items(get(report, "errors"))
        .iter()
        .map(|error| {
            format!(
                "[red]error[/red] {}: {}",
                s(get(error, "target")),
                s(get(error, "error"))
            )
        })
        .collect();
    out.push_str(&lines(errors));
    if dry_run {
        out.push_str(&lines(vec!["(dry run: nothing changed)".to_owned()]));
    }
    out
}

/// An option's value as given, empty text included: where Python passes the option on without
/// an `or` default, `--description ""` is a value, not an absent option.
#[must_use]
pub fn option_raw(m: &clap::ArgMatches, id: &str) -> Option<String> {
    m.try_get_one::<String>(id).ok().flatten().cloned()
}

/// Whether the rename reported errors (it then exits 1).
#[must_use]
pub fn rename_failed(report: &Value) -> bool {
    !items(get(report, "errors")).is_empty()
}
