//! The pure half of the `git` command group (`cli/commands/gitcmd.py`): the clap command and the
//! human views of its results.

use wg_clispec::args::{get, items, python, s};
use wg_clispec::tree::{SNAPSHOT_JSON, placeholder};
use wg_clispec::{Snapshot, Usage};
use wg_json::Value;
use wg_render::Console;

/// Click's usage line of `git log`, for its `--limit` usage error.
pub const LOG: Usage = Usage {
    usage: "workgraph git log [OPTIONS] {repository}",
    command: "workgraph git log",
};

/// Click's usage line of `git analyze`, for its `--limit` usage error.
pub const ANALYZE: Usage = Usage {
    usage: "workgraph git analyze [OPTIONS] {repository}",
    command: "workgraph git analyze",
};

/// Click's usage line of `git changes`, for its `--min-confidence` usage error.
pub const CHANGES: Usage = Usage {
    usage: "workgraph git changes [OPTIONS] {task}",
    command: "workgraph git changes",
};

/// The clap command of the `git` group, from the snapshot's own tree.
#[must_use]
pub fn command() -> clap::Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, "git"),
        Err(_) => wg_clispec::tree::command("git"),
    }
}

/// `text[:n]`: the first `n` characters.
fn cut(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// `value[:8]` of an optional id, as `short()` shows it.
fn short8(value: Option<&Value>) -> String {
    let text = s(value);
    cut(&text, 8)
}

/// `value[:10]`, the SHA prefix the tables show.
fn sha10(value: Option<&Value>) -> String {
    cut(&s(value), 10)
}

/// The text of a value that may be a string, or empty when absent.
fn text_of(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(other) => python(other),
    }
}

/// `+insertions/-deletions`.
fn lines_of(item: &Value) -> String {
    format!(
        "+{}/-{}",
        s(get(item, "insertions")),
        s(get(item, "deletions"))
    )
}

/// The task keys of a list of `{task_key, ...}` objects, comma-joined.
fn task_keys(list: Option<&Value>) -> String {
    items(list)
        .iter()
        .map(|task| s(get(task, "task_key")))
        .collect::<Vec<_>>()
        .join(",")
}

/// `git sync`: one row per repository result.
#[must_use]
pub fn sync_view(console: &Console, results: &Value) -> String {
    let rows: Vec<Vec<String>> = items(Some(results))
        .iter()
        .map(|result| {
            vec![
                s(get(result, "repository")),
                s(get(result, "mode")),
                s(get(result, "new_commits")),
                s(get(result, "refs_changed")),
                s(get(result, "marked_unreachable")),
                s(get(result, "equivalences")),
                s(get(result, "links_created")),
                match get(result, "shallow") {
                    Some(Value::Bool(true)) => "yes".to_owned(),
                    _ => String::new(),
                },
            ]
        })
        .collect();
    console
        .table(
            &rows,
            &[
                "repository",
                "mode",
                "new",
                "refs",
                "unreachable",
                "equivalent",
                "links",
                "shallow",
            ],
            None,
        )
        .text
}

/// `git log`: one row per commit of the page.
#[must_use]
pub fn log_view(console: &Console, page: &Value) -> String {
    let rows: Vec<Vec<String>> = items(get(page, "items"))
        .iter()
        .map(|commit| {
            vec![
                sha10(get(commit, "sha")),
                cut(&s(get(commit, "subject")), 60),
                s(get(commit, "author")),
                lines_of(commit),
                task_keys(get(commit, "tasks")),
            ]
        })
        .collect();
    console
        .table(&rows, &["sha", "subject", "author", "lines", "tasks"], None)
        .text
}

/// `git show`: the commit's key/value block, then one row per changed file.
#[must_use]
pub fn show_view(console: &Console, detail: &Value) -> String {
    let author = get(detail, "author")
        .map(|person| s(get(person, "name")))
        .unwrap_or_default();
    let tasks = items(get(detail, "tasks"))
        .iter()
        .map(|task| Value::Str(wg_json::PyStr::from(s(get(task, "task_key")))))
        .collect::<Vec<_>>();
    let tasks_text = if tasks.is_empty() {
        None
    } else {
        Some(python(&Value::List(tasks)))
    };
    let merge = python(get(detail, "is_merge").unwrap_or(&Value::Null));
    let reachable = python(get(detail, "reachable").unwrap_or(&Value::Null));
    let mut out = console
        .kv(
            &[
                ("sha", Some(&s(get(detail, "sha")))),
                ("subject", Some(&s(get(detail, "subject")))),
                ("author", Some(&author)),
                ("merge", Some(&merge)),
                ("reachable", Some(&reachable)),
                ("tasks", tasks_text.as_deref()),
            ],
            Some("commit"),
        )
        .text;
    let rows: Vec<Vec<String>> = items(get(detail, "files"))
        .iter()
        .map(|file| {
            let symbols = items(get(file, "symbols"))
                .iter()
                .map(|symbol| s(get(symbol, "name")))
                .collect::<Vec<_>>()
                .join(", ");
            vec![
                s(get(file, "change_type")),
                s(get(file, "path")),
                lines_of(file),
                s(get(file, "file_class")),
                cut(&symbols, 60),
            ]
        })
        .collect();
    out.push_str(
        &console
            .table(&rows, &["", "path", "lines", "class", "symbols"], None)
            .text,
    );
    out
}

/// `git link`: the confirmation line, from the links the command returns.
#[must_use]
pub fn link_line(console: &Console, links: &Value) -> String {
    let list = items(Some(links));
    let sha = list
        .first()
        .map(|link| sha10(get(link, "sha")))
        .unwrap_or_default();
    console
        .print(&format!("linked {sha} to task ({} link(s))", list.len()))
        .text
}

/// `git unlink`: how many explicit links were removed.
#[must_use]
pub fn unlink_line(console: &Console, removed: &Value) -> String {
    console
        .print(&format!(
            "removed {} explicit link(s)",
            s(get(removed, "removed"))
        ))
        .text
}

/// `git changes`: the commits, the cumulative churn and each final change surface.
#[must_use]
pub fn changes_view(console: &Console, changes: &Value) -> String {
    let rows: Vec<Vec<String>> = items(get(changes, "commits"))
        .iter()
        .map(|commit| {
            let mut sources: Vec<String> = items(get(commit, "links"))
                .iter()
                .map(|link| s(get(link, "source")))
                .collect();
            sources.sort();
            sources.dedup();
            let confidence = match get(commit, "confidence") {
                Some(Value::Float(value)) => format!("{value:.2}"),
                other => text_of(other),
            };
            vec![
                s(get(commit, "repository")),
                sha10(get(commit, "sha")),
                cut(&s(get(commit, "subject")), 50),
                confidence,
                sources.join(","),
            ]
        })
        .collect();
    let mut out = console
        .table(
            &rows,
            &["repository", "sha", "subject", "conf", "sources"],
            None,
        )
        .text;
    let churn_pairs: Vec<(String, String)> = match get(changes, "cumulative_churn") {
        Some(Value::Dict(entries)) => entries
            .iter()
            .map(|(key, value)| (python(&Value::Str(key.clone())), python(value)))
            .collect(),
        _ => Vec::new(),
    };
    let churn_rows: Vec<(&str, Option<&str>)> = churn_pairs
        .iter()
        .filter(|(_, value)| !value.is_empty() && value != "[]" && value != "{}" && value != "None")
        .map(|(key, value)| (key.as_str(), Some(value.as_str())))
        .collect();
    out.push_str(&console.kv(&churn_rows, Some("cumulative churn")).text);
    for surface in items(get(changes, "final_change_surface")) {
        let pairs: Vec<(String, String)> = match surface {
            Value::Dict(entries) => entries
                .iter()
                .filter(|(key, _)| python(&Value::Str(key.clone())) != "detail")
                .map(|(key, value)| (python(&Value::Str(key.clone())), python(value)))
                .collect(),
            _ => Vec::new(),
        };
        let rows: Vec<(&str, Option<&str>)> = pairs
            .iter()
            .filter(|(_, value)| {
                !value.is_empty() && value != "[]" && value != "{}" && value != "None"
            })
            .map(|(key, value)| (key.as_str(), Some(value.as_str())))
            .collect();
        out.push_str(&console.kv(&rows, Some("final change surface")).text);
    }
    out
}

/// `git diff`: each part under its header, then the missing objects.
#[must_use]
pub fn diff_view(console: &Console, diff: &Value) -> String {
    let mut out = String::new();
    for part in items(get(diff, "parts")) {
        let label = match get(part, "range") {
            Some(Value::Null) | None => s(get(part, "sha")),
            Some(range) => python(range),
        };
        out.push_str(
            &console
                .print(&format!(
                    "[bold]# {} {}[/bold]",
                    s(get(part, "repository")),
                    label
                ))
                .text,
        );
        out.push_str(&s(get(part, "diff")));
        out.push('\n');
    }
    for gap in items(get(diff, "missing_objects")) {
        out.push_str(
            &console
                .print(&format!(
                    "[yellow]missing Git objects:[/yellow] {}",
                    python(gap)
                ))
                .text,
        );
    }
    out
}

/// `git surface`: one row per measured repository.
#[must_use]
pub fn surface_view(console: &Console, rows: &Value) -> String {
    let table_rows: Vec<Vec<String>> = items(Some(rows))
        .iter()
        .map(|row| {
            vec![
                short8(get(row, "repository_id")),
                sha10(get(row, "base_sha")),
                sha10(get(row, "final_sha")),
                s(get(row, "files_changed")),
                s(get(row, "insertions")),
                s(get(row, "deletions")),
                s(get(row, "functions_affected")),
            ]
        })
        .collect();
    console
        .table(
            &table_rows,
            &["repo", "base", "final", "files", "+", "-", "functions"],
            None,
        )
        .text
}

/// `git analyze`: the number of commits analysed.
#[must_use]
pub fn analyze_line(console: &Console, result: &Value) -> String {
    console
        .print(&format!(
            "analyzed {} commit(s)",
            s(get(result, "analyzed"))
        ))
        .text
}

/// `git worktrees`: one row per managed worktree.
#[must_use]
pub fn worktrees_view(console: &Console, rows: &Value) -> String {
    let table_rows: Vec<Vec<String>> = items(Some(rows))
        .iter()
        .map(|row| {
            vec![
                short8(get(row, "id")),
                s(get(row, "repository")),
                s(get(row, "status")),
                s(get(row, "branch")),
                s(get(row, "path")),
            ]
        })
        .collect();
    console
        .table(
            &table_rows,
            &["id", "repository", "status", "branch", "path"],
            None,
        )
        .text
}

/// `git cleanup-worktrees`: the path (or id) of each worktree and what happened to it.
#[must_use]
pub fn cleanup_view(console: &Console, rows: &Value) -> String {
    let table_rows: Vec<Vec<String>> = items(Some(rows))
        .iter()
        .map(|row| {
            let worktree = match get(row, "path") {
                Some(path) => python(path),
                None => s(get(row, "worktree_id")),
            };
            let result = match get(row, "action") {
                Some(action) if !matches!(action, Value::Null) => python(action),
                _ => match get(row, "removed") {
                    Some(Value::Bool(true)) => "removed".to_owned(),
                    _ => text_of(get(row, "reason")),
                },
            };
            vec![worktree, result]
        })
        .collect();
    console
        .table(&table_rows, &["worktree", "result"], None)
        .text
}

/// `git install-hook`: the path of the installed hook.
#[must_use]
pub fn hook_line(console: &Console, result: &Value) -> String {
    console
        .print(&format!("installed {}", s(get(result, "hook"))))
        .text
}
