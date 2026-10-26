//! CORE-05: triage-apply validation and plan. Ports the pure cases of `tests/integration/test_triage.py`
//! (the verdict shapes, the planned effects, the skips and the refusals) as decisions on values.
//! The service-level cases (the stored comments, the created rows, the CLI exit codes) belong to
//! the scenario corpus, which runs once the interfaces exist.

mod common;

use common::id;
use serde_json::{Value, json};
use wg_tasks::triage::{
    Action, Decision, Entry, FileSpec, Skipped, Step, TriagePlan, TriageTarget, Verdict,
    comment_body, is_file_spec, parse_entry, parse_file_spec, plan_triage, verdict_list,
};
use wg_types::{ErrorCode, TaskPriority, TaskStatus, WgError};

const SOURCE: &str = "triage batch TEST";

fn target(key: &str, status: TaskStatus, tags: &[&str]) -> TriageTarget {
    TriageTarget {
        key: key.to_owned(),
        status,
        priority: TaskPriority::High,
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        projects: vec![id(5)],
        repositories: vec![id(6)],
    }
}

fn plan(targets: &[TriageTarget], entries: &[Value]) -> TriagePlan {
    plan_triage(entries, Some(SOURCE), |key| {
        targets
            .iter()
            .find(|t| t.key == key)
            .cloned()
            .ok_or_else(|| WgError::new(ErrorCode::NotFound, format!("task {key} not found")))
    })
}

fn rewrite() -> Value {
    json!({
        "title": "Rewritten task",
        "description": "Do the thing.",
        "files": ["src/a.py", "src/b.py"],
        "acceptance_criteria": ["it works"],
        "size": "one-file"
    })
}

fn only_steps(plan: &TriagePlan) -> &[Step] {
    assert_eq!(plan.planned.len(), 1, "one planned verdict: {plan:?}");
    &plan.planned[0].steps
}

// covers: CORE-05
#[test]
fn resolved_comments_tags_and_cancels() {
    let targets = [target("WG-1", TaskStatus::Backlog, &[])];
    let p = plan(
        &targets,
        &[
            json!({"key": "WG-1", "verdict": "resolved", "summary": "Fixed in abc.", "evidence": ["git show abc", "a.py:1"]}),
        ],
    );
    assert!(p.errors.is_empty());
    assert_eq!(p.planned[0].action, Action::Cancelled);
    assert_eq!(p.planned[0].key, "WG-1");
    assert_eq!(
        only_steps(&p),
        [
            Step::Comment(
                "Triage (triage batch TEST): resolved. Fixed in abc.\n\nEvidence:\n- git show abc\n- a.py:1"
                    .to_owned()
            ),
            Step::AddTags(vec!["triage:resolved".to_owned()]),
            Step::Cancel("triage: resolved".to_owned()),
        ]
    );
}

// covers: CORE-05
#[test]
fn obsolete_cancels_with_obsolete_tag() {
    let targets = [target("WG-1", TaskStatus::Backlog, &[])];
    let p = plan(
        &targets,
        &[json!({"key": "WG-1", "verdict": "obsolete", "summary": "Module removed."})],
    );
    assert_eq!(p.planned[0].action, Action::Cancelled);
    assert_eq!(
        only_steps(&p)[0],
        Step::Comment("Triage (triage batch TEST): obsolete. Module removed.".to_owned())
    );
    assert_eq!(
        only_steps(&p)[1],
        Step::AddTags(vec!["triage:obsolete".to_owned()])
    );
}

// covers: CORE-05
#[test]
fn open_creates_follow_ups_and_supersedes_the_original() {
    let targets = [target(
        "WG-1",
        TaskStatus::Backlog,
        &["wg136", "kind:bug", "area:core", "triaged-before"],
    )];
    let entries = [json!({
        "key": "WG-1",
        "verdict": "open",
        "summary": "Half is done.",
        "evidence": ["x.py:3"],
        "rewrites": [
            rewrite(),
            {"title": "Second part", "description": "Rest.", "files": [], "acceptance_criteria": [], "size": "large"}
        ]
    })];
    let p = plan(&targets, &entries);
    assert!(p.errors.is_empty());
    assert_eq!(p.planned[0].action, Action::Superseded);
    let steps = only_steps(&p);

    let Step::CreateFollowUp(first) = &steps[0] else {
        panic!("first step creates a follow-up: {steps:?}")
    };
    assert_eq!(first.title, "Rewritten task");
    assert_eq!(first.priority, TaskPriority::High);
    assert_eq!(first.acceptance_criteria, vec!["it works".to_owned()]);
    let mut tags = first.tags.clone();
    tags.sort();
    assert_eq!(
        tags,
        ["area:core", "scope:one-file", "triaged", "triaged-before"]
    );
    assert_eq!(
        first.description,
        "Do the thing.\n\n**Files:** src/a.py, src/b.py\n\nTriaged from WG-1: Half is done."
    );
    assert_eq!(
        first.files,
        vec![
            FileSpec {
                path: "src/a.py".to_owned(),
                anchor: None
            },
            FileSpec {
                path: "src/b.py".to_owned(),
                anchor: None
            },
        ]
    );
    assert_eq!(first.follows_up, "WG-1");
    assert_eq!(first.projects, vec![id(5)]);
    assert_eq!(first.repositories, vec![id(6)]);

    let Step::CreateFollowUp(second) = &steps[1] else {
        panic!("second step creates a follow-up: {steps:?}")
    };
    assert!(!second.description.contains("**Files:**"));
    assert!(second.tags.contains(&"scope:several-crates".to_owned()));

    // The superseded text is rendered once the created keys are known.
    let Step::CommentSuperseded(note) = &steps[2] else {
        panic!("third step is the superseded comment: {steps:?}")
    };
    let created = ["WG-2".to_owned(), "WG-3".to_owned()];
    assert_eq!(
        note.comment(&created),
        "Triage (triage batch TEST): superseded by WG-2, WG-3. Half is done.\n\nEvidence:\n- x.py:3"
    );
    assert_eq!(
        steps[3],
        Step::AddTags(vec!["triage:superseded".to_owned()])
    );
    let Step::CancelSuperseded(reason) = &steps[4] else {
        panic!("last step cancels: {steps:?}")
    };
    assert_eq!(reason.reason(&created), "triage: superseded by WG-2, WG-3");
    assert_eq!(steps.len(), 5);
}

// covers: CORE-05
#[test]
fn non_cancelling_verdicts_keep_the_status() {
    let targets = [target("WG-1", TaskStatus::Ready, &[])];
    let cases = [
        (
            json!({"key": "WG-1", "verdict": "needs-decision", "summary": "Undecided.", "evidence": ["e1"], "decision": "Pick A or B?"}),
            "Triage (triage batch TEST): needs a decision. Undecided.\n\nDecision: Pick A or B?\n\nEvidence:\n- e1",
            vec![
                "needs-decision".to_owned(),
                "triage:needs-decision".to_owned(),
            ],
        ),
        (
            json!({"key": "WG-1", "verdict": "needs-run", "summary": "Undecided.", "evidence": ["e1"], "command": "cargo test -p rho"}),
            "Triage (triage batch TEST): needs a run to confirm. Undecided.\n\nConfirm with: cargo test -p rho\n\nEvidence:\n- e1",
            vec!["triage:needs-run".to_owned()],
        ),
        (
            json!({"key": "WG-1", "verdict": "unclear", "summary": "Undecided.", "evidence": ["e1"]}),
            "Triage (triage batch TEST): unclear. Undecided.\n\nEvidence:\n- e1",
            vec!["triage:unclear".to_owned()],
        ),
    ];
    for (entry, body, tags) in cases {
        let p = plan(&targets, std::slice::from_ref(&entry));
        assert!(p.errors.is_empty(), "{entry}: {:?}", p.errors);
        assert_eq!(p.planned[0].action, Action::Commented);
        let steps = only_steps(&p);
        assert_eq!(steps[0], Step::Comment(body.to_owned()));
        assert!(
            steps
                .iter()
                .all(|s| !matches!(s, Step::Cancel(_) | Step::CancelSuperseded(_)))
        );
        assert_eq!(steps.last(), Some(&Step::AddTags(tags)));
    }
}

// covers: CORE-05
#[test]
fn a_structured_decision_requests_a_decision_with_normalised_options() {
    let targets = [target("WG-1", TaskStatus::Ready, &[])];
    let p = plan(
        &targets,
        &[json!({
            "key": "WG-1",
            "verdict": "needs-decision",
            "summary": "Ask.",
            "decision": {"question": "Port it?", "options": ["Yes", {"label": "No", "detail": "Keep Python", "recommended": true}]}
        })],
    );
    assert!(p.errors.is_empty(), "{:?}", p.errors);
    let steps = only_steps(&p);
    assert_eq!(
        steps[0],
        Step::Comment(
            "Triage (triage batch TEST): needs a decision. Ask.\n\nDecision: Port it?".to_owned()
        )
    );
    assert_eq!(
        steps[1],
        Step::RequestDecision {
            question: "Port it?".to_owned(),
            options: vec![
                json!({"label": "Yes"}),
                json!({"label": "No", "detail": "Keep Python", "recommended": true}),
            ],
        }
    );
    assert_eq!(steps.len(), 3);
}

// covers: CORE-05
#[test]
fn a_scope_size_maps_legacy_sizes_to_one_scope() {
    let targets = [target("WG-1", TaskStatus::Backlog, &[])];
    for (size, tag) in [
        ("one-crate", "scope:one-crate"),
        ("several-crates", "scope:several-crates"),
        ("medium", "scope:one-crate"),
    ] {
        let mut r = rewrite();
        r["size"] = json!(size);
        let p = plan(
            &targets,
            &[json!({"key": "WG-1", "verdict": "open", "summary": "s", "rewrites": [r]})],
        );
        let Step::CreateFollowUp(follow_up) = &only_steps(&p)[0] else {
            panic!("follow-up expected")
        };
        assert!(
            follow_up.tags.contains(&tag.to_owned()),
            "{size}: {:?}",
            follow_up.tags
        );
    }
}

// covers: CORE-05
#[test]
fn open_work_tracked_elsewhere_duplicates_the_existing_task() {
    let targets = [target("WG-1", TaskStatus::Backlog, &[])];
    let p = plan(
        &targets,
        &[
            json!({"key": "WG-1", "verdict": "open", "summary": "s", "rewrites": [], "duplicate_of": ["WG-7"]}),
        ],
    );
    assert!(p.errors.is_empty());
    assert_eq!(p.planned[0].action, Action::Superseded);
    let steps = only_steps(&p);
    assert_eq!(
        steps[0],
        Step::MarkDuplicate {
            target: "WG-7".to_owned()
        }
    );
    let Step::CommentSuperseded(note) = &steps[1] else {
        panic!("superseded comment expected: {steps:?}")
    };
    assert_eq!(
        note.comment(&[]),
        "Triage (triage batch TEST): superseded by WG-7. s"
    );
    assert_eq!(steps.len(), 4);
}

// covers: CORE-05
#[test]
fn open_without_rewrites_or_duplicates_is_refused() {
    let targets = [target("WG-1", TaskStatus::Backlog, &[])];
    let p = plan(
        &targets,
        &[json!({"key": "WG-1", "verdict": "open", "summary": "s", "rewrites": []})],
    );
    assert_eq!(
        p.errors[0].error,
        "open verdict needs at least one rewrite or a duplicate_of task"
    );
}

// covers: CORE-05
#[test]
fn unknown_rewrite_size_is_refused() {
    let targets = [target("WG-1", TaskStatus::Backlog, &[])];
    let mut r = rewrite();
    r["size"] = json!("huge");
    let p = plan(
        &targets,
        &[json!({"key": "WG-1", "verdict": "open", "summary": "s", "rewrites": [r]})],
    );
    assert_eq!(p.errors[0].error, "rewrite 0 has unknown size 'huge'");
}

// covers: CORE-05
#[test]
fn a_rerun_is_a_skip_naming_the_first_triage_tag() {
    let targets = [target("WG-1", TaskStatus::Cancelled, &["triage:obsolete"])];
    let entries = [json!({"key": "WG-1", "verdict": "obsolete", "summary": "Gone."})];
    let p = plan(&targets, &entries);
    assert!(p.planned.is_empty() && p.errors.is_empty());
    assert_eq!(
        p.skipped,
        vec![Skipped {
            key: "WG-1".to_owned(),
            verdict: Verdict::Obsolete,
            reason: "already triaged (triage:obsolete)".to_owned(),
        }]
    );
}

// covers: CORE-05
#[test]
fn a_decision_replaces_a_pending_verdict_and_clears_its_tags() {
    let targets = [target(
        "WG-1",
        TaskStatus::Ready,
        &["triage:needs-decision", "needs-decision"],
    )];
    let again = plan(
        &targets,
        &[json!({"key": "WG-1", "verdict": "needs-decision", "summary": "Ask."})],
    );
    assert_eq!(
        again.skipped[0].reason,
        "already triaged (triage:needs-decision)"
    );

    let p = plan(
        &targets,
        &[json!({"key": "WG-1", "verdict": "obsolete", "summary": "Port dropped."})],
    );
    assert_eq!(p.planned[0].action, Action::Cancelled);
    let steps = only_steps(&p);
    assert_eq!(
        steps[0],
        Step::RemoveTags(vec![
            "triage:needs-decision".to_owned(),
            "needs-decision".to_owned()
        ])
    );
    assert_eq!(
        steps[steps.len() - 2],
        Step::AddTags(vec!["triage:obsolete".to_owned()])
    );
}

// covers: CORE-05
#[test]
fn a_hand_set_needs_decision_tag_survives_triage() {
    let targets = [target("WG-1", TaskStatus::Backlog, &["needs-decision"])];
    let p = plan(
        &targets,
        &[json!({"key": "WG-1", "verdict": "unclear", "summary": "Not sure."})],
    );
    let steps = only_steps(&p);
    assert!(steps.iter().all(|s| !matches!(s, Step::RemoveTags(_))));
}

// covers: CORE-05
#[test]
fn a_bad_element_does_not_block_the_others() {
    let targets = [
        target("WG-1", TaskStatus::Backlog, &[]),
        target("WG-2", TaskStatus::Accepted, &[]),
    ];
    let good = "WG-1";
    let entries = [
        json!({"verdict": "resolved", "summary": "no key"}),
        json!({"key": good, "verdict": "bogus", "summary": "?"}),
        json!({"key": "WG-9999", "verdict": "unclear", "summary": "?"}),
        json!({"key": good, "verdict": "open", "summary": "no rewrites", "rewrites": []}),
        json!({"key": good, "verdict": "open", "summary": "bad rewrite", "rewrites": [{"description": "d"}]}),
        json!({"key": good, "verdict": "needs-run", "summary": "no command"}),
        json!({"key": "WG-2", "verdict": "obsolete", "summary": "too late"}),
        json!({"key": good, "verdict": "resolved", "summary": "Fixed."}),
    ];
    let p = plan(&targets, &entries);
    assert_eq!(
        p.errors.iter().map(|e| e.index).collect::<Vec<_>>(),
        [0, 1, 2, 3, 4, 5, 6]
    );
    assert_eq!(p.errors[0].error, "missing key");
    assert_eq!(p.errors[0].key, Value::Null);
    assert!(p.errors[1].error.starts_with("unknown verdict"));
    assert!(p.errors[2].error.contains("not found"));
    assert!(p.errors[4].error.contains("rewrite"));
    assert!(p.errors[5].error.contains("command"));
    assert!(
        p.errors[6]
            .error
            .starts_with("cannot move WG-2 from accepted to cancelled")
    );
    assert_eq!(
        p.planned.iter().map(|v| v.key.as_str()).collect::<Vec<_>>(),
        [good]
    );
    assert_eq!(p.planned[0].index, 7);
}

// covers: CORE-05
#[test]
fn a_duplicate_key_in_the_file_is_an_error() {
    let targets = [target("WG-1", TaskStatus::Backlog, &[])];
    let p = plan(
        &targets,
        &[
            json!({"key": "WG-1", "verdict": "unclear", "summary": "a"}),
            json!({"key": "wg-1", "verdict": "unclear", "summary": "b"}),
        ],
    );
    assert_eq!(p.planned.len(), 1);
    assert_eq!(p.errors[0].index, 1);
    assert_eq!(p.errors[0].error, "duplicate key wg-1 in file");
}

// covers: CORE-05
#[test]
fn follow_up_tags_keep_only_the_listed_tags() {
    let targets = [target(
        "WG-1",
        TaskStatus::Backlog,
        &["kind:feature", "wg136", "keep"],
    )];
    let mut r = rewrite();
    r["size"] = Value::Null;
    let p = plan(
        &targets,
        &[json!({"key": "WG-1", "verdict": "open", "summary": "s", "rewrites": [r]})],
    );
    let Step::CreateFollowUp(follow_up) = &only_steps(&p)[0] else {
        panic!("follow-up expected")
    };
    let mut tags = follow_up.tags.clone();
    tags.sort();
    assert_eq!(tags, ["keep", "triaged"]);
}

// covers: CORE-05
#[test]
fn a_triage_file_must_be_an_array() {
    let error = match verdict_list(&json!({"key": "WG-1"})) {
        Ok(items) => panic!("object refused, got {items:?}"),
        Err(error) => error,
    };
    assert_eq!(error.code, ErrorCode::ValidationFailed);
    assert_eq!(
        error.message,
        "triage file must contain a JSON array of verdicts"
    );
    let empty = json!([]);
    assert_eq!(verdict_list(&empty).map(<[Value]>::len).ok(), Some(0));
}

// covers: CORE-05
#[test]
fn a_decision_object_is_validated_like_a_decision_request() {
    let entry = |decision: Value| json!({"key": "WG-1", "verdict": "needs-decision", "summary": "s", "decision": decision});
    let parse = |decision: Value| parse_entry(&entry(decision));

    assert_eq!(
        parse(json!("Pick?")).map(|e| e.decision),
        Ok(Some(Decision::Prose("Pick?".to_owned())))
    );
    assert_eq!(parse(json!(null)).map(|e| e.decision), Ok(None));
    assert_eq!(
        parse(json!(5)).err(),
        Some("decision must be a string or an object with question and options".to_owned())
    );
    assert_eq!(
        parse(json!({"options": []})).err(),
        Some("decision is missing a question".to_owned())
    );
    assert_eq!(
        parse(json!({"question": "q", "options": "x"})).err(),
        Some("decision options must be a list".to_owned())
    );
    assert!(
        parse(json!({"question": "q", "options": [{"label": "a", "colour": "red"}]}))
            .err()
            .is_some_and(|e| e.starts_with("decision options are not valid:"))
    );
    assert!(parse(json!({"question": "q", "options": [{"label": "a", "id": "bad id"}]})).is_err());
    assert!(parse(json!({"question": "q", "options": [{"label": "   "}]})).is_err());
    let too_many: Vec<Value> = (0..27).map(|i| json!(format!("o{i}"))).collect();
    assert!(parse(json!({"question": "q", "options": too_many})).is_err());
    assert!(parse(json!({"question": "q", "options": [{"label": "a", "id": "a-1"}]})).is_ok());
}

// covers: CORE-05
#[test]
fn an_evidence_list_must_be_strings() {
    let error = parse_entry(&json!({"key": "WG-1", "verdict": "unclear", "evidence": "x"})).err();
    assert_eq!(error, Some("evidence must be a list of strings".to_owned()));
}

// covers: CORE-05
#[test]
fn the_open_lists_and_rewrite_shapes_are_checked_in_order() {
    let open = |extra: Value| {
        let mut base = json!({"key": "WG-1", "verdict": "open", "summary": "s"});
        for (name, value) in extra.as_object().cloned().unwrap_or_default() {
            base[name] = value;
        }
        base
    };
    assert_eq!(
        parse_entry(&open(json!({"rewrites": "x"}))).err(),
        Some("rewrites must be a list and duplicate_of a list of task keys".to_owned())
    );
    assert_eq!(
        parse_entry(&open(json!({"rewrites": [5]}))).err(),
        Some("rewrite 0 is not an object".to_owned())
    );
    assert_eq!(
        parse_entry(&open(
            json!({"rewrites": [{"title": "t", "description": "d", "files": [":anchor"]}]})
        ))
        .err(),
        Some("rewrite 0 has a file that is not a path: ':anchor'".to_owned())
    );
    assert_eq!(
        parse_entry(&open(
            json!({"rewrites": [{"title": "t", "description": "d", "files": "a.py"}]})
        ))
        .err(),
        Some("rewrite 0 files and acceptance_criteria must be lists of strings".to_owned())
    );
}

// covers: CORE-05
#[test]
fn a_file_spec_splits_path_and_anchor_after_a_drive_letter() {
    assert_eq!(
        parse_file_spec(r"C:\dev\a.rs:parse"),
        FileSpec {
            path: r"C:\dev\a.rs".to_owned(),
            anchor: Some("parse".to_owned())
        }
    );
    assert_eq!(
        parse_file_spec("a.py:"),
        FileSpec {
            path: "a.py".to_owned(),
            anchor: None
        }
    );
    assert_eq!(
        parse_file_spec("a :b"),
        FileSpec {
            path: "a".to_owned(),
            anchor: Some("b".to_owned())
        }
    );
    assert!(!is_file_spec(":anchor"));
    assert!(!is_file_spec("   "));
    assert!(is_file_spec("src/lib.rs:Thing"));
}

// covers: CORE-05
#[test]
fn a_verdict_entry_keeps_its_key_as_written() {
    let parsed: Entry =
        parse_entry(&json!({"key": " wg-1 ", "verdict": "resolved", "summary": "x"}))
            .unwrap_or_else(|e| panic!("entry: {e}"));
    assert_eq!(parsed.key, " wg-1 ");
    assert_eq!(parsed.verdict, Verdict::Resolved);
    assert_eq!(parsed.verdict.as_str(), "resolved");
    assert_eq!(
        comment_body(None, "unclear", "", &[], &[]),
        "Triage: unclear."
    );
}
