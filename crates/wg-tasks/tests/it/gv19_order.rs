//! GV-19, storage order: `task_graph` lists its edges in database order and `rollup` counts
//! statuses in first-seen order, as the Python service does. The golden vectors compare edges
//! sorted, so these tests pin the order the service itself returns.

use crate::common;

use common::{id, stamp, task, tasks_slice, workspace};
use wg_model::entity::TaskRelation;
use wg_model::slice::StackSlice;
use wg_model::{Row, Task, TaskSlice, Write};
use wg_tasks::graph::{GraphEdgeKind, GraphQuery, task_graph};
use wg_tasks::hierarchy::{descendants, rollup};
use wg_types::{Id, RelationType, TaskStatus};

/// A task with an explicit id, so the id order and the number order can disagree.
fn with_id(id_value: u128, number: i64, status: TaskStatus, parent: Option<Id>) -> Task {
    Task {
        id: id(id_value),
        ..task("WG", number, status, parent)
    }
}

fn relation(seq: u128, source: Id, target: Id, kind: RelationType) -> Write {
    Write::Insert(Row::TaskRelation(TaskRelation {
        id: id(0x2_0000 + seq),
        source_task_id: source,
        target_task_id: target,
        relation_type: kind,
        created_by: "system".to_owned(),
        created_at: stamp(0),
    }))
}

// covers: C154
#[test]
fn graph_edges_are_in_database_order_not_sorted_by_key() {
    // Ids run against the number order: t1 has the largest id, t4 the smallest.
    let t1 = with_id(0x30, 1, TaskStatus::Backlog, None);
    let t2 = with_id(0x10, 2, TaskStatus::Backlog, Some(id(0x30)));
    let t3 = with_id(0x20, 3, TaskStatus::Backlog, None);
    let t4 = with_id(0x05, 4, TaskStatus::Backlog, Some(id(0x30)));
    // Dependency rows (task, depended on): read through the (task_id, depends_on_id) index.
    let deps = vec![
        (id(0x20), id(0x30)),
        (id(0x10), id(0x30)),
        (id(0x20), id(0x10)),
    ];
    let base = tasks_slice(vec![t1, t2, t3, t4], Vec::new(), deps);
    let tasks: TaskSlice = base.apply_all(&[
        relation(1, id(0x20), id(0x30), RelationType::SimilarTo),
        relation(2, id(0x10), id(0x05), RelationType::RelatesTo),
    ]);
    let query = GraphQuery {
        workspace_id: workspace(),
        project: None,
        repository: None,
        root: None,
        include_closed: true,
    };
    let graph = task_graph(&tasks, &StackSlice::default(), &query);
    let edges: Vec<(GraphEdgeKind, Id, Id)> = graph
        .edges
        .iter()
        .map(|edge| (edge.kind, edge.source, edge.target))
        .collect();
    assert_eq!(
        edges,
        vec![
            // depends_on rows by (task, depends on): 0x10<-0x30, 0x20<-0x10, 0x20<-0x30.
            (GraphEdgeKind::DependsOn, id(0x30), id(0x10)),
            (GraphEdgeKind::DependsOn, id(0x10), id(0x20)),
            (GraphEdgeKind::DependsOn, id(0x30), id(0x20)),
            // parent edges by child number: t2 (number 2) before t4 (number 4), though t4's id
            // is the smaller.
            (GraphEdgeKind::Parent, id(0x30), id(0x10)),
            (GraphEdgeKind::Parent, id(0x30), id(0x05)),
            // relation rows by (source, target, type): source 0x10 before source 0x20.
            (GraphEdgeKind::Relation, id(0x10), id(0x05)),
            (GraphEdgeKind::Relation, id(0x20), id(0x30)),
        ]
    );
}

// covers: C165
#[test]
fn rollup_counts_statuses_in_first_seen_order_over_a_level_walk() {
    // Root R (number 1) with children c2 (Ready, number 2, the larger id) and c3 (Backlog,
    // number 3). c3 has a child g4 (Accepted, number 4).
    let root = with_id(0x01, 1, TaskStatus::Backlog, None);
    let c2 = with_id(0xF0, 2, TaskStatus::Ready, Some(id(0x01)));
    let c3 = with_id(0x03, 3, TaskStatus::Backlog, Some(id(0x01)));
    let g4 = with_id(0x04, 4, TaskStatus::Accepted, Some(id(0x03)));
    let tasks = tasks_slice(vec![root, c2, c3, g4], Vec::new(), Vec::new());

    // Level by level, siblings in creation order: c2 before c3 although c2's id is larger.
    let order: Vec<i64> = descendants(&tasks, id(0x01))
        .iter()
        .map(|task| task.number)
        .collect();
    assert_eq!(order, vec![2, 3, 4]);

    let found = rollup(&tasks, id(0x01)).expect("the root has children");
    assert_eq!(found.total_descendants, 3);
    assert_eq!(
        found.by_status,
        vec![
            (TaskStatus::Ready, 1),
            (TaskStatus::Backlog, 1),
            (TaskStatus::Accepted, 1),
        ]
    );
    assert_eq!(found.accepted, 1);
    assert!((found.progress - 0.3333).abs() < 1e-12);
}
