//! CORE-18: the landing preconditions of `golden/stackplan/landing_preconditions.json` (GV-21) and
//! the landing plans of `stack_landing.py`: compare-and-swap moves, rollback, deferral, release of
//! unverified children and the refusal text.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;

use serde_json::{Value, json};
use wg_stacks::landing::{
    AfterVerified, Dependent, FfCheck, FfStep, JobRef, LandDecision, LandItem, LandRepo,
    LandRequest, LandState, Refusal, after_verified_outcome, after_verified_request,
    fast_forward_plan, guard_child, guard_defer, integration_busy, land_decision, move_args,
    move_bases_event, release_plan, release_unverified, rollback_args, rollback_order, stale_heads,
    verified_run,
};
use wg_types::{ErrorCode, StackItemState, StackStatus, TaskStatus};

type TestResult = Result<(), Box<dyn Error>>;

fn golden() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/stackplan/landing_preconditions.json");
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn text(value: &Value) -> Result<String, Box<dyn Error>> {
    Ok(value.as_str().ok_or("string")?.to_string())
}

fn state_of(state: &Value) -> Result<LandState, Box<dyn Error>> {
    let items = state["items"]
        .as_array()
        .ok_or("items")?
        .iter()
        .map(|item| {
            Ok(LandItem {
                key: text(&item["key"])?,
                number: item["number"].as_i64().ok_or("number")?,
                merge_order: item["merge_order"].as_i64(),
                state: text(&item["state"])?.parse()?,
                task_status: text(&item["task_status"])?.parse()?,
                detail: item["detail"].as_str().map(str::to_string),
            })
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let repos = state["repos"]
        .as_array()
        .ok_or("repos")?
        .iter()
        .map(|repo| {
            Ok(LandRepo {
                id: text(&repo["id"])?,
                name: text(&repo["name"])?,
                default_branch: repo["default_branch"].as_str().map(str::to_string),
                base_sha: text(&repo["base_sha"])?,
                integrated_tip: text(&repo["integrated_tip"])?,
                default_head: text(&repo["default_head"])?,
            })
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
    let tips = state["tips"]
        .as_object()
        .ok_or("tips")?
        .iter()
        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
        .collect();
    Ok(LandState {
        parent_key: "WG-1".into(),
        stack_id: "stack".into(),
        stack_status: text(&state["stack_status"])?.parse()?,
        parent_status: text(&state["parent_status"])?.parse()?,
        items,
        verified_event: state["verified_event"].as_bool().ok_or("verified_event")?,
        repos,
        tips,
    })
}

#[test]
fn landing_precondition_vectors() -> TestResult {
    let vectors = golden()?;
    let cases = vectors["cases"].as_array().ok_or("cases")?;
    assert!(!cases.is_empty());
    for case in cases {
        let id = text(&case["id"])?;
        let got = match land_decision(&state_of(&case["state"])?) {
            Ok(LandDecision::Land { children }) => json!({"outcome": "land", "children": children}),
            Ok(LandDecision::Deferred { children }) => {
                json!({"outcome": "deferred", "children": children})
            }
            Err(refusal) => match &refusal {
                Refusal::UnverifiedChildrenWithMerges { unfinished, .. } => json!({
                    "outcome": "refused",
                    "error": {"kind": refusal.kind(), "code": refusal.code()},
                    "unfinished": unfinished,
                }),
                _ => json!({
                    "outcome": "refused",
                    "error": {"kind": refusal.kind(), "code": refusal.code()},
                }),
            },
        };
        assert_eq!(got, case["result"], "{id}");
    }
    Ok(())
}

#[test]
fn refusals_carry_codes_and_messages() {
    let moved = Refusal::IntegrationMoved {
        parent: "WG-1".into(),
        repository: "api".into(),
    }
    .to_error();
    assert_eq!(moved.code, ErrorCode::Conflict);
    assert!(moved.message.contains("integration branch of api moved"));
    let none = Refusal::NoDefaultBranch {
        repository: "api".into(),
    }
    .to_error();
    assert_eq!(none.code, ErrorCode::ValidationFailed);
    assert_eq!(none.message, "repository \"api\" has no default branch");
    let status = Refusal::StackNotVerified {
        parent: "WG-1".into(),
        status: StackStatus::Open,
    }
    .to_error();
    assert_eq!(
        status.message,
        "WG-1's stack is open; only a verified stack can land"
    );
}

fn check(name: &str, descends: bool, holder: Option<(&str, bool)>) -> FfCheck {
    FfCheck {
        repository: name.into(),
        default_branch: "main".into(),
        base_sha: "b".into(),
        tip: "t".into(),
        descends,
        holder: holder.map(|(p, c)| (p.to_string(), c)),
    }
}

#[test]
fn fast_forward_uses_checkout_or_compare_and_swap() -> TestResult {
    let plan = fast_forward_plan(&[
        check("a", true, Some(("/co", true))),
        check("b", true, None),
    ])?;
    assert_eq!(
        plan[0].step,
        FfStep::MergeFfOnly {
            holder: "/co".into()
        }
    );
    assert_eq!(move_args(&plan[0]), ["merge", "--ff-only", "t"]);
    assert_eq!(
        move_args(&plan[1]),
        ["update-ref", "refs/heads/main", "t", "b"]
    );
    assert_eq!(rollback_args(&plan[0]), ["reset", "--keep", "b"]);
    assert_eq!(
        rollback_args(&plan[1]),
        ["update-ref", "refs/heads/main", "b", "t"]
    );
    assert_eq!(rollback_order(&plan)[0].repository, "b");
    Ok(())
}

#[test]
fn fast_forward_refuses_before_moving_anything() {
    let not_descended = fast_forward_plan(&[check("a", true, None), check("b", false, None)]);
    assert_eq!(
        not_descended.err().map(|e| e.code),
        Some(ErrorCode::Conflict)
    );
    let dirty = fast_forward_plan(&[check("a", true, Some(("/co", false)))]);
    assert_eq!(
        dirty.err().map(|e| e.code),
        Some(ErrorCode::ValidationFailed)
    );
}

fn repo(name: &str, head: &str, branch: Option<&str>) -> LandRepo {
    LandRepo {
        id: format!("id-{name}"),
        name: name.into(),
        default_branch: branch.map(str::to_string),
        base_sha: "b".into(),
        integrated_tip: "t".into(),
        default_head: head.into(),
    }
}

#[test]
fn stale_heads_need_a_moved_branch_and_every_default() {
    assert_eq!(stale_heads(&[repo("a", "b", Some("main"))]), None);
    assert_eq!(stale_heads(&[repo("a", "m", None)]), None);
    let heads = stale_heads(&[repo("a", "m", Some("main")), repo("b", "b", Some("main"))]);
    assert_eq!(
        heads,
        Some(BTreeMap::from([
            ("a".to_string(), "m".to_string()),
            ("b".to_string(), "b".to_string())
        ]))
    );
}

#[test]
fn move_bases_event_keeps_base_keys_first() {
    let event = move_bases_event(
        "WG-1",
        &BTreeMap::from([("a".to_string(), "b".to_string())]),
        &BTreeMap::from([("a".to_string(), "m".to_string())]),
        &json!({"reason": "r"}),
    );
    assert_eq!(
        event,
        json!({"parent_key": "WG-1", "base_shas": {"a": "b"}, "new_base_shas": {"a": "m"}, "reason": "r"})
    );
}

#[test]
fn request_wins_over_auto_accept() {
    let request = LandRequest {
        mode: "manual".into(),
        reason: "ship".into(),
    };
    assert_eq!(
        after_verified_request(Some(request.clone()), true),
        Some(request)
    );
    let auto = after_verified_request(None, true).map(|r| (r.mode, r.reason));
    assert_eq!(auto, Some(("automated".into(), "stack.auto_accept".into())));
    assert_eq!(after_verified_request(None, false), None);
}

#[test]
fn after_verified_reports_each_outcome() {
    let landed: Result<Option<u8>, _> = Ok(Some(1));
    let deferred: Result<Option<u8>, _> = Ok(None);
    let refused: Result<Option<u8>, _> = Err(Refusal::NoVerificationEvent {
        stack_id: "s".into(),
    }
    .to_error());
    assert_eq!(after_verified_outcome(&landed), AfterVerified::Landed);
    assert_eq!(after_verified_outcome(&deferred), AfterVerified::Deferred);
    assert_eq!(after_verified_outcome(&refused).as_str(), "refused");
}

#[test]
fn only_this_stacks_integration_jobs_block() {
    let job = |kind: &str, status: &str, stack: &str| JobRef {
        kind: kind.into(),
        status: status.into(),
        stack_id: Some(stack.into()),
    };
    let jobs = [
        job("stack_verify", "running", "s1"),
        job("stack_verify", "done", "s2"),
    ];
    assert!(integration_busy(&jobs, "s1"));
    assert!(!integration_busy(&jobs, "s2"));
    assert!(guard_child("WG-2", "s1", true).is_err());
    assert!(guard_child("WG-2", "s1", false).is_ok());
    assert!(guard_defer("s1", true).is_err());
}

#[test]
fn verified_run_needs_an_event() -> TestResult {
    assert!(verified_run("s", None).is_err());
    let event = json!({"run_id": "r1", "tips": {"a": "t"}});
    let (run, tips) = verified_run("s", Some(&event)).map_err(|r| r.to_error())?;
    assert_eq!(run.as_deref(), Some("r1"));
    assert_eq!(tips["a"], "t");
    Ok(())
}

#[test]
fn unverified_child_returns_to_ready_with_a_comment() {
    let release = release_unverified(
        "WG-1",
        "WG-2",
        StackItemState::Conflict,
        TaskStatus::AwaitingReview,
        "s",
    );
    assert_eq!(
        release.ready_reason.as_deref(),
        Some("stack WG-1 landed without it")
    );
    assert!(
        release
            .comment_body
            .contains("workgraph stack add WG-1 WG-2")
    );
    assert_eq!(
        release.comment_data["stack_item_release"]["state"],
        "conflict"
    );
    let claimed = release_unverified(
        "WG-1",
        "WG-2",
        StackItemState::Ejected,
        TaskStatus::Claimed,
        "s",
    );
    assert_eq!(claimed.ready_reason, None);
}

#[test]
fn releasing_a_merged_child_resets_dependents_and_rebuilds() {
    let dependents = [
        Dependent {
            item_id: "i1".into(),
            task_key: "WG-3".into(),
            state: StackItemState::Merged,
        },
        Dependent {
            item_id: "i2".into(),
            task_key: "WG-4".into(),
            state: StackItemState::Ejected,
        },
    ];
    let plan = release_plan("WG-2", "WG-1", StackItemState::Merged, &dependents);
    assert!(plan.rebuild);
    assert_eq!(plan.reset.len(), 1);
    assert_eq!(
        plan.reset[0].detail,
        json!({"merges": {}, "conflicts": {}, "reason": "dependency WG-2 was accepted on its own"})
    );
    let off_branch = release_plan("WG-2", "WG-1", StackItemState::Pending, &dependents);
    assert!(!off_branch.rebuild);
    assert!(off_branch.reset.is_empty());
}
