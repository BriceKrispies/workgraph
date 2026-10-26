//! The pure half of the `repo` commands (`cli/commands/repo.py`): the inputs the options make and
//! the human views of the results.

use crate::spec::repos::RegisterInput;
use clap::ArgMatches;
use wg_json::Value;
use wg_registry::admin::Patch;
use wg_render::Console;
use wg_types::AccessMode;

use wg_clispec::args::{flag, get, items, many, one, opt, python, s, when};
use wg_clispec::tree::{SNAPSHOT_JSON, placeholder};
use wg_clispec::{CliFailure, Snapshot};

/// The clap command of the `repo` group: the snapshot's own tree.
#[must_use]
pub fn command() -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "repo"),
        Err(_) => wg_clispec::tree::command("repo"),
    }
}

/// The `RepositoryRegister` of `repo register`.
#[must_use]
pub fn register_input(m: &ArgMatches) -> RegisterInput {
    RegisterInput {
        path: one(m, "path").unwrap_or_default(),
        name: one(m, "--name"),
        description: one(m, "--description").unwrap_or_default(),
        read_only: flag(m, "--read-only"),
        tracked_branches: many(m, "--branch"),
        projects: many(m, "--project"),
    }
}

/// The values of `repo list`: whether archived repositories are included, and the project.
#[must_use]
pub fn list_scope(m: &ArgMatches) -> (bool, Option<String>) {
    (flag(m, "--all"), one(m, "--project"))
}

/// The tracked branches `--track` and `--untrack` leave, given the current ones: new branches
/// are appended in order, then the untracked ones are dropped. `None` when neither is given.
#[must_use]
pub fn tracked_after(
    current: &[String],
    track: &[String],
    untrack: &[String],
) -> Option<Vec<String>> {
    if track.is_empty() && untrack.is_empty() {
        return None;
    }
    let grown = track.iter().fold(current.to_vec(), |list, branch| {
        if list.contains(branch) {
            list
        } else {
            list.into_iter().chain([branch.clone()]).collect()
        }
    });
    Some(
        grown
            .into_iter()
            .filter(|branch| !untrack.contains(branch))
            .collect(),
    )
}

/// The branches `--track` and `--untrack` name.
#[must_use]
pub fn track_options(m: &ArgMatches) -> (Vec<String>, Vec<String>) {
    (many(m, "--track"), many(m, "--untrack"))
}

/// The tracked branches of a `RepositoryOut`.
#[must_use]
pub fn tracked_of(repository: &Value) -> Vec<String> {
    items(get(repository, "tracked_branches"))
        .iter()
        .map(python)
        .collect()
}

/// The `RepositoryUpdate` of `repo update`; `tracked` is the list after `--track`/`--untrack`.
pub fn update_patch(m: &ArgMatches, tracked: Option<Vec<String>>) -> Result<Patch, CliFailure> {
    let access = match one(m, "--access").as_deref() {
        None => None,
        Some("read_only") => Some(AccessMode::ReadOnly),
        Some("read_write") => Some(AccessMode::ReadWrite),
        Some(other) => {
            return Err(CliFailure::fail(
                format!(
                    "Invalid value for '--access': {other:?} is not one of 'read_only', 'read_write'."
                ),
                2,
            ));
        }
    };
    Ok(Patch {
        name: None,
        description: one(m, "--description"),
        access_mode: access,
        default_branch: one(m, "--default-branch"),
        tracked_branches: tracked,
    })
}

/// The values of `repo depend`: the dependency, its description and whether to remove it.
#[must_use]
pub fn depend_args(m: &ArgMatches) -> (String, String, bool) {
    (
        one(m, "depends_on").unwrap_or_default(),
        one(m, "--description").unwrap_or_default(),
        flag(m, "--remove"),
    )
}

/// The first `n` characters (`short()`), or nothing for a missing value.
fn short(value: Option<String>, n: usize) -> String {
    value
        .map(|text| text.chars().take(n).collect())
        .unwrap_or_default()
}

fn yes(flag: Option<&Value>, on: &str, off: &str) -> String {
    if matches!(flag, Some(Value::Bool(true))) {
        on.to_owned()
    } else {
        off.to_owned()
    }
}

/// `_render_repo`.
#[must_use]
pub fn repo_view(console: &Console, repo: &Value) -> String {
    let tracked = tracked_of(repo).join(", ");
    let shallow =
        matches!(get(repo, "is_shallow"), Some(Value::Bool(true))).then(|| "True".to_owned());
    let archived = Some(when(get(repo, "archived_at"))).filter(|text| !text.is_empty());
    let pairs: Vec<(&str, Option<String>)> = vec![
        ("id", opt(get(repo, "id"))),
        ("name", opt(get(repo, "name"))),
        ("access", opt(get(repo, "access_mode"))),
        ("health", opt(get(repo, "health_status"))),
        ("default branch", opt(get(repo, "default_branch"))),
        ("tracked", Some(tracked)),
        (
            "fingerprint",
            Some(short(opt(get(repo, "identity_fingerprint")), 16)),
        ),
        ("shallow", shallow),
        ("last scanned", Some(when(get(repo, "last_scanned_at")))),
        ("archived", archived),
    ];
    let borrowed: Vec<(&str, Option<&str>)> =
        pairs.iter().map(|(k, v)| (*k, v.as_deref())).collect();
    let name = s(get(repo, "name"));
    let mut out = console.kv(&borrowed, Some(&name)).text;
    let checkouts: Vec<Vec<String>> = items(get(repo, "checkouts"))
        .iter()
        .map(|c| {
            vec![
                s(get(c, "path")),
                s(get(c, "kind")),
                yes(get(c, "is_primary"), "yes", ""),
                yes(get(c, "available"), "yes", "NO"),
            ]
        })
        .collect();
    out.push_str(
        &console
            .table(
                &checkouts,
                &["checkout", "kind", "primary", "available"],
                None,
            )
            .text,
    );
    let remotes: Vec<Vec<String>> = items(get(repo, "remotes"))
        .iter()
        .map(|r| vec![s(get(r, "name")), s(get(r, "url"))])
        .collect();
    if !remotes.is_empty() {
        out.push_str(&console.table(&remotes, &["remote", "url"], None).text);
    }
    out
}

/// The human view of `repo list`.
#[must_use]
pub fn list_view(console: &Console, repos: &Value) -> String {
    let rows: Vec<Vec<String>> = match repos {
        Value::List(list) => list
            .iter()
            .map(|r| {
                let checkouts = items(get(r, "checkouts"));
                vec![
                    s(get(r, "name")),
                    s(get(r, "access_mode")),
                    s(get(r, "health_status")),
                    checkouts.len().to_string(),
                    checkouts
                        .first()
                        .map(|c| s(get(c, "path")))
                        .unwrap_or_default(),
                    when(get(r, "last_scanned_at")),
                ]
            })
            .collect(),
        _ => Vec::new(),
    };
    console
        .table(
            &rows,
            &[
                "name",
                "access",
                "health",
                "checkouts",
                "primary path",
                "scanned",
            ],
            None,
        )
        .text
}

/// The human view of `repo status`.
#[must_use]
pub fn status_view(console: &Console, status: &Value) -> String {
    // `kv` leaves out an empty list as well as a missing value.
    let listed = |name: &str| match get(status, name) {
        Some(Value::List(list)) if list.is_empty() => None,
        other => opt(other),
    };
    let pairs: Vec<(&str, Option<String>)> = vec![
        ("path", opt(get(status, "checkout_path"))),
        ("available", opt(get(status, "available"))),
        ("branch", opt(get(status, "branch"))),
        ("head", Some(short(opt(get(status, "head")), 12))),
        ("ahead", opt(get(status, "ahead"))),
        ("behind", opt(get(status, "behind"))),
        ("staged", listed("staged")),
        ("unstaged", listed("unstaged")),
        ("untracked", listed("untracked")),
        ("error", opt(get(status, "error"))),
    ];
    let borrowed: Vec<(&str, Option<&str>)> =
        pairs.iter().map(|(k, v)| (*k, v.as_deref())).collect();
    let name = s(get(status, "name"));
    console.kv(&borrowed, Some(&name)).text
}

/// The line `repo remove` prints.
#[must_use]
pub fn removed_line(console: &Console, repo: &Value) -> String {
    console
        .print(&format!(
            "removed registration of {}; files on disk untouched",
            s(get(repo, "name"))
        ))
        .text
}
