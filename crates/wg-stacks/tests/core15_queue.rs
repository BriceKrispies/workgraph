//! CORE-15: the queue order and merge sequence vectors of `golden/stackplan/queue_order.json`, and
//! the due order, settle, merge plan and conflict cases of `tests/integration/test_stacks.py`
//! expressed on the pure decisions of `wg-stacks`.

use std::error::Error;
use std::path::PathBuf;

use serde_json::{Value, json};
use wg_stacks::merge::{
    ItemStep, MergeOutcome, conflict_comment, conflict_record, directory_name, fully_merged,
    merge_args, plan_item, record_merge, start_integrating,
};
use wg_stacks::queue::{
    DueItem, ItemDetail, QueueItem, is_settled, merge_sequence, next_due, ordered_items,
};
use wg_types::{StackItemState as S, StackStatus, TaskPriority};

type TestResult = Result<(), Box<dyn Error>>;

fn golden(name: &str) -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/stackplan")
        .join(format!("{name}.json"));
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn field<'a>(value: &'a Value, name: &str) -> Result<&'a Value, Box<dyn Error>> {
    value
        .get(name)
        .ok_or_else(|| format!("missing {name}").into())
}

#[test]
fn queue_order_vectors() -> TestResult {
    let vectors = golden("queue_order")?;
    let cases = field(&vectors, "cases")?
        .as_array()
        .ok_or("cases is not an array")?;
    assert!(!cases.is_empty());
    for case in cases {
        let id = field(case, "id")?.as_str().ok_or("id")?;
        let mut items = Vec::new();
        for item in field(case, "items")?.as_array().ok_or("items")? {
            items.push(QueueItem {
                key: field(item, "key")?.as_str().ok_or("key")?.to_string(),
                number: field(item, "number")?.as_i64().ok_or("number")?,
                merge_order: field(item, "merge_order")?.as_i64(),
                state: field(item, "state")?.as_str().ok_or("state")?.parse()?,
                detail: field(item, "detail")?.as_str().map(str::to_string),
            });
        }
        let order: Vec<&str> = ordered_items(&items)
            .iter()
            .map(|i| i.key.as_str())
            .collect();
        assert_eq!(json!(order), *field(case, "order")?, "order of {id}");
        assert_eq!(
            json!(merge_sequence(&items)),
            *field(case, "sequence")?,
            "sequence of {id}"
        );
    }
    Ok(())
}

#[test]
fn detail_reads_free_text_as_empty() {
    for text in [None, Some(""), Some("free text"), Some("[1, 2]")] {
        let detail = ItemDetail::load(text);
        assert!(detail.merges.is_empty() && detail.conflicts.is_empty());
    }
    let detail = ItemDetail::load(Some(r#"{"merges": {"alpha": {"head": "h"}}, "note": 1}"#));
    assert_eq!(detail.merges.len(), 1);
    assert_eq!(detail.other["note"], json!(1));
    assert_eq!(detail.to_value()["conflicts"], json!({}));
}

#[test]
fn settled_needs_no_waiting_item_and_one_merged() {
    assert!(is_settled(&[S::Merged, S::Ejected]));
    assert!(is_settled(&[S::Verified, S::Conflict]));
    assert!(!is_settled(&[S::Merged, S::Ready]));
    assert!(!is_settled(&[S::Merged, S::Pending]));
    assert!(!is_settled(&[S::Ejected, S::Conflict]));
    assert!(!is_settled(&[]));
}

fn due(id: &str, state: S, priority: TaskPriority, updated_at: i64, number: i64) -> DueItem {
    DueItem {
        id: id.into(),
        task_id: format!("t-{id}"),
        state,
        priority,
        updated_at,
        number,
    }
}

#[test]
fn next_due_orders_by_depth_priority_readiness_number() {
    let items = [
        due("a", S::Merged, TaskPriority::Low, 0, 1),
        due("b", S::Ready, TaskPriority::Low, 5, 2),
        due("c", S::Ready, TaskPriority::Critical, 9, 3),
        due("d", S::Ready, TaskPriority::Critical, 7, 4),
        due("e", S::Ready, TaskPriority::Low, 1, 5),
    ];
    // e depends on a (merged): depth 1, so it goes after the depth-0 items.
    let edges = [("t-e".to_string(), "t-a".to_string())];
    assert_eq!(next_due(&items, &edges, &[]).as_deref(), Some("d"));
    let processed = ["d".to_string()];
    assert_eq!(next_due(&items, &edges, &processed).as_deref(), Some("c"));
    let processed = ["d".to_string(), "c".to_string()];
    assert_eq!(next_due(&items, &edges, &processed).as_deref(), Some("b"));
    let processed = ["d".to_string(), "c".to_string(), "b".to_string()];
    assert_eq!(next_due(&items, &edges, &processed).as_deref(), Some("e"));
}

#[test]
fn next_due_waits_for_unmerged_prerequisites() {
    let items = [
        due("a", S::Ready, TaskPriority::Medium, 0, 1),
        due("b", S::Ready, TaskPriority::Critical, 0, 2),
    ];
    let edges = [("t-b".to_string(), "t-a".to_string())];
    assert_eq!(next_due(&items, &edges, &[]).as_deref(), Some("a"));
    assert_eq!(next_due(&items, &edges, &["a".to_string()]), None);
    // A prerequisite outside the stack is ignored.
    let outside = [("t-b".to_string(), "t-x".to_string())];
    assert_eq!(next_due(&items[1..], &outside, &[]).as_deref(), Some("b"));
}

#[test]
fn item_steps() {
    assert_eq!(plan_item(false, None, None), ItemStep::MergeUnchanged);
    assert_eq!(plan_item(true, None, None), ItemStep::Skip);
    assert_eq!(plan_item(true, Some("h1"), Some("h1")), ItemStep::Noop);
    assert_eq!(
        plan_item(true, Some("h2"), Some("h1")),
        ItemStep::Merge("h2".into())
    );
    assert_eq!(
        merge_args("h", "m")[..5],
        ["merge", "--no-ff", "--no-edit", "-m", "m"]
    );
}

#[test]
fn conflict_record_and_comment() {
    let outcome = MergeOutcome::conflicted("tip1", "a.txt\n\nb.txt\n", "boom");
    let conflict = conflict_record("alpha", "integration/WG-1", "h9", &outcome);
    assert_eq!(conflict["paths"], json!(["a.txt", "b.txt"]));
    assert_eq!(conflict["git_error"], json!(""));
    let comment = conflict_comment("WG-2", "integration/WG-1", "alpha", &conflict);
    assert!(comment.starts_with("Merge conflict integrating WG-2 into integration/WG-1 (alpha). Conflicting paths: a.txt, b.txt. Integration tip: tip1. Next step: rebase this change onto the integration tip tip1 of integration/WG-1 and resolve the conflicts there."));

    // No paths: the git error is kept and the comment says none were reported.
    let bare = MergeOutcome::conflicted("tip1", "", "  fatal: x ");
    let conflict = conflict_record("alpha", "b", "h", &bare);
    assert_eq!(conflict["git_error"], json!("fatal: x"));
    assert!(conflict_comment("WG-2", "b", "alpha", &conflict).contains("none reported"));
}

#[test]
fn merges_are_recorded_and_checked() {
    let mut detail = ItemDetail::load(Some(r#"{"conflicts": {"alpha": {"x": 1}}}"#));
    assert!(!fully_merged(&detail, &[("alpha".into(), "h".into())]));
    record_merge(&mut detail, "alpha", "h", "m1");
    assert!(detail.conflicts.is_empty());
    assert!(fully_merged(&detail, &[("alpha".into(), "h".into())]));
    assert!(!fully_merged(&detail, &[("alpha".into(), "h2".into())]));
    assert!(fully_merged(&detail, &[]));
}

#[test]
fn small_helpers() {
    assert_eq!(directory_name("my repo/é.rs-1"), "my_repo__.rs-1");
    assert_eq!(
        start_integrating(StackStatus::Verified),
        Some(StackStatus::Integrating)
    );
    assert_eq!(start_integrating(StackStatus::Verifying), None);
}
