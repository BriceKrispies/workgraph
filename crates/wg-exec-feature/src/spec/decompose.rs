//! The decomposition of a task into children (`Scheduler.decompose` of
//! `orchestration/scheduler.py`): the model's schema and instructions, the validation of what it
//! proposes, the dependency order of the children, and the bodies the children are created from.

use std::collections::BTreeMap;

use rpds::VectorSync;
use serde_json::{Map, Value as Json, json};
use wg_clispec::args::{get, python};
use wg_model::event::{EventDraft, EventPayload, TaskDecomposed};
use wg_model::stamp::Stamp;
use wg_model::{Decision, py_repr_str};
use wg_storecodec::task_world_sql::TaskWorld;
use wg_tasks::edit::Enrichment;
use wg_tasks::shape::key_of;
use wg_tasks_feature::spec::requests::{read_create, validate_create};
use wg_tasks_feature::spec::writes::create_task;
use wg_types::pyval::prefix;
use wg_types::{Id, TaskKey, WgError};
use wg_validate::{Extra, Field, Model, Schema, StrRules, validate};

/// The system prompt of the decomposition request.
pub const DECOMPOSE_SYSTEM: &str = "You split a software task into smaller child tasks. Respond with JSON matching the schema. \
Use short refs (c1, c2, ...). depends_on lists refs of children that must be finished first. \
repositories must be chosen from the provided list. Do not invent work beyond the task.";

/// The longest rationale the `task.decomposed` event keeps.
pub const RATIONALE_EVENT_LIMIT: usize = 1000;

/// How many validation errors the failure shows (`exc.errors()[:5]`).
const SHOWN_ERRORS: usize = 5;

fn strings() -> Schema {
    Schema::List {
        item: Box::new(Schema::Str(StrRules::default())),
        min_length: None,
        max_length: None,
    }
}

/// `TaskDecomposition` as the validator describes it.
pub fn model() -> Model {
    let child = Model::new(
        "ChildProposal",
        Extra::Forbid,
        vec![
            Field::required(
                "ref",
                Schema::Str(StrRules {
                    pattern: Some("^[A-Za-z0-9_-]{1,20}$".to_owned()),
                    ..StrRules::default()
                }),
            ),
            Field::required(
                "title",
                Schema::Str(StrRules {
                    min_length: Some(1),
                    max_length: Some(300),
                    ..StrRules::default()
                }),
            ),
            Field::optional(
                "description",
                Schema::Str(StrRules {
                    max_length: Some(5000),
                    ..StrRules::default()
                }),
                json!(""),
            ),
            Field::optional("depends_on", strings(), json!([])),
            Field::optional("repositories", strings(), json!([])),
        ],
    );
    Model::new(
        "TaskDecomposition",
        Extra::Forbid,
        vec![
            Field::required(
                "children",
                Schema::List {
                    item: Box::new(Schema::Model(Box::new(child))),
                    min_length: Some(1),
                    max_length: Some(15),
                },
            ),
            Field::optional("rationale", Schema::Str(StrRules::default()), json!("")),
        ],
    )
}

/// `TaskDecomposition.model_json_schema()` with its title, the schema the model is asked to fill.
pub fn json_schema() -> Json {
    json!({
        "$defs": {
            "ChildProposal": {
                "additionalProperties": false,
                "properties": {
                    "ref": {"pattern": "^[A-Za-z0-9_-]{1,20}$", "title": "Ref", "type": "string"},
                    "title": {"maxLength": 300, "minLength": 1, "title": "Title", "type": "string"},
                    "description": {
                        "default": "", "maxLength": 5000, "title": "Description", "type": "string"
                    },
                    "depends_on": {
                        "items": {"type": "string"}, "title": "Depends On", "type": "array"
                    },
                    "repositories": {
                        "items": {"type": "string"}, "title": "Repositories", "type": "array"
                    }
                },
                "required": ["ref", "title"],
                "title": "ChildProposal",
                "type": "object"
            }
        },
        "additionalProperties": false,
        "properties": {
            "children": {
                "items": {"$ref": "#/$defs/ChildProposal"},
                "maxItems": 15,
                "minItems": 1,
                "title": "Children",
                "type": "array"
            },
            "rationale": {"default": "", "title": "Rationale", "type": "string"}
        },
        "required": ["children"],
        "title": "TaskDecomposition",
        "type": "object"
    })
}

/// A child the model proposed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Child {
    pub reference: String,
    pub title: String,
    pub description: String,
    pub depends_on: Vec<String>,
    pub repositories: Vec<String>,
}

/// A validated proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub children: Vec<Child>,
    pub rationale: String,
    /// The proposal as `model_dump()` gives it.
    pub dump: Json,
}

fn texts(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Json::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn text(value: Option<&Json>) -> String {
    value.and_then(Json::as_str).unwrap_or_default().to_owned()
}

/// `TaskDecomposition.model_validate(data)`, or the `validation_failed` error the service raises.
pub fn read_proposal(data: &Map<String, Json>) -> Result<Proposal, WgError> {
    let dump = validate(&model(), &Json::Object(data.clone())).map_err(|errors| {
        WgError::validation("model proposed an invalid decomposition").detail(
            "errors",
            Json::Array(
                errors
                    .iter()
                    .take(SHOWN_ERRORS)
                    .map(wg_validate::ValidationError::to_json)
                    .collect(),
            ),
        )
    })?;
    let children = dump
        .get("children")
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| Child {
                    reference: text(item.get("ref")),
                    title: text(item.get("title")),
                    description: text(item.get("description")),
                    depends_on: texts(item.get("depends_on")),
                    repositories: texts(item.get("repositories")),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Proposal {
        children,
        rationale: text(dump.get("rationale")),
        dump,
    })
}

/// The Python list repr of strings, `['a', 'b']`.
fn list_repr(items: &[String]) -> String {
    let shown: Vec<String> = items.iter().map(|item| py_repr_str(item)).collect();
    format!("[{}]", shown.join(", "))
}

fn cycle(message: String) -> WgError {
    WgError::dependency_cycle(message)
}

/// `validate_decomposition`: checks the references and returns the refs in dependency order, as
/// `graphlib.TopologicalSorter.static_order` gives them.
pub fn validate_decomposition(
    proposal: &Proposal,
    allowed_repos: &[String],
) -> Result<Vec<String>, WgError> {
    let refs: Vec<&str> = proposal
        .children
        .iter()
        .map(|child| child.reference.as_str())
        .collect();
    let distinct: std::collections::BTreeSet<&str> = refs.iter().copied().collect();
    if distinct.len() != refs.len() {
        return Err(WgError::validation(
            "decomposition contains duplicate child refs".to_owned(),
        ));
    }
    for child in &proposal.children {
        let unknown: Vec<String> = child
            .depends_on
            .iter()
            .filter(|dep| !refs.contains(&dep.as_str()))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(WgError::validation(format!(
                "child {} depends on unknown refs {}",
                child.reference,
                list_repr(&unknown)
            )));
        }
        if child.depends_on.contains(&child.reference) {
            return Err(cycle(format!(
                "child {} depends on itself",
                child.reference
            )));
        }
        let outside: Vec<String> = child
            .repositories
            .iter()
            .filter(|repo| !allowed_repos.contains(repo))
            .cloned()
            .collect();
        if !outside.is_empty() {
            return Err(WgError::validation(format!(
                "child {} references repositories outside the parent task: {}",
                child.reference,
                list_repr(&outside)
            )));
        }
    }
    let graph: Vec<(String, Vec<String>)> = proposal
        .children
        .iter()
        .map(|child| (child.reference.clone(), child.depends_on.clone()))
        .collect();
    static_order(&graph).map_err(|found| {
        cycle(format!(
            "proposed child dependencies are cyclic: {}",
            list_repr(&found)
        ))
    })
}

/// `TopologicalSorter(graph).static_order()` for a graph of node to predecessors. A cycle is
/// returned as `_find_cycle` finds it.
fn static_order(graph: &[(String, Vec<String>)]) -> Result<Vec<String>, Vec<String>> {
    // `_node2info`: nodes in the order `add` first meets them; each with its successors.
    let mut nodes: Vec<String> = Vec::new();
    let mut successors: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut waiting: BTreeMap<String, usize> = BTreeMap::new();
    let meet = |node: &str, nodes: &mut Vec<String>| {
        if !nodes.iter().any(|known| known == node) {
            nodes.push(node.to_owned());
        }
    };
    for (node, predecessors) in graph {
        meet(node, &mut nodes);
        *waiting.entry(node.clone()).or_default() += predecessors.len();
        for predecessor in predecessors {
            meet(predecessor, &mut nodes);
            successors
                .entry(predecessor.clone())
                .or_default()
                .push(node.clone());
        }
    }
    if let Some(found) = find_cycle(&nodes, &successors) {
        return Err(found);
    }
    let mut ready: Vec<String> = nodes
        .iter()
        .filter(|node| waiting.get(*node).copied().unwrap_or(0) == 0)
        .cloned()
        .collect();
    let mut order = Vec::new();
    while !ready.is_empty() {
        let batch = std::mem::take(&mut ready);
        for node in &batch {
            order.push(node.clone());
        }
        for node in &batch {
            for next in successors.get(node).into_iter().flatten() {
                let count = waiting.entry(next.clone()).or_default();
                *count -= 1;
                if *count == 0 {
                    ready.push(next.clone());
                }
            }
        }
    }
    Ok(order)
}

/// `TopologicalSorter._find_cycle`: a depth-first walk over the successors, in node order.
fn find_cycle(nodes: &[String], successors: &BTreeMap<String, Vec<String>>) -> Option<Vec<String>> {
    let mut seen: Vec<String> = Vec::new();
    let mut stack: Vec<String> = Vec::new();
    // The next successor index to try, per stack entry.
    let mut cursor: Vec<usize> = Vec::new();
    for start in nodes {
        if seen.contains(start) {
            continue;
        }
        let mut node = start.clone();
        loop {
            if seen.contains(&node) {
                if let Some(at) = stack.iter().position(|member| *member == node) {
                    return Some(stack[at..].iter().cloned().chain([node]).collect());
                }
            } else {
                seen.push(node.clone());
                cursor.push(0);
                stack.push(node.clone());
            }
            // Back up to the topmost entry with another successor.
            let mut next = None;
            while let Some(top) = stack.last() {
                let followers = successors.get(top).map_or(&[][..], Vec::as_slice);
                let index = cursor.last().copied().unwrap_or(0);
                if let Some(follower) = followers.get(index) {
                    if let Some(last) = cursor.last_mut() {
                        *last += 1;
                    }
                    next = Some(follower.clone());
                    break;
                }
                stack.pop();
                cursor.pop();
            }
            match next {
                Some(follower) => node = follower,
                None => break,
            }
        }
    }
    None
}

/// The prompt of the decomposition request.
pub fn prompt(
    repositories: &[String],
    title: &str,
    description: &str,
    criteria: &[String],
) -> String {
    let listed = if repositories.is_empty() {
        "(none)".to_owned()
    } else {
        repositories.join(", ")
    };
    let text: Vec<String> = [format!("Title: {title}"), description.to_owned()]
        .into_iter()
        .chain(criteria.iter().map(|criterion| format!("- {criterion}")))
        .collect();
    format!(
        "Available repositories: {listed}\n\n<task>\n{}\n</task>",
        text.join("\n")
    )
}

/// The `TaskCreate` body of one child: its title and description under the parent, the
/// repositories it names (else all of the parent's), after the children it depends on.
pub fn child_body(
    child: &Child,
    parent: &str,
    parent_repositories: &[String],
    created: &BTreeMap<String, String>,
) -> Json {
    let repositories = if child.repositories.is_empty() {
        parent_repositories.to_vec()
    } else {
        child.repositories.clone()
    };
    json!({
        "title": child.title,
        "description": child.description,
        "parent": parent,
        "repositories": repositories,
        "depends_on": child
            .depends_on
            .iter()
            .filter_map(|dep| created.get(dep))
            .collect::<Vec<_>>(),
        "status": "ready",
        "tags": ["decomposed"],
    })
}

/// What `create_children` has built so far.
struct Built {
    world: TaskWorld,
    decision: Decision<()>,
    /// Child ref to the id of the task created for it.
    created: BTreeMap<String, String>,
    keys: Vec<String>,
}

/// What the children of a decomposition are created under.
pub struct Parent<'a> {
    pub task: Id,
    pub workspace: Id,
    /// The names of the repositories the parent links.
    pub repositories: &'a [String],
    pub provider: &'a str,
    pub model: &'a str,
}

/// `Scheduler.decompose` with `create`: one task per child, in dependency `order`, each after the
/// tasks it depends on, then the `task.decomposed` event on the parent. The reply is the keys
/// created.
pub fn create_children(
    world: &TaskWorld,
    stamp: &Stamp,
    parent: &Parent<'_>,
    proposal: &Proposal,
    order: &[String],
    enrich: Enrichment,
) -> Result<Decision<Vec<String>>, WgError> {
    let by_ref: BTreeMap<&str, &Child> = proposal
        .children
        .iter()
        .map(|child| (child.reference.as_str(), child))
        .collect();
    let start = Built {
        world: world.clone(),
        decision: Decision::new(()),
        created: BTreeMap::new(),
        keys: Vec::new(),
    };
    let done = order
        .iter()
        .enumerate()
        .try_fold(start, |built, (index, reference)| {
            let child = by_ref.get(reference.as_str()).ok_or_else(|| {
                WgError::validation(format!("decomposition has no child {reference}"))
            })?;
            let body = child_body(
                child,
                &parent.task.to_string(),
                parent.repositories,
                &built.created,
            );
            let read = read_create(&validate_create(&body)?)?;
            // Each child draws its own ids.
            let shifted = Stamp {
                entropy: stamp
                    .entropy
                    .map(|byte| byte ^ u8::try_from(index + 1).unwrap_or(u8::MAX)),
                ..stamp.clone()
            };
            let made = create_task(&built.world, &shifted, parent.workspace, read, enrich)?;
            let id = get(&made.reply, "id").map(python).unwrap_or_default();
            let key = key_of(&made.reply).unwrap_or_default();
            let next = TaskWorld {
                tasks: built.world.tasks.apply_all(made.writes.iter()),
                stacks: built.world.stacks.clone(),
            };
            Ok::<Built, WgError>(Built {
                world: next,
                decision: built.decision.then(made.with_reply(())),
                created: built
                    .created
                    .into_iter()
                    .chain([(reference.clone(), id)])
                    .collect(),
                keys: built.keys.into_iter().chain([key]).collect(),
            })
        })?;
    let children: VectorSync<TaskKey> = done
        .keys
        .iter()
        .filter_map(|key| TaskKey::parse(key).ok())
        .collect();
    let (event_id, _) = stamp.ids().next();
    let event = EventDraft::new(
        stamp,
        event_id,
        Some(parent.workspace),
        parent.task,
        EventPayload::TaskDecomposed(TaskDecomposed {
            children,
            provider: parent.provider.to_owned(),
            model: parent.model.to_owned(),
            rationale: prefix(&proposal.rationale, RATIONALE_EVENT_LIMIT),
        }),
    );
    Ok(done.decision.event(event).with_reply(done.keys))
}
