//! GV-19: every case of `golden/taskgraph/*.json` must match the dependency, hierarchy, promotion,
//! ready-queue and graph functions of `wg-tasks`.
//!
//! The Python generator drives a whole Workgraph and records each case's steps and the state it
//! observed afterwards. The test rebuilds each observed state as slices (tasks, tags, dependency
//! edges, and the stacks the `add_stack` steps inserted) and asks the pure functions the same
//! questions the generator asked the services.

mod common;

use std::collections::BTreeMap;

use common::{id, stack_slice, tag, task, tasks_slice, workspace};
use serde_json::{Value, json};
use wg_model::TaskSlice;
use wg_model::slice::StackSlice;
use wg_tasks::deps::{
    check_add_dependency, depends_on_reachable, satisfied_dependencies, unsatisfied_dependencies,
};
use wg_tasks::graph::{GraphEdgeKind, GraphQuery, task_graph};
use wg_tasks::hierarchy::{check_set_parent, is_ancestor, rollup};
use wg_tasks::promote::{PromoteScope, ReadyQuery, promote_ready, ready_tasks};
use wg_types::{Id, StackItemState, StackStatus, TaskStatus};

fn load(name: &str) -> Value {
    let path = format!(
        "{}/../../golden/taskgraph/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn case_names() -> Vec<String> {
    let dir = format!("{}/../../golden/taskgraph", env!("CARGO_MANIFEST_DIR"));
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {dir}: {e}"))
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .and_then(|n| n.strip_suffix(".json"))
                .map(str::to_owned)
        })
        .collect();
    names.sort();
    names
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// Every observation (an object with `dependency_edges`) in the document, with its label.
fn snapshots<'a>(label: &str, value: &'a Value, out: &mut Vec<(String, &'a Value)>) {
    if let Value::Object(map) = value {
        if map.contains_key("dependency_edges") {
            out.push((label.to_owned(), value));
        } else {
            for (k, v) in map {
                snapshots(&format!("{label}/{k}"), v, out);
            }
        }
    }
}

/// A case rebuilt at one observation.
struct World {
    tasks: TaskSlice,
    stacks: StackSlice,
    keys: Vec<String>,
    ids: BTreeMap<String, Id>,
}

impl World {
    fn id(&self, key: &str) -> Id {
        *self
            .ids
            .get(key)
            .unwrap_or_else(|| panic!("unknown key {key}"))
    }

    fn key(&self, id: Id) -> String {
        self.ids
            .iter()
            .find(|(_, v)| **v == id)
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| format!("<unknown {id}>"))
    }

    fn keys_of(&self, tasks: &[&wg_model::Task]) -> Vec<String> {
        tasks.iter().map(|t| t.key.to_string()).collect()
    }
}

fn number(key: &str) -> i64 {
    key.rsplit('-')
        .next()
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("bad key {key}"))
}

/// The tasks of an observation, with `status_override` applied to some keys.
fn build(doc: &Value, snap: &Value, status_override: &BTreeMap<String, TaskStatus>) -> World {
    let titles: Vec<&str> = doc["steps"]
        .as_array()
        .map(|steps| {
            steps
                .iter()
                .filter(|s| s["op"] == "create" && s["outcome"] == "ok")
                .map(|s| text(&s["title"]))
                .collect()
        })
        .unwrap_or_default();
    let observed = snap["tasks"].as_object().cloned().unwrap_or_default();
    let mut keys: Vec<String> = observed.keys().cloned().collect();
    keys.sort_by_key(|k| number(k));
    let ids: BTreeMap<String, Id> = keys
        .iter()
        .map(|k| (k.clone(), id(number(k) as u128)))
        .collect();
    let mut rows = Vec::new();
    let mut tags = Vec::new();
    for key in &keys {
        let state = &observed[key];
        let n = number(key);
        let prefix = key.split('-').next().unwrap_or("TAA");
        let status = status_override.get(key).copied().unwrap_or_else(|| {
            TaskStatus::parse(text(&state["status"])).unwrap_or(TaskStatus::Backlog)
        });
        let parent = state["parent"].as_str().map(|p| ids[p]);
        let mut row = task(prefix, n, status, parent);
        row.title = titles
            .get(usize::try_from(n - 1).unwrap_or(0))
            .map_or_else(String::new, |t| (*t).to_owned());
        if state["archived"] == true {
            row.archived_at = Some(row.created_at);
        }
        for t in state["tags"].as_array().into_iter().flatten() {
            tags.push(tag(row.id, text(t)));
        }
        rows.push(row);
    }
    let deps: Vec<(Id, Id)> = snap["dependency_edges"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| (ids[text(&e[0])], ids[text(&e[1])]))
        .collect();
    let stacks = stack_slice(
        doc["steps"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|s| s["op"] == "add_stack" && s["outcome"] == "ok")
            .map(|s| {
                let items = s["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|item| {
                        (
                            ids[text(&item[0])],
                            StackItemState::parse(text(&item[1]))
                                .unwrap_or(StackItemState::Pending),
                        )
                    })
                    .collect();
                (
                    ids[text(&s["parent"])],
                    StackStatus::parse(text(&s["status"])).unwrap_or(StackStatus::Open),
                    items,
                )
            })
            .collect(),
    );
    World {
        tasks: tasks_slice(rows, tags, deps),
        stacks,
        keys,
        ids,
    }
}

fn graph_json(world: &World, graph: &wg_tasks::graph::TaskGraph) -> Value {
    let name = |id: Id| format!("<task:{}>", world.key(id));
    // The golden vectors fix the edges by (kind, source key, target key, relation): the generator
    // sorts Python's edges because storage order is not part of its contract. `task_graph` returns
    // database order instead, which `graph_edges_follow_database_order` pins on its own.
    let mut edges: Vec<&wg_tasks::graph::GraphEdge> = graph.edges.iter().collect();
    edges.sort_by_key(|e| {
        (
            e.kind,
            world.key(e.source).to_string(),
            world.key(e.target).to_string(),
            e.relation_type.map(|r| r.as_str()),
        )
    });
    json!({
        "nodes": graph.nodes.iter().map(|n| json!({
            "blocked": n.blocked,
            "category": n.category,
            "id": name(n.id),
            "key": n.key,
            "parent_id": n.parent_id.map(name),
            "priority": n.priority.as_str(),
            "project_ids": n.project_ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "repository_ids": n.repository_ids.iter().map(ToString::to_string).collect::<Vec<_>>(),
            "status": n.status.as_str(),
            "tags": n.tags,
            "title": n.title,
        })).collect::<Vec<_>>(),
        "edges": edges.into_iter().map(|e| json!({
            "kind": e.kind.as_str(),
            "relation_type": e.relation_type.map(|r| r.as_str()),
            "source": name(e.source),
            "target": name(e.target),
        })).collect::<Vec<_>>(),
    })
}

fn rollup_json(rollup: Option<wg_tasks::hierarchy::Rollup>) -> Value {
    match rollup {
        None => Value::Null,
        Some(r) => json!({
            "accepted": r.accepted,
            "by_status": r.by_status.iter().map(|(s, n)| (s.to_string(), json!(n))).collect::<serde_json::Map<_, _>>(),
            "progress": r.progress,
            "total_descendants": r.total_descendants,
        }),
    }
}

fn check_snapshot(label: &str, world: &World, snap: &Value) {
    let ctx = |what: &str| format!("{label}: {what}");

    let edges: Vec<(Id, Id)> = snap["dependency_edges"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|e| (world.id(text(&e[0])), world.id(text(&e[1]))))
        .collect();
    let satisfied = satisfied_dependencies(&world.tasks, &world.stacks, &edges);
    for row in snap["satisfied"].as_array().into_iter().flatten() {
        let edge = (world.id(text(&row[0])), world.id(text(&row[1])));
        assert_eq!(
            satisfied.contains(&edge),
            row[2] == true,
            "{}",
            ctx(&format!("satisfied {row}"))
        );
    }

    for key in &world.keys {
        let blockers = unsatisfied_dependencies(&world.tasks, &world.stacks, world.id(key));
        let got: Vec<Value> = world
            .keys_of(&blockers)
            .into_iter()
            .map(Value::from)
            .collect();
        assert_eq!(
            Value::Array(got),
            snap["unsatisfied_blockers"][key],
            "{}",
            ctx(&format!("unsatisfied blockers of {key}"))
        );
        assert_eq!(
            rollup_json(rollup(&world.tasks, world.id(key))),
            snap["rollup"][key],
            "{}",
            ctx(&format!("rollup of {key}"))
        );
        for other in &world.keys {
            let pair = format!("{key}>{other}");
            let path = depends_on_reachable(&world.tasks, world.id(key), world.id(other))
                .map(|p| p.into_iter().map(|i| world.key(i)).collect::<Vec<_>>());
            assert_eq!(
                json!(path),
                snap["reachable_path"][&pair],
                "{}",
                ctx(&format!("reachable {pair}"))
            );
            assert_eq!(
                is_ancestor(&world.tasks, world.id(key), world.id(other)),
                snap["is_ancestor"][&pair] == true,
                "{}",
                ctx(&format!("is_ancestor {pair}"))
            );
        }
    }

    let ready = ready_tasks(
        &world.tasks,
        &world.stacks,
        ReadyQuery {
            workspace_id: workspace(),
            project: None,
            repository: None,
            agent: None,
            limit: 500,
        },
    );
    assert_eq!(
        json!(world.keys_of(&ready)),
        snap["ready"],
        "{}",
        ctx("ready queue")
    );

    let base = GraphQuery {
        workspace_id: workspace(),
        project: None,
        repository: None,
        root: None,
        include_closed: true,
    };
    let all = task_graph(&world.tasks, &world.stacks, &base);
    assert_eq!(
        graph_json(world, &all),
        snap["graph"]["all"],
        "{}",
        ctx("graph all")
    );
    let open = task_graph(
        &world.tasks,
        &world.stacks,
        &GraphQuery {
            include_closed: false,
            ..base
        },
    );
    assert_eq!(
        graph_json(world, &open),
        snap["graph"]["open_only"],
        "{}",
        ctx("graph open only")
    );
    if let Some(root) = world
        .keys
        .iter()
        .find(|k| !snap["rollup"][k.as_str()].is_null())
    {
        let rooted = task_graph(
            &world.tasks,
            &world.stacks,
            &GraphQuery {
                root: Some(world.id(root)),
                ..base
            },
        );
        assert_eq!(
            graph_json(world, &rooted),
            snap["graph"]["root_first_parent"],
            "{}",
            ctx("graph from the first parent")
        );
    }
    assert!(
        all.edges
            .iter()
            .all(|e| matches!(e.kind, GraphEdgeKind::DependsOn | GraphEdgeKind::Parent)),
        "no relations are recorded in these cases"
    );
}

/// The refusals the generator recorded that do not depend on when they happened.
fn check_refusals(doc: &Value, world: &World) {
    let final_state_cases = ["cycle_detection", "parent_rollup_and_hierarchy_cycle"];
    let exact = final_state_cases.contains(&text(&doc["case"]));
    for step in doc["steps"].as_array().into_iter().flatten() {
        let outcome = &step["outcome"];
        if outcome["error"] != "DependencyCycleError" {
            continue;
        }
        let got = match text(&step["op"]) {
            "add_dependency" => {
                let task = text(&step["task"]);
                let dep = text(&step["depends_on"]);
                if task != dep && !exact {
                    continue;
                }
                check_add_dependency(&world.tasks, world.id(task), world.id(dep))
            }
            "set_parent" if exact => check_set_parent(
                &world.tasks,
                world.id(text(&step["task"])),
                world.id(text(&step["parent"])),
            )
            .map(|()| wg_tasks::deps::DependencyAdd::Add),
            _ => continue,
        };
        let err = got
            .err()
            .unwrap_or_else(|| panic!("{step}: expected a cycle"));
        assert_eq!(err.code.as_str(), "dependency_cycle", "{step}");
        assert_eq!(err.message, text(&outcome["message"]), "{step}");
    }
}

/// `dry_run` and `promoted` come from the state before promotion, which the document records as
/// `before` or `before_promotion`; when it does not, it is the `after` state with the promoted
/// tasks put back to backlog.
fn check_promotion(doc: &Value, observations: &Value) {
    let Some(dry) = observations.get("dry_run") else {
        return;
    };
    let promoted: Vec<String> = observations["promoted"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|k| text(k).to_owned())
        .collect();
    assert_eq!(dry, &observations["promoted"], "dry run and real run agree");
    let (before, after) = [("before", "after"), ("before_promotion", "after_promotion")]
        .into_iter()
        .find(|(b, a)| observations.get(*b).is_some() || observations.get(*a).is_some())
        .map(|(b, a)| (observations.get(b), &observations[a]))
        .unwrap_or_else(|| panic!("{}: no promotion snapshots", doc["case"]));
    let world = match before {
        Some(snap) => build(doc, snap, &BTreeMap::new()),
        None => {
            let reset = promoted
                .iter()
                .map(|k| (k.clone(), TaskStatus::Backlog))
                .collect();
            build(doc, after, &reset)
        }
    };
    let got = promote_ready(
        &world.tasks,
        &world.stacks,
        workspace(),
        PromoteScope::default(),
    );
    assert_eq!(
        world.keys_of(&got),
        promoted,
        "{}: promoted keys",
        doc["case"]
    );

    // After promotion exactly the promoted tasks went from backlog to ready.
    let after_world = build(doc, after, &BTreeMap::new());
    for key in &world.keys {
        let was = world
            .tasks
            .task(world.id(key))
            .map(|t| t.status)
            .unwrap_or(TaskStatus::Backlog);
        let now = after_world
            .tasks
            .task(after_world.id(key))
            .map(|t| t.status)
            .unwrap_or(TaskStatus::Backlog);
        if promoted.contains(key) {
            assert_eq!(
                (was, now),
                (TaskStatus::Backlog, TaskStatus::Ready),
                "{key}"
            );
        } else {
            assert_eq!(was, now, "{key} is untouched by promotion");
        }
    }
}

fn check_case(name: &str) {
    let doc = load(name);
    let mut snaps = Vec::new();
    snapshots("", &doc["observations"], &mut snaps);
    assert!(!snaps.is_empty(), "{name}: no observations");
    for (label, snap) in &snaps {
        let world = build(&doc, snap, &BTreeMap::new());
        check_snapshot(&format!("{name}{label}"), &world, snap);
    }
    if let Some((_, last)) = snaps.last() {
        check_refusals(&doc, &build(&doc, last, &BTreeMap::new()));
    }
    check_promotion(&doc, &doc["observations"]);
}

#[test]
fn every_golden_case_matches() {
    let names = case_names();
    assert_eq!(names.len(), 19, "7 curated and 12 generated cases");
    for name in &names {
        check_case(name);
    }
}

/// `test_dependencies_block_and_unblock`: B waits for A until A is accepted, not merely completed.
#[test]
fn dependencies_block_and_unblock() {
    check_case("dependencies_block_and_unblock");
}

/// `test_cycle_detection`: a chain A <- B <- C refuses A -> C and A -> A.
#[test]
fn cycle_detection() {
    check_case("cycle_detection");
}

/// `test_parent_child_rollup_and_hierarchy_cycle`.
#[test]
fn parent_child_rollup_and_hierarchy_cycle() {
    check_case("parent_rollup_and_hierarchy_cycle");
}

/// `test_promote_ready_moves_backlog_whose_dependencies_are_accepted`.
#[test]
fn promote_ready_moves_backlog_whose_dependencies_are_accepted() {
    check_case("promote_moves_accepted_dependencies");
}

/// `test_promote_ready_skips_parent_and_parked_tasks`.
#[test]
fn promote_ready_skips_parent_and_parked_tasks() {
    check_case("promote_skips_parent_and_parked");
}

/// `test_graph`.
#[test]
fn graph() {
    check_case("graph_basic");
}

/// The stack rule of `satisfied_dependencies`.
#[test]
fn stack_sibling_states() {
    check_case("stack_sibling_states");
}
