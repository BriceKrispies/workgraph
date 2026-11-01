//! `task import-breakdown` (the shell half of `core/services/breakdown.py`). The markdown is read
//! and validated, the tasks it was imported as before and the dependencies named by key are
//! looked up, `wg_tasks::breakdown::plan` decides what each item needs, and the writes run in
//! order through the task services. Each service call commits on its own, as for triage verdicts.
//! A dry run plans the same actions and writes nothing.

use serde_json::json;
use std::collections::HashMap;
use wg_iface::cli::args::{get, items, opt};
use wg_json::Value;
use wg_model::py_repr_str;
use wg_runtime::{App, RequestContext};
use wg_tasks::breakdown::{
    self, Existing, Plan, PlanInput, Ref, description_digest, external_ids, needs_update,
    parse_items, promotes, ref_key, section_lines,
};
use wg_types::{ErrorCode, WgError};

use super::{task_add_dependency, task_create, task_list, task_show, task_update};
use crate::spec::args::dependency_body;
use crate::spec::breakdown::{answer, existing_of, imported_id, update_body};
use crate::spec::triage::created_key;

/// What `task import-breakdown` and `task_import_breakdown` are asked.
pub struct ImportRequest<'a> {
    pub markdown: &'a str,
    pub section: &'a str,
    pub source: &'a str,
    pub parent: &'a str,
    pub tags: &'a [String],
    pub project: Option<&'a str>,
    pub repository: Option<&'a str>,
    pub dry_run: bool,
}

/// Creates or updates the tasks of `section`; the answer is the plan.
pub fn task_import_breakdown(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    request: &ImportRequest<'_>,
) -> Result<Value, WgError> {
    let items = parse_items(&section_lines(request.markdown, request.section)?)?;
    if items.is_empty() {
        return Err(WgError::validation(format!(
            "the section {} holds no tasks",
            py_repr_str(request.section)
        )));
    }
    let doc_ref = format!("{}#{}", request.source, request.section);
    let existing = existing_by_id(app, context, workspace, &doc_ref)?;
    let parent = find(app, context, workspace, request.parent)?
        .ok_or_else(|| WgError::not_found(format!("task {} not found", request.parent)))?;
    let mut lookups = HashMap::new();
    for token in external_ids(&items) {
        let found = find(app, context, workspace, &token)?;
        lookups.insert(token, found);
    }
    let mut plan = breakdown::plan(&PlanInput {
        items: &items,
        existing: &existing,
        lookups: &lookups,
        parent: Some((request.parent, parent.id.as_str())),
        tags: request.tags,
    });
    if !request.dry_run {
        apply(app, context, workspace, &mut plan, request, &doc_ref)?;
    }
    Ok(answer(&plan, &doc_ref, request.dry_run))
}

/// The task a reference names; `None` when there is no such task or the reference is invalid.
fn find(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    reference: &str,
) -> Result<Option<Existing>, WgError> {
    let shown = task_show(app, context, workspace, &[reference.to_owned()])?;
    if let Some((_, error)) = shown.errors.into_iter().next() {
        return match error.code {
            ErrorCode::NotFound | ErrorCode::ValidationFailed => Ok(None),
            _ => Err(error),
        };
    }
    Ok(shown.items.first().map(existing_of))
}

/// Tasks already imported from `doc_ref`, by breakdown id (archived ones included).
fn existing_by_id(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    doc_ref: &str,
) -> Result<HashMap<String, Existing>, WgError> {
    let mut found = HashMap::new();
    let mut offset = 0_i64;
    loop {
        let body = json!({"include_archived": true, "limit": 500, "offset": offset});
        let page = task_list(app, context, workspace, &body)?;
        for task in items(get(&page, "items")) {
            if let Some(id) = imported_id(task, doc_ref) {
                found.entry(id).or_insert_with(|| existing_of(task));
            }
        }
        match opt(get(&page, "next_offset")).and_then(|n| n.parse().ok()) {
            Some(next) => offset = next,
            None => return Ok(found),
        }
    }
}

/// Writes the plan: create, update, add dependencies, then promote the ready tasks.
fn apply(
    app: &mut App,
    context: &RequestContext,
    ws: Option<&str>,
    plan: &mut Plan,
    request: &ImportRequest<'_>,
    doc_ref: &str,
) -> Result<(), WgError> {
    let projects: Vec<&str> = request.project.into_iter().collect();
    let repositories: Vec<&str> = request.repository.into_iter().collect();
    for entry in &mut plan.entries {
        if entry.current.is_none() {
            let body = json!({
                "title": entry.item.title,
                "description": entry.item.description,
                "tags": entry.desired_tags,
                "parent": request.parent,
                "projects": projects,
                "repositories": repositories,
                "metadata": {
                    "breakdown_id": entry.item.id,
                    "breakdown_doc": doc_ref,
                    breakdown::DESCRIPTION_DIGEST: description_digest(&entry.item.description),
                },
            });
            entry.key = Some(created_key(&task_create(app, context, ws, &body)?));
        }
    }
    for entry in plan.entries.iter().filter(|e| needs_update(e)) {
        let key = entry.key.clone().unwrap_or_default();
        task_update(app, context, ws, &key, &update_body(entry))?;
    }
    for entry in &plan.entries {
        for dependency in &entry.deps_to_add {
            let target = ref_key(plan, dependency).ok_or_else(|| {
                let id = match dependency {
                    Ref::Section(id) => id.clone(),
                    Ref::Task(task) => task.key.clone(),
                };
                WgError::validation(format!("dependency {id} of {} has no task", entry.item.id))
            })?;
            let key = entry.key.clone().unwrap_or_default();
            task_add_dependency(app, context, ws, &key, &dependency_body(&target))?;
        }
    }
    for entry in plan.entries.iter().filter(|e| promotes(e)) {
        let key = entry.key.clone().unwrap_or_default();
        task_update(app, context, ws, &key, &json!({"status": "ready"}))?;
    }
    Ok(())
}
