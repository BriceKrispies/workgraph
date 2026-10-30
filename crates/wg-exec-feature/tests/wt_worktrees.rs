//! The worktree service's pure rules (WG-3773): the stack a worktree path belongs to, the release
//! decision, and the outputs of recovery and removal.

use serde_json::json;
use wg_exec_feature::spec::worktrees::{
    RecoveryReport, Removal, recovery_report_out, release_decision, removal_out, stack_of,
};
use wg_json::cli_json;
use wg_model::entity::{ManagedWorktree, ResourceLease};
use wg_model::write::{RowKey, Write};
use wg_runtime::spec::context::timestamp_from_unix_nanos;
use wg_types::{Id, WorktreeStatus};

const ROOT: &str = "/data/worktrees/stacks";

fn worktree(id: Id, status: WorktreeStatus) -> Result<ManagedWorktree, String> {
    let now = timestamp_from_unix_nanos(0).map_err(|err| err.to_string())?;
    Ok(ManagedWorktree {
        id,
        workspace_id: Id::from_u128(1),
        repository_id: Id::from_u128(2),
        source_checkout_id: Id::from_u128(3),
        attempt_id: Some(Id::from_u128(4)),
        path: format!("/data/worktrees/{id}/api"),
        branch: "wg/wg-1/attempt-1-00000000".to_owned(),
        base_sha: "0".repeat(40),
        status,
        last_error: None,
        created_at: now,
        updated_at: now,
        removed_at: None,
    })
}

#[test]
fn stack_of_names_the_stack_directory_of_a_stack_worktree_only() -> Result<(), String> {
    let stack = Id::from_u128(9).to_string();
    assert_eq!(
        stack_of(&format!("{ROOT}/{stack}/api"), ROOT),
        Some(stack.clone())
    );
    assert_eq!(stack_of(&format!("{ROOT}/{stack}/a/b"), ROOT), None);
    assert_eq!(stack_of("/data/worktrees/attempt/api", ROOT), None);
    assert_eq!(stack_of(&format!("{ROOT}/api"), ROOT), None);
    Ok(())
}

#[test]
fn release_drops_the_leases_and_releases_the_rows_still_in_use() -> Result<(), String> {
    let now = timestamp_from_unix_nanos(0).map_err(|err| err.to_string())?;
    let lease = ResourceLease {
        resource_key: "checkout:1".to_owned(),
        holder: Id::from_u128(4).to_string(),
        purpose: "in-place write".to_owned(),
        acquired_at: now,
        expires_at: None,
    };
    let active = worktree(Id::from_u128(10), WorktreeStatus::Active)?;
    let released = worktree(Id::from_u128(11), WorktreeStatus::Released)?;
    let decision = release_decision(&[lease], &[active.clone(), released.clone()]);
    assert_eq!(decision.writes.len(), 2);
    assert_eq!(
        decision.writes.first(),
        Some(&Write::delete(RowKey::ResourceLease(
            "checkout:1".to_owned()
        )))
    );
    assert_eq!(
        decision.writes.get(1),
        Some(&Write::update(ManagedWorktree {
            status: WorktreeStatus::Released,
            ..active.clone()
        }))
    );
    let targets: Vec<Id> = decision.reply.iter().map(|target| target.id).collect();
    assert_eq!(targets, vec![active.id, released.id]);
    Ok(())
}

// covers: C178
#[test]
fn recovery_report_lists_the_four_outcomes_in_python_key_order() -> Result<(), String> {
    let report = RecoveryReport {
        activated: vec!["/a".to_owned()],
        released: vec![],
        orphaned: vec!["/b".to_owned()],
        unknown_dirs: vec!["/c/d".to_owned()],
    };
    let text = cli_json(&recovery_report_out(&report)).map_err(|err| format!("{err:?}"))?;
    let parsed: serde_json::Value = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    assert_eq!(
        parsed,
        json!({"activated": ["/a"], "released": [], "orphaned": ["/b"], "unknown_dirs": ["/c/d"]})
    );
    assert!(text.find("activated") < text.find("unknown_dirs"));
    Ok(())
}

#[test]
fn removal_reports_a_reason_only_when_it_was_kept() -> Result<(), String> {
    let id = Id::from_u128(12);
    let parse = |text: String| {
        serde_json::from_str::<serde_json::Value>(&text).map_err(|err| err.to_string())
    };
    let render = |value: &wg_json::Value| cli_json(value).map_err(|err| format!("{err:?}"));
    let removed = parse(render(&removal_out(&Removal::removed(id)))?)?;
    let kept = parse(render(&removal_out(&Removal::refused(
        id,
        "worktree has uncommitted changes",
    )))?)?;
    assert_eq!(
        removed,
        json!({"worktree_id": id.to_string(), "removed": true})
    );
    assert_eq!(
        kept,
        json!({
            "worktree_id": id.to_string(),
            "removed": false,
            "reason": "worktree has uncommitted changes",
        })
    );
    Ok(())
}
