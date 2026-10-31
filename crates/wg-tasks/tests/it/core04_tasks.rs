//! CORE-04: claims, releases, lease expiry, completion, result reports, comments, relations and
//! repository links. Ports the claim, completion and comment cases of
//! `tests/unit/test_tasks_core.py` as decisions on slices.

use crate::common;

use common::{id, stamp, task, tasks_slice, workspace};
use rpds::VectorSync;
use serde_json::{Map, json};
use wg_model::entity::{Repository, TaskRelation, TaskRepository};
use wg_model::{Row, Task, TaskSlice, Write};
use wg_tasks::claims::{
    Caller, ClaimPlan, REASON_LEASE_EXPIRED, REASON_RELEASED, ResultComment, check_version,
    complete_event_data, expired_claims, plan_claim, plan_complete, plan_release,
    plan_report_result,
};
use wg_tasks::comments::{check_attempt_owner, comment_event_data, progress_data};
use wg_tasks::links::{
    RelationAdd, RepoLinkPlan, check_add_relation, plan_link_repository, plan_unlink_repository,
    relation_event_data,
};
use wg_types::{AccessMode, CommentKind, ErrorCode, RelationType, TaskStatus};

const HUMAN: &str = "human:brice";
const AGENT_ONE: &str = "agent:one";

fn holder(mut task: Task, who: &str, expires_at: i64) -> Task {
    task.status = TaskStatus::Claimed;
    task.claimed_by = Some(who.to_owned());
    task.claim_expires_at = Some(stamp(expires_at));
    task
}

fn caller(actor: &str) -> Caller<'_> {
    Caller {
        actor,
        is_admin: false,
    }
}

fn code_of<T: std::fmt::Debug>(result: Result<T, wg_types::WgError>) -> ErrorCode {
    match result {
        Ok(value) => panic!("expected an error, got {value:?}"),
        Err(error) => error.code,
    }
}

#[test]
fn claims_are_exclusive_and_only_the_holder_releases() {
    // test_claims_are_exclusive: agent one claims a ready task; agent two is refused.
    let ready = task("TAA", 1, TaskStatus::Ready, None);
    let plan = plan_claim(&ready, HUMAN, Some("one"), 120, None, stamp(0), &[])
        .unwrap_or_else(|e| panic!("{e}"));
    let ClaimPlan::Take {
        claimant,
        set_assignee,
        release_expired,
        ..
    } = plan
    else {
        panic!("first claim should take the task");
    };
    assert_eq!(claimant, AGENT_ONE);
    assert!(set_assignee);
    assert_eq!(release_expired, None);

    let claimed = holder(ready, AGENT_ONE, 7200);
    assert_eq!(
        code_of(plan_claim(
            &claimed,
            HUMAN,
            Some("two"),
            120,
            None,
            stamp(0),
            &[]
        )),
        ErrorCode::ResourceBusy
    );
    assert_eq!(
        code_of(plan_release(&claimed, caller("agent:two"), None)),
        ErrorCode::PermissionDenied
    );
    let released =
        plan_release(&claimed, caller(AGENT_ONE), None).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(released.reason, REASON_RELEASED);
    assert_eq!(released.previous_claimant.as_deref(), Some(AGENT_ONE));
}

#[test]
fn the_holder_renews_its_lease_and_an_expired_claim_is_retaken() {
    let claimed = holder(task("TAA", 1, TaskStatus::Claimed, None), AGENT_ONE, 100);
    // Same claimant while the lease is live: renewal.
    let renew = plan_claim(&claimed, HUMAN, Some("one"), 10, None, stamp(50), &[])
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        renew,
        ClaimPlan::Renew {
            expires_at: stamp(50 + 600)
        }
    );
    // Another claimant after the lease ended: the expired claim is released first.
    let retake = plan_claim(&claimed, HUMAN, Some("two"), 10, None, stamp(200), &[])
        .unwrap_or_else(|e| panic!("{e}"));
    match retake {
        ClaimPlan::Take {
            claimant,
            release_expired,
            ..
        } => {
            assert_eq!(claimant, "agent:two");
            assert_eq!(release_expired.as_deref(), Some(AGENT_ONE));
        }
        ClaimPlan::Renew { .. } => panic!("another claimant cannot renew"),
    }
}

#[test]
fn claim_refuses_blocked_and_wrong_status_tasks_and_stale_versions() {
    let blocker = task("TAA", 2, TaskStatus::Ready, None);
    let ready = task("TAA", 1, TaskStatus::Ready, None);
    assert_eq!(
        code_of(plan_claim(
            &ready,
            HUMAN,
            None,
            120,
            None,
            stamp(0),
            &[&blocker]
        )),
        ErrorCode::InvalidTransition
    );
    let backlog = task("TAA", 3, TaskStatus::Backlog, None);
    assert_eq!(
        code_of(plan_claim(&backlog, HUMAN, None, 120, None, stamp(0), &[])),
        ErrorCode::InvalidTransition
    );
    assert_eq!(
        code_of(plan_claim(&ready, HUMAN, None, 120, Some(9), stamp(0), &[])),
        ErrorCode::VersionConflict
    );
    assert!(check_version(&ready, Some(ready.version)).is_ok());
    assert_eq!(
        code_of(plan_claim(&ready, HUMAN, None, 0, None, stamp(0), &[])),
        ErrorCode::ValidationFailed
    );
}

#[test]
fn expired_claims_are_listed_in_number_order() {
    let early = holder(task("TAA", 5, TaskStatus::Claimed, None), AGENT_ONE, 10);
    let late = holder(task("TAA", 2, TaskStatus::Claimed, None), AGENT_ONE, 20);
    let live = holder(task("TAA", 1, TaskStatus::Claimed, None), AGENT_ONE, 9999);
    let tasks = tasks_slice(vec![early, late, live], vec![], vec![]);
    let keys: Vec<String> = expired_claims(&tasks, stamp(100))
        .iter()
        .map(|t| t.key.to_string())
        .collect();
    assert_eq!(keys, vec!["TAA-2", "TAA-5"]);
    assert_eq!(REASON_LEASE_EXPIRED, "claim lease expired");
}

#[test]
fn agent_completion_moves_to_review_or_verification_never_accepted() {
    let running = holder(task("TAA", 1, TaskStatus::Running, None), AGENT_ONE, 9999);
    let plan = plan_complete(&running, caller(AGENT_ONE), false).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(plan.target, TaskStatus::AwaitingReview);
    let checked =
        plan_complete(&running, caller(AGENT_ONE), true).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(checked.target, TaskStatus::AwaitingVerification);
    assert_eq!(
        complete_event_data(&checked, "all good"),
        json!({
            "summary": "all good",
            "next_status": "awaiting_verification",
            "verification_configured": true,
        })
    );
    // A system actor may complete any claim; a stranger may not.
    assert!(plan_complete(&running, caller("system:scheduler"), false).is_ok());
    assert_eq!(
        code_of(plan_complete(&running, caller("agent:two"), false)),
        ErrorCode::PermissionDenied
    );
    let ready = task("TAA", 2, TaskStatus::Ready, None);
    assert_eq!(
        code_of(plan_complete(&ready, caller(AGENT_ONE), false)),
        ErrorCode::InvalidTransition
    );
}

#[test]
fn result_reports_complete_claimed_work_or_record_a_comment() {
    let running = holder(task("TAA", 1, TaskStatus::Running, None), AGENT_ONE, 9999);
    let success = plan_report_result(&running, caller(AGENT_ONE), true, "", false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(success.completion.is_some());
    assert_eq!(success.comment, None);
    assert!(!success.fail);

    let failure = plan_report_result(&running, caller(AGENT_ONE), false, "", false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert!(failure.fail);
    assert_eq!(
        failure.comment,
        Some(ResultComment {
            body: "attempt failed".to_owned(),
            failed: true,
        })
    );

    let backlog = task("TAA", 2, TaskStatus::Backlog, None);
    let recorded = plan_report_result(&backlog, caller(AGENT_ONE), true, "", false)
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(recorded.completion, None);
    assert!(!recorded.fail);
    assert_eq!(
        recorded.comment.map(|c| c.body),
        Some("succeeded".to_owned())
    );
}

#[test]
fn comments_and_progress_payloads() {
    let mut extra = Map::new();
    extra.insert("kind".to_owned(), json!("overridden"));
    let payload = comment_event_data(id(7), CommentKind::Progress, "halfway", &extra);
    assert_eq!(payload["excerpt"], json!("halfway"));
    assert_eq!(payload["kind"], json!("overridden"));

    let data = progress_data(&Map::new(), Some(50.0));
    assert_eq!(data.get("percent"), Some(&json!(50.0)));
    assert!(progress_data(&Map::new(), None).is_empty());

    let task_id = id(1);
    assert!(check_attempt_owner(task_id, id(1)).is_ok());
    assert_eq!(
        code_of(check_attempt_owner(task_id, id(2))),
        ErrorCode::ValidationFailed
    );
}

fn relation_slice() -> TaskSlice {
    tasks_slice(
        vec![
            task("TAA", 1, TaskStatus::Ready, None),
            task("TAA", 2, TaskStatus::Ready, None),
        ],
        vec![],
        vec![],
    )
}

#[test]
fn relations_refuse_self_and_report_existing_rows() {
    let tasks = relation_slice();
    assert_eq!(
        code_of(check_add_relation(
            &tasks,
            id(1),
            id(1),
            RelationType::RelatesTo
        )),
        ErrorCode::ValidationFailed
    );
    assert_eq!(
        check_add_relation(&tasks, id(1), id(2), RelationType::RelatesTo).ok(),
        Some(RelationAdd::Add)
    );
    let relation = TaskRelation {
        id: id(0x9000),
        source_task_id: id(1),
        target_task_id: id(2),
        relation_type: RelationType::Duplicates,
        created_by: HUMAN.to_owned(),
        created_at: stamp(0),
    };
    let with_row = tasks.apply(&Write::Insert(Row::TaskRelation(relation)));
    assert_eq!(
        check_add_relation(&with_row, id(1), id(2), RelationType::Duplicates).ok(),
        Some(RelationAdd::Exists(id(0x9000)))
    );
    assert_eq!(
        check_add_relation(&with_row, id(1), id(2), RelationType::RelatesTo).ok(),
        Some(RelationAdd::Add)
    );
    let target = tasks.task(id(2)).unwrap_or_else(|| panic!("task 2"));
    assert_eq!(
        relation_event_data(target, RelationType::RelatesTo)["target_key"],
        json!("TAA-2")
    );
}

fn repository(access: AccessMode) -> Repository {
    Repository {
        id: id(0xAB),
        workspace_id: workspace(),
        name: "repo".to_owned(),
        description: String::new(),
        identity_fingerprint: None,
        default_branch: None,
        tracked_branches: VectorSync::new_sync(),
        access_mode: access,
        health_status: "unknown".to_owned(),
        health_detail: Map::new(),
        health_checked_at: None,
        is_shallow: false,
        last_scanned_at: None,
        archived_at: None,
        metadata: Map::new(),
        created_at: stamp(0),
        updated_at: stamp(0),
    }
}

fn link(access: &str) -> TaskRepository {
    TaskRepository {
        task_id: id(1),
        repository_id: id(0xAB),
        access: access.to_owned(),
        created_at: stamp(0),
    }
}

#[test]
fn repository_links_follow_the_repository_access_mode() {
    let t = task("TAA", 1, TaskStatus::Ready, None);
    let read_write = repository(AccessMode::ReadWrite);
    let read_only = repository(AccessMode::ReadOnly);

    // No access given: the repository's own mode is used.
    assert_eq!(
        plan_link_repository(&t, &read_write, None, None).ok(),
        Some(RepoLinkPlan::Insert {
            access: "read_write"
        })
    );
    // A read-write link to a read-only repository is refused.
    assert_eq!(
        code_of(plan_link_repository(
            &t,
            &read_only,
            Some("read_write"),
            None
        )),
        ErrorCode::ValidationFailed
    );
    assert_eq!(
        code_of(plan_link_repository(&t, &read_write, Some("maybe"), None)),
        ErrorCode::ValidationFailed
    );
    // Existing link: same access is unchanged, different access is an update.
    assert_eq!(
        plan_link_repository(&t, &read_write, Some("read_only"), Some(&link("read_only"))).ok(),
        Some(RepoLinkPlan::Unchanged)
    );
    assert_eq!(
        plan_link_repository(
            &t,
            &read_write,
            Some("read_only"),
            Some(&link("read_write"))
        )
        .ok(),
        Some(RepoLinkPlan::Update {
            access: "read_only"
        })
    );
    assert!(plan_unlink_repository(&read_write, false).is_none());
    assert_eq!(
        plan_unlink_repository(&read_write, true).map(|v| v["repository"].clone()),
        Some(json!("repo"))
    );
}
