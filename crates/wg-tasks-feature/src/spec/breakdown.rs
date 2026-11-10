//! The `task import-breakdown` report (Pure): the `Existing` task read from a task output, the
//! JSON answer of a plan (`wg_wire::dto::mcp_outputs::import_breakdown`) and its human view. The
//! parsing and the plan are `wg_tasks::breakdown`.

use serde_json::{Value as Json, json};
use std::collections::HashMap;
use wg_iface::cli::args::{get, items, opt, python, s};
use wg_json::Value;
use wg_model::py_repr_str;
use wg_render::Console;
use wg_tasks::breakdown::{DESCRIPTION_DIGEST, Entry, Existing, Plan, Ref};
use wg_wire::dto::mcp_outputs::{
    breakdown_kept, breakdown_row, breakdown_unresolved, import_breakdown,
};

/// The fields the import reads of a task output.
#[must_use]
pub fn existing_of(task: &Value) -> Existing {
    let texts = |name: &str| items(get(task, name)).iter().map(python).collect();
    Existing {
        id: s(get(task, "id")),
        key: s(get(task, "key")),
        title: s(get(task, "title")),
        description: s(get(task, "description")),
        status: s(get(task, "status")),
        tags: texts("tags"),
        parent_id: opt(get(task, "parent_id")),
        depends_on: texts("depends_on"),
        digest: get(task, "metadata")
            .and_then(|metadata| get(metadata, DESCRIPTION_DIGEST))
            .and_then(|digest| opt(Some(digest))),
        acceptance_criteria: items(get(task, "acceptance_criteria"))
            .iter()
            .map(python)
            .collect(),
    }
}

/// The breakdown id a task was imported under, when it was imported from `doc_ref`.
#[must_use]
pub fn imported_id(task: &Value, doc_ref: &str) -> Option<String> {
    let metadata = get(task, "metadata")?;
    (opt(get(metadata, "breakdown_doc")).as_deref() == Some(doc_ref))
        .then(|| s(get(metadata, "breakdown_id")))
}

/// The description the task holds after the import: the pending change, else the stored text
/// (a started or edited task keeps its own), else the parsed one for a new task.
fn planned_description(entry: &Entry) -> &str {
    entry
        .changes
        .description
        .as_deref()
        .or_else(|| entry.current.as_ref().map(|c| c.description.as_str()))
        .unwrap_or(&entry.item.description)
}

/// The acceptance criteria the task holds after the import, by the same rule as the description.
fn planned_criteria(entry: &Entry) -> &[String] {
    entry
        .changes
        .acceptance_criteria
        .as_deref()
        .or_else(|| {
            entry
                .current
                .as_ref()
                .map(|c| c.acceptance_criteria.as_slice())
        })
        .unwrap_or(&entry.item.acceptance)
}

/// The answer of an import: the plan as it stands once the shell has applied it (or not).
#[must_use]
pub fn answer(plan: &Plan, source: &str, dry_run: bool) -> Value {
    let keys: HashMap<&str, Option<&str>> = plan
        .entries
        .iter()
        .map(|entry| (entry.item.id.as_str(), entry.key.as_deref()))
        .collect();
    let rows = plan
        .entries
        .iter()
        .zip(&plan.refs)
        .map(|(entry, refs)| {
            let depends: Vec<&str> = refs
                .iter()
                .map(|r| match r {
                    Ref::Section(id) => keys
                        .get(id.as_str())
                        .copied()
                        .flatten()
                        .unwrap_or(id.as_str()),
                    Ref::Task(target) => target.key.as_str(),
                })
                .collect();
            let tags: Vec<&str> = entry.desired_tags.iter().map(String::as_str).collect();
            let criteria: Vec<&str> = planned_criteria(entry).iter().map(String::as_str).collect();
            breakdown_row(
                &entry.item.id,
                entry.key.as_deref(),
                &entry.item.title,
                entry.action,
                &entry.status,
                &depends,
                &tags,
                planned_description(entry),
                &criteria,
            )
        })
        .collect();
    let map: Vec<(&str, Option<&str>)> = plan
        .entries
        .iter()
        .map(|entry| (entry.item.id.as_str(), entry.key.as_deref()))
        .collect();
    let unresolved = plan
        .unresolved
        .iter()
        .map(|u| breakdown_unresolved(&u.id, &u.dependency, u.reason))
        .collect();
    let kept = plan
        .kept
        .iter()
        .map(|k| breakdown_kept(&k.id, &k.key, k.reason))
        .collect();
    let count = |f: &dyn Fn(&wg_tasks::breakdown::Entry) -> bool| {
        i64::try_from(plan.entries.iter().filter(|e| f(e)).count()).unwrap_or(i64::MAX)
    };
    import_breakdown(
        source,
        dry_run,
        rows,
        &map,
        unresolved,
        kept,
        count(&|e| e.action == "created"),
        count(&|e| e.action == "updated"),
        count(&|e| e.action == "unchanged"),
        count(&|e| e.status == "ready"),
        count(&|e| e.status == "backlog"),
    )
}

/// `_render_breakdown`: the table, the unresolved and kept lines and the counts.
#[must_use]
pub fn import_view(console: &Console, result: &Value) -> String {
    let dry_run = matches!(get(result, "dry_run"), Some(Value::Bool(true)));
    let rows: Vec<Vec<String>> = items(get(result, "tasks"))
        .iter()
        .map(|t| {
            vec![
                s(get(t, "id")),
                opt(get(t, "key")).unwrap_or_else(|| "-".to_owned()),
                s(get(t, "action")),
                s(get(t, "status")),
                items(get(t, "acceptance_criteria")).len().to_string(),
                s(get(t, "title")).chars().take(60).collect(),
            ]
        })
        .collect();
    let title = format!(
        "import breakdown{}",
        if dry_run { " (dry run)" } else { "" }
    );
    let mut out = console
        .table(
            &rows,
            &["id", "key", "action", "status", "criteria", "title"],
            Some(&title),
        )
        .text;
    for u in items(get(result, "unresolved")) {
        let line = format!(
            "[yellow]unresolved[/yellow] {} depends on {}: {}",
            s(get(u, "id")),
            py_repr_str(&s(get(u, "dependency"))),
            s(get(u, "reason"))
        );
        out.push_str(&console.print(&line).text);
    }
    for k in items(get(result, "kept_descriptions")) {
        let who = opt(get(k, "key")).unwrap_or_else(|| s(get(k, "id")));
        let line = format!(
            "[yellow]kept[/yellow] {who}: {}; not overwritten",
            s(get(k, "reason"))
        );
        out.push_str(&console.print(&line).text);
    }
    let counts = get(result, "counts");
    let n = |name: &str| s(counts.and_then(|c| get(c, name)));
    let line = format!(
        "{} created, {} updated, {} unchanged; {} ready, {} backlog{}",
        n("created"),
        n("updated"),
        n("unchanged"),
        n("ready"),
        n("backlog"),
        if dry_run { " (nothing changed)" } else { "" }
    );
    out.push_str(&console.print(&line).text);
    out
}

/// The body of the update an existing entry needs. Only the fields that changed are given.
#[must_use]
pub fn update_body(entry: &Entry) -> Json {
    let changes = &entry.changes;
    let mut fields = serde_json::Map::new();
    for (name, value) in [
        ("title", &changes.title),
        ("description", &changes.description),
        ("parent", &changes.parent),
    ] {
        if let Some(text) = value {
            fields.insert(name.to_owned(), json!(text));
        }
    }
    if let Some(digest) = &changes.digest {
        fields.insert("metadata".to_owned(), json!({ DESCRIPTION_DIGEST: digest }));
    }
    if let Some(criteria) = &changes.acceptance_criteria {
        fields.insert("acceptance_criteria".to_owned(), json!(criteria));
    }
    if !entry.add_tags.is_empty() {
        fields.insert("add_tags".to_owned(), json!(entry.add_tags));
    }
    if !entry.remove_tags.is_empty() {
        fields.insert("remove_tags".to_owned(), json!(entry.remove_tags));
    }
    Json::Object(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wg_tasks::breakdown::{PlanInput, parse_items, plan, section_lines};

    const DOC: &str = "\
## Plan

**A1 Build it**
Build the thing.
Acceptance:
- `cargo check` passes
- the report lists both files

**A2 Second**
Second prose.
";

    fn dry_run_answer() -> Value {
        let parsed = parse_items(&section_lines(DOC, "Plan").expect("section")).expect("items");
        let existing = HashMap::new();
        let lookups = HashMap::new();
        let tags: Vec<String> = Vec::new();
        let planned = plan(&PlanInput {
            items: &parsed,
            existing: &existing,
            lookups: &lookups,
            parent: Some(("WG-1", "parent-id")),
            tags: &tags,
        });
        answer(&planned, "doc#Plan", true)
    }

    #[test]
    fn dry_run_plan_lists_description_and_criteria_per_task() {
        let result = dry_run_answer();
        let tasks = items(get(&result, "tasks"));
        let first = &tasks[0];
        assert_eq!(s(get(first, "id")), "A1");
        assert_eq!(opt(get(first, "key")), None);
        let criteria: Vec<String> = items(get(first, "acceptance_criteria"))
            .iter()
            .map(python)
            .collect();
        assert_eq!(
            criteria,
            vec![
                "`cargo check` passes".to_owned(),
                "the report lists both files".to_owned()
            ]
        );
        let description = s(get(first, "description"));
        assert!(description.contains("Build the thing."));
        assert!(!description.contains("Acceptance:"));
        assert!(!description.contains("cargo check"));
        let second = &tasks[1];
        assert!(items(get(second, "acceptance_criteria")).is_empty());
        assert!(s(get(second, "description")).contains("Second prose."));
    }

    #[test]
    fn human_view_has_a_criteria_count_column() {
        let text = import_view(&Console::new(120), &dry_run_answer());
        assert!(text.contains("2"));
        assert!(text.contains("criteria"));
        assert!(text.contains("dry run"));
    }
}
