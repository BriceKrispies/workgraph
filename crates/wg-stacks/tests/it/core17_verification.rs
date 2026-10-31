//! CORE-17: `tests/unit/test_regression.py` ported onto `wg_stacks::regression`, and the
//! verification cases of `tests/integration/test_stack_integration.py` (errored checks on the tip,
//! the base and a probe; base reuse and its refusals; failures on the base; bisection; ejection
//! and rebuild) expressed on the pure decisions of `wg-stacks`.

use std::collections::BTreeMap;
use std::error::Error;

use serde_json::{Value, json};
use wg_stacks::bisect::{
    BisectEntry, Bisection, Entry, ProbeVerdict, StackRow, bisect_entry, prefix_shas,
    probe_failed_checks, probe_verdict, sequence,
};
use wg_stacks::merge::MergeOutcome;
use wg_stacks::queue::{ItemDetail, QueueItem};
use wg_stacks::rebuild::{
    RebuildTarget, Replayed, StackTask, apply_replay, dependents, ejected_task_reason,
    ejection_names, rebuild_plan,
};
use wg_stacks::regression::{BaseOutcomes, CheckOutcome, RunCheck, RunResult, leaf, regressions};
use wg_stacks::verify::{
    BaseVerdict, CachedRun, PassedRow, StackRepoBase, Stage, TipStep, at_bases, baselines,
    cached_outcomes, in_scope, judge_against_base, judge_tip, missing_base_checks, passed_plan,
    ran_with, review_parent_steps, tail, verified_event, verify_job_key, verify_resource_keys,
    with_base_run,
};
use wg_types::{StackItemState as S, TaskStatus as T, VerificationStatus as V};
use wg_verify::{Baseline, CheckSpec};

type TestResult = Result<(), Box<dyn Error>>;

fn check(name: &str, status: V, signatures: &[&str], required: bool) -> RunCheck {
    RunCheck {
        name: name.into(),
        required,
        status,
        repository_id: None,
        failure_signatures: signatures.iter().map(|s| s.to_string()).collect(),
        output_excerpt: String::new(),
    }
}

fn failed(name: &str, signatures: &[&str]) -> RunCheck {
    check(name, V::Failed, signatures, true)
}

fn outcome(status: V, signatures: &[&str]) -> CheckOutcome {
    CheckOutcome {
        status,
        signatures: signatures.iter().map(|s| s.to_string()).collect(),
        run_id: None,
        excerpt: String::new(),
    }
}

fn base(pairs: &[(&str, CheckOutcome)]) -> BaseOutcomes {
    BaseOutcomes::from_pairs(
        pairs
            .iter()
            .map(|(name, o)| (name.to_string(), o.clone()))
            .collect(),
    )
}

fn modes(pairs: &[(&str, Baseline)]) -> BTreeMap<String, Baseline> {
    pairs.iter().map(|(n, b)| (n.to_string(), *b)).collect()
}

fn found(pairs: &[(&str, &[&str])]) -> Vec<(String, Vec<String>)> {
    pairs
        .iter()
        .map(|(n, s)| (n.to_string(), s.iter().map(|x| x.to_string()).collect()))
        .collect()
}

const R: Baseline = Baseline::Regression;
const A: Baseline = Baseline::Absolute;

fn spec(raw: Value) -> Result<CheckSpec, Box<dyn Error>> {
    Ok(CheckSpec::from_json(raw.as_object().ok_or("object")?)?)
}

// ------------------------------------------------------------- test_regression.py

#[test]
fn baseline_defaults_to_absolute_and_rejects_unknown_modes() -> TestResult {
    assert_eq!(
        spec(json!({"name": "suite", "command": ["check"]}))?.baseline,
        A
    );
    assert!(spec(json!({"name": "suite", "command": ["check"], "baseline": "sometimes"})).is_err());
    Ok(())
}

#[test]
fn a_passed_check_is_never_failing() {
    let run = [check("suite", V::Passed, &[], true)];
    assert!(regressions(&run, Some(&base(&[])), &modes(&[("suite", R)])).is_empty());
}

#[test]
fn an_optional_failing_check_is_not_failing() {
    let run = [check("lint", V::Failed, &["test:x"], false)];
    assert!(regressions(&run, None, &modes(&[("lint", A)])).is_empty());
}

#[test]
fn an_absolute_failure_is_failing_whatever_the_base_says() {
    let run = [failed("suite", &["test:a", "test:b"])];
    let base = base(&[("suite", outcome(V::Failed, &["test:a", "test:b"]))]);
    assert_eq!(
        regressions(&run, Some(&base), &modes(&[("suite", A)])),
        found(&[("suite", &["test:a", "test:b"])])
    );
}

#[test]
fn a_regression_failure_already_on_the_base_is_tolerated() {
    let run = [failed("suite", &["test:a"])];
    let base = base(&[("suite", outcome(V::Failed, &["test:a"]))]);
    assert!(regressions(&run, Some(&base), &modes(&[("suite", R)])).is_empty());
}

#[test]
fn a_known_failure_whose_test_moved_to_another_binary_is_tolerated() {
    let run = [failed(
        "suite",
        &["test:spec02::spec02_055::a_device_run", "test:fresh"],
    )];
    let base = base(&[(
        "suite",
        outcome(
            V::Failed,
            &["test:a_device_run", "test:spec02_055::a_device_run"],
        ),
    )]);
    assert_eq!(
        regressions(&run, Some(&base), &modes(&[("suite", R)])),
        found(&[("suite", &["test:fresh"])])
    );
    assert_eq!(leaf("test:a::b::name"), "test:name");
    assert_eq!(leaf("build:E0001"), "build:E0001");
}

#[test]
fn only_the_new_signatures_of_a_regression_failure_are_reported_in_order() {
    let run = [failed("suite", &["test:c", "test:a", "test:b"])];
    let base = base(&[("suite", outcome(V::Failed, &["test:a"]))]);
    assert_eq!(
        regressions(&run, Some(&base), &modes(&[("suite", R)])),
        found(&[("suite", &["test:c", "test:b"])])
    );
}

#[test]
fn a_regression_failure_is_failing_when_the_base_has_no_outcome() {
    let run = [failed("suite", &["test:a"])];
    let want = found(&[("suite", &["test:a"])]);
    assert_eq!(
        regressions(&run, Some(&base(&[])), &modes(&[("suite", R)])),
        want
    );
    assert_eq!(regressions(&run, None, &modes(&[("suite", R)])), want);
}

#[test]
fn a_regression_failure_is_failing_when_the_base_passed() {
    let run = [failed("suite", &["test:a"])];
    let base = base(&[("suite", outcome(V::Passed, &[]))]);
    assert_eq!(
        regressions(&run, Some(&base), &modes(&[("suite", R)])),
        found(&[("suite", &["test:a"])])
    );
}

#[test]
fn a_timeout_is_failing_even_when_the_base_fails_the_same_way() {
    let run = [failed("suite", &["timeout", "test:a"])];
    let base = base(&[("suite", outcome(V::Failed, &["timeout", "test:a"]))]);
    assert_eq!(
        regressions(&run, Some(&base), &modes(&[("suite", R)])),
        found(&[("suite", &["timeout", "test:a"])])
    );
}

#[test]
fn an_unparseable_failure_is_failing_even_when_the_base_fails() {
    let run = [failed("suite", &[])];
    let base = base(&[("suite", outcome(V::Failed, &[]))]);
    assert_eq!(
        regressions(&run, Some(&base), &modes(&[("suite", R)])),
        found(&[("suite", &[])])
    );
}

#[test]
fn checks_are_judged_by_their_own_mode() {
    let run = [
        failed("unit", &["test:a"]),
        failed("integration", &["test:b"]),
        failed("types", &["error:x"]),
    ];
    let base = base(&[
        ("unit", outcome(V::Failed, &["test:a"])),
        ("integration", outcome(V::Failed, &["test:b"])),
    ]);
    let specs = modes(&[("unit", R), ("integration", A), ("types", A)]);
    assert_eq!(
        regressions(&run, Some(&base), &specs),
        found(&[("integration", &["test:b"]), ("types", &["error:x"])])
    );
}

#[test]
fn a_check_missing_from_the_specs_is_judged_absolutely() {
    let run = [failed("suite", &["test:a"])];
    let base = base(&[("suite", outcome(V::Failed, &["test:a"]))]);
    assert_eq!(
        regressions(&run, Some(&base), &modes(&[])),
        found(&[("suite", &["test:a"])])
    );
}

#[test]
fn an_errored_check_is_failing_in_absolute_mode_whatever_the_base_says() {
    let run = [check("suite", V::Error, &[], true)];
    let mut older = outcome(V::Error, &[]);
    older.run_id = Some("older".into());
    let base = base(&[("suite", older)]);
    assert_eq!(
        regressions(&run, Some(&base), &modes(&[("suite", A)])),
        found(&[("suite", &[])])
    );
}

#[test]
fn an_errored_check_is_never_tolerated_in_regression_mode() {
    let run = [check("suite", V::Error, &["test:a"], true)];
    let base = base(&[("suite", outcome(V::Failed, &["test:a"]))]);
    let want = found(&[("suite", &["test:a"])]);
    assert_eq!(
        regressions(&run, Some(&base), &modes(&[("suite", R)])),
        want
    );
    assert_eq!(regressions(&run, None, &modes(&[("suite", R)])), want);
}

#[test]
fn an_errored_optional_check_is_not_failing() {
    let run = [check("lint", V::Error, &[], false)];
    assert!(regressions(&run, None, &modes(&[("lint", A)])).is_empty());
}

#[test]
fn outcome_run_id_does_not_affect_the_judgement() {
    let run = [failed("suite", &["test:a"])];
    let mut older = outcome(V::Failed, &["test:a"]);
    older.run_id = Some("older".into());
    assert!(
        regressions(
            &run,
            Some(&base(&[("suite", older)])),
            &modes(&[("suite", R)])
        )
        .is_empty()
    );
}

// ------------------------------------------------------------- verification decisions

fn run(id: &str, checks: Vec<RunCheck>) -> RunResult {
    let required_passed = checks
        .iter()
        .filter(|c| c.required)
        .all(|c| c.status == V::Passed);
    RunResult {
        id: id.into(),
        required_passed,
        checks,
    }
}

#[test]
fn a_check_that_judges_nothing_on_the_tip_stops_the_stack_without_bisecting() {
    let mut suite = check("suite", V::Error, &[], true);
    suite.output_excerpt = "runner could not start: no interpreter for c.txt\n".into();
    let tip = run("run-tip", vec![suite]);
    let TipStep::Errored(errored) = judge_tip(&tip) else {
        panic!("an errored tip stops the verification");
    };
    assert_eq!(errored.stage, Stage::Tip);
    assert_eq!(errored.checks(), ["suite"]);
    let data = errored.event_data();
    assert_eq!(data["stage"], "tip");
    assert_eq!(data["run_id"], "run-tip");
    assert!(
        data["excerpt"]["suite"]
            .as_str()
            .is_some_and(|t| t.contains("runner could not start"))
    );
    assert_eq!(
        errored.reason(),
        "verification could not judge suite (run run-tip, tip)"
    );
}

#[test]
fn all_passing_full_checks_verify_without_measuring_the_base() {
    let tip = run("run-tip", vec![check("suite", V::Passed, &[], true)]);
    assert_eq!(judge_tip(&tip), TipStep::Passed);
}

#[test]
fn a_failure_the_base_already_has_does_not_block_the_stack() {
    let tip = run("run-tip", vec![failed("suite", &["test:bad_a.txt"])]);
    let TipStep::MeasureBase(failed_names) = judge_tip(&tip) else {
        panic!("a failing tip measures the base");
    };
    assert_eq!(failed_names, [("suite".to_string(), None)]);
    let base = base(&[("suite", outcome(V::Failed, &["test:bad_a.txt"]))]);
    assert_eq!(
        judge_against_base(&tip, &base, &modes(&[("suite", R)])),
        BaseVerdict::Tolerated(found(&[("suite", &["test:bad_a.txt"])]))
    );
}

#[test]
fn an_errored_base_run_stops_the_stack_and_is_never_reused() -> TestResult {
    let tip = run("run-tip", vec![failed("suite", &["test:suite::late"])]);
    let mut blind = outcome(V::Error, &[]);
    blind.run_id = Some("run-base".into());
    blind.excerpt = "runner could not start".into();
    let base = base(&[("suite", blind)]);
    let BaseVerdict::Errored(errored) = judge_against_base(&tip, &base, &modes(&[("suite", R)]))
    else {
        panic!("an errored base stops the verification");
    };
    assert_eq!(errored.stage, Stage::Base);
    assert_eq!(errored.run_id.as_deref(), Some("run-base"));
    assert_eq!(errored.checks(), ["suite"]);

    // The errored result is never taken from the cache.
    let repos = [repo()];
    let specs = spec_map()?;
    let mut cached = cached_run("run-base", "b0");
    cached.checks = vec![check("suite", V::Error, &[], true)];
    let found = cached_outcomes(&[cached], &repos, &[("suite".into(), None)], &specs);
    assert!(found.is_empty());
    Ok(())
}

fn repo() -> StackRepoBase {
    StackRepoBase {
        repository_id: "r1".into(),
        name: "svc".into(),
        base_sha: "b0".into(),
    }
}

fn suite_spec() -> Value {
    json!({"name": "suite", "command": ["check"], "baseline": "regression", "parser": "cargo"})
}

fn spec_map() -> Result<BTreeMap<String, CheckSpec>, Box<dyn Error>> {
    Ok(BTreeMap::from([("suite".to_string(), spec(suite_spec())?)]))
}

fn cached_run(id: &str, sha: &str) -> CachedRun {
    let snapshot = spec(suite_spec())
        .map(|s| {
            let mut entry = s.to_json();
            entry.insert("config_id".into(), json!("cfg-1"));
            entry.insert("config".into(), json!("gate"));
            vec![Value::Object(entry)]
        })
        .unwrap_or_default();
    CachedRun {
        id: id.into(),
        manifest_entries: Some(vec![json!({"repository_id": "r1", "sha": sha})]),
        config_snapshot: snapshot,
        checks: vec![failed("suite", &["test:bad_a.txt"])],
    }
}

#[test]
fn a_regression_at_the_same_base_reuses_the_cached_base_outcome() -> TestResult {
    let repos = [repo()];
    let specs = spec_map()?;
    let failed_names = [("suite".to_string(), None)];
    // Newest first: a run elsewhere, then the run at the base.
    let runs = [cached_run("run-other", "b9"), cached_run("run-base", "b0")];
    let found = cached_outcomes(&runs, &repos, &failed_names, &specs);
    let reused = found.get("suite").ok_or("reused outcome")?;
    assert_eq!(reused.run_id.as_deref(), Some("run-base"));
    assert_eq!(reused.signatures, ["test:bad_a.txt"]);
    assert!(missing_base_checks(&failed_names, &found).is_empty());
    Ok(())
}

#[test]
fn a_cached_base_result_from_an_older_check_definition_is_not_reused() -> TestResult {
    let repos = [repo()];
    let mut current = suite_spec();
    current["env"] = json!({"SUITE_VERSION": "2"});
    let specs = BTreeMap::from([("suite".to_string(), spec(current)?)]);
    let failed_names = [("suite".to_string(), None)];
    let found = cached_outcomes(
        &[cached_run("run-old", "b0")],
        &repos,
        &failed_names,
        &specs,
    );
    assert!(found.is_empty());
    assert_eq!(missing_base_checks(&failed_names, &found), ["suite"]);

    // The new base run supplies the outcome for the check recorded against the same repository.
    let measured = with_base_run(
        found,
        "run-new",
        &[failed("suite", &["test:bad_a.txt"])],
        &["suite".to_string()],
        &failed_names,
    );
    assert_eq!(
        measured.get("suite").and_then(|o| o.run_id.as_deref()),
        Some("run-new")
    );
    Ok(())
}

#[test]
fn a_cached_result_needs_the_same_repository_and_a_manifest_at_the_bases() -> TestResult {
    let repos = [repo()];
    let specs = spec_map()?;
    let mut elsewhere = cached_run("run-a", "b0");
    elsewhere.checks[0].repository_id = Some("r2".into());
    let mut no_manifest = cached_run("run-b", "b0");
    no_manifest.manifest_entries = None;
    let found = cached_outcomes(
        &[elsewhere, no_manifest],
        &repos,
        &[("suite".to_string(), None)],
        &specs,
    );
    assert!(found.is_empty());
    Ok(())
}

#[test]
fn manifests_and_snapshots_are_compared_as_python_compares_them() -> TestResult {
    let repos = [repo()];
    assert!(at_bases(
        &[
            json!({"repository_id": "r1", "sha": "x"}),
            json!({"repository_id": "r1", "sha": "b0"})
        ],
        &repos
    ));
    assert!(!at_bases(
        &[json!({"repository_id": "r2", "sha": "b0"})],
        &repos
    ));
    let current = spec(suite_spec())?;
    let mut entry = current.to_json();
    entry.insert("timeout_seconds".into(), json!(900));
    assert!(ran_with(&[Value::Object(entry.clone())], "suite", &current));
    assert!(!ran_with(&[Value::Object(entry)], "other", &current));
    Ok(())
}

#[test]
fn only_full_checks_of_the_stacks_repositories_are_in_scope() -> TestResult {
    let repos = [repo()];
    assert!(in_scope(
        &repos,
        &spec(json!({"name": "a", "command": ["x"], "tier": "full"}))?
    ));
    assert!(in_scope(
        &repos,
        &spec(json!({"name": "a", "command": ["x"], "repository": "svc"}))?
    ));
    assert!(in_scope(
        &repos,
        &spec(json!({"name": "a", "command": ["x"], "repository": "r1"}))?
    ));
    assert!(!in_scope(
        &repos,
        &spec(json!({"name": "a", "command": ["x"], "repository": "web"}))?
    ));
    assert!(!in_scope(
        &repos,
        &spec(json!({"name": "a", "command": ["x"], "tier": "fast"}))?
    ));
    assert_eq!(baselines(&[spec(suite_spec())?])["suite"], R);
    Ok(())
}

#[test]
fn an_absolute_check_failing_on_the_base_still_ejects_nothing() {
    let tip = [failed("suite", &["test:bad_a.txt", "test:bad_b.txt"])];
    let mut red = outcome(V::Failed, &["test:bad_a.txt"]);
    red.run_id = Some("run-base".into());
    let base = base(&[("suite", red)]);
    assert_eq!(
        bisect_entry(3, "run-tip", &tip, &base, &modes(&[("suite", A)])),
        BisectEntry::FailedOnBase {
            reason: "the base fails",
            failed_checks: vec!["suite".into()],
            run_id: Some("run-base".into()),
        }
    );
    assert_eq!(
        bisect_entry(0, "run-tip", &tip, &base, &modes(&[("suite", R)])),
        BisectEntry::FailedOnBase {
            reason: "no merged change to eject",
            failed_checks: vec!["suite".into()],
            run_id: Some("run-tip".into()),
        }
    );
    assert_eq!(
        bisect_entry(3, "run-tip", &tip, &base, &modes(&[("suite", R)])),
        BisectEntry::Search
    );
}

fn row(n: i64, state: S, merges: Value) -> StackRow {
    StackRow {
        item_id: format!("item-{n}"),
        task_id: format!("task-{n}"),
        title: format!("Change {n}"),
        item: QueueItem {
            key: format!("WG-{n}"),
            number: n,
            merge_order: Some(n),
            state,
            detail: Some(json!({ "merges": merges }).to_string()),
        },
    }
}

fn merge(head: &str, commit: &str) -> Value {
    json!({"svc": {"head": head, "merge_commit": commit}})
}

#[test]
fn bisect_ejects_the_child_that_completes_a_failing_combination() -> TestResult {
    let rows = [
        row(3, S::Merged, merge("h3", "m3")),
        row(1, S::Merged, merge("h1", "m1")),
        row(2, S::Merged, merge("h2", "m2")),
        row(4, S::Pending, json!({})),
    ];
    let entries = sequence(&rows);
    let keys: Vec<&str> = entries.iter().map(|e| e.task_key.as_str()).collect();
    assert_eq!(keys, ["WG-1", "WG-2", "WG-3"]);
    let bases = BTreeMap::from([("svc".to_string(), "b0".to_string())]);
    let specs = modes(&[("combined", A)]);
    let empty = BaseOutcomes::new();

    // Prefixes 0..2 pass; only the third change completes the failing combination.
    let mut search = Bisection::new(entries.len(), "run-tip".to_string());
    let mut probed = Vec::new();
    while let Some(position) = search.next_probe() {
        let shas = prefix_shas(&bases, &entries, position)?;
        let checks = if shas["svc"] == "m3" {
            vec![failed("combined", &["test:combined::a_and_c"])]
        } else {
            vec![check("combined", V::Passed, &[], true)]
        };
        let verdict = probe_verdict(&checks, &empty, &specs);
        probed.push((position, shas["svc"].clone(), probe_failed_checks(&checks)));
        search = search.record(
            position,
            verdict == ProbeVerdict::Passed,
            format!("run-{position}"),
        );
    }
    assert_eq!(
        probed,
        [
            (1, "m1".to_string(), Vec::<String>::new()),
            (2, "m2".to_string(), vec![])
        ]
    );
    let (position, high_run) = search.finish();
    assert_eq!(position, 3);
    assert_eq!(high_run, "run-tip");
    assert_eq!(entries[position - 1].task_key, "WG-3");

    let signatures = found(&[("combined", &["test:combined::a_and_c"])]);
    assert_eq!(
        ejected_task_reason("WG-9", &ejection_names(&signatures)),
        "ejected from stack WG-9: combined"
    );

    // The rebuild replays the remaining merges on the base, in merge order.
    let remaining: Vec<Entry> = entries[..2].to_vec();
    let plan = rebuild_plan(
        &remaining,
        &[RebuildTarget {
            repository_name: "svc".into(),
            integration_branch: "stack/wg-9".into(),
        }],
        &bases,
    )?;
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].base_sha, "b0");
    let heads: Vec<&str> = plan[0].merges.iter().map(|m| m.head.as_str()).collect();
    assert_eq!(heads, ["h1", "h2"]);
    assert_eq!(
        plan[0].merges[0].message,
        "Merge WG-1: Change 1 into stack/wg-9"
    );
    Ok(())
}

#[test]
fn a_probe_that_judges_nothing_stops_the_bisection() {
    let checks = [check("suite", V::Error, &[], true)];
    assert_eq!(
        probe_verdict(&checks, &BaseOutcomes::new(), &modes(&[])),
        ProbeVerdict::Errored(vec!["suite".into()])
    );
}

#[test]
fn a_replayed_merge_updates_the_detail_and_a_conflict_drops_it() {
    let detail = ItemDetail::load(Some(
        r#"{"merges": {"svc": {"head": "h1", "merge_commit": "old"}, "web": {"head": "w1", "merge_commit": "w-old"}}, "conflicts": {"svc": {"x": 1}}}"#,
    ));
    let result = apply_replay(
        detail,
        &[
            Replayed {
                repository_name: "svc".into(),
                head: "h1".into(),
                outcome: MergeOutcome::merged("new"),
            },
            Replayed {
                repository_name: "web".into(),
                head: "w1".into(),
                outcome: MergeOutcome::conflicted("tip", "a.txt\n", "conflict"),
            },
        ],
    );
    assert_eq!(result.detail.merges["svc"]["merge_commit"], "new");
    assert!(!result.detail.merges.contains_key("web"));
    assert!(result.detail.conflicts.is_empty());
    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].repository_name, "web");
}

#[test]
fn dependents_of_an_ejected_child_are_found_through_other_items() {
    let tasks = [
        StackTask {
            task_id: "a".into(),
            number: 1,
        },
        StackTask {
            task_id: "b".into(),
            number: 3,
        },
        StackTask {
            task_id: "c".into(),
            number: 2,
        },
    ];
    let edges = [
        ("b".to_string(), "c".to_string()),
        ("c".to_string(), "a".to_string()),
        ("a".to_string(), "outside".to_string()),
    ];
    assert_eq!(dependents(&tasks, &edges, "a"), ["c", "b"]);
    assert!(dependents(&tasks, &edges, "b").is_empty());
}

#[test]
fn a_passing_verification_verifies_merged_items_and_reviews_their_tasks() {
    let rows = [
        PassedRow {
            task_key: "WG-1".into(),
            state: S::Merged,
            task_status: T::AwaitingVerification,
        },
        PassedRow {
            task_key: "WG-2".into(),
            state: S::Verified,
            task_status: T::AwaitingVerification,
        },
        PassedRow {
            task_key: "WG-3".into(),
            state: S::Ejected,
            task_status: T::Ready,
        },
        PassedRow {
            task_key: "WG-4".into(),
            state: S::Merged,
            task_status: T::AwaitingReview,
        },
    ];
    let plan = passed_plan(&rows);
    assert_eq!(plan.verified, ["WG-1", "WG-4"]);
    assert_eq!(plan.to_review, ["WG-1", "WG-2"]);
    assert_eq!(
        review_parent_steps(T::Backlog),
        [T::Ready, T::Claimed, T::AwaitingReview]
    );
    assert!(review_parent_steps(T::AwaitingReview).is_empty());
    let tips = BTreeMap::from([("svc".to_string(), "t1".to_string())]);
    let data = verified_event(
        Some("run-1"),
        &tips,
        &plan.verified,
        "no new failures against the base",
        Some(&found(&[("suite", &["test:bad_a.txt"])])),
    );
    assert_eq!(data["tolerated"], json!({"suite": ["test:bad_a.txt"]}));
}

#[test]
fn stack_verify_jobs_hold_their_stacks_key_and_the_repository_verify_key() {
    let tips = [
        ("svc".to_string(), "t1".to_string()),
        ("web".to_string(), "t2".to_string()),
    ];
    assert_eq!(
        verify_job_key("s1", &tips, None),
        "stack-verify:s1:svc=t1,web=t2"
    );
    assert_eq!(
        verify_job_key("s1", &tips, Some("e9")),
        "stack-verify:s1:svc=t1,web=t2:e9"
    );
    assert_eq!(
        verify_resource_keys("s1", &["r2".into(), "r1".into()]),
        [
            "exclusive:stack:s1:r1",
            "exclusive:stack:s1:r2",
            "verify:r1",
            "verify:r2"
        ]
    );
}

#[test]
fn an_errored_report_keeps_the_last_twenty_lines() {
    let text: String = (1..=25).map(|i| format!("line {i}\r\n")).collect();
    let kept = tail(&text, 20);
    assert!(kept.starts_with("line 6\nline 7"));
    assert!(kept.ends_with("line 25"));
    assert_eq!(tail("", 20), "");
}
