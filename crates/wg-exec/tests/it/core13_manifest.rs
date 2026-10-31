//! CORE-13: execution manifest JSON matches pydantic's `model_dump_json(indent=2)` and the
//! repository plans, change set plans and capture plans follow `integrations/git/worktrees.py`.

use serde_json::json;
use wg_exec::branch::attempt_branch_name;
use wg_exec::manifest::{
    AttemptRef, DIRTY_FILE_LIMIT, ExecutionManifest, ManifestEntry, PlanError, RepoFacts,
    RepoObservation, RepoPlan, RepoRequest, change_set_plan, effective_target, expected_changes,
    needs_commit_range, plan_capture, plan_repo, with_stack_base, worktree_dir_name,
};
use wg_types::{ChangeSetItemStatus as I, ChangeSetStatus as C};

/// Output of `ExecutionManifest.model_dump_json(indent=2)` for the fixture in `fixture_manifest`.
const PYTHON_MANIFEST_JSON: &str = r#"{
  "task_id": "t1",
  "task_key": "WG-1",
  "attempt_id": "a1",
  "workspace_dir": "C:\\ws",
  "repositories": [
    {
      "repository_id": "r1",
      "repository": "my repo",
      "checkout_path": "C:\\ws\\my_repo",
      "source_checkout_path": "C:\\src",
      "sha": "abc",
      "ref": "wg/x",
      "target_ref": null,
      "access": "read_write",
      "isolation": "worktree",
      "worktree_id": null,
      "branch": "wg/x",
      "base": {
        "source": "stack_base",
        "stack_base_sha": "abc"
      }
    }
  ],
  "expected_changes": [
    "my repo"
  ],
  "verification": [
    {
      "name": "fmt",
      "command": "cargo fmt",
      "repository": null
    }
  ],
  "agent_checks": [],
  "baseline_manifest_id": "m1",
  "change_set_id": null
}"#;

/// Output of `ManifestEntry.model_dump_json()` (compact) for the read-only entry below.
const PYTHON_READ_ONLY_ENTRY: &str = r#"{"repository_id":"r2","repository":"ro","checkout_path":"p","source_checkout_path":"s","sha":null,"ref":null,"target_ref":"main","access":"read_only","isolation":"snapshot","worktree_id":null,"branch":null,"base":null}"#;

fn fixture_manifest() -> ExecutionManifest {
    ExecutionManifest {
        task_id: "t1".into(),
        task_key: "WG-1".into(),
        attempt_id: "a1".into(),
        workspace_dir: r"C:\ws".into(),
        repositories: vec![ManifestEntry {
            repository_id: "r1".into(),
            repository: "my repo".into(),
            checkout_path: r"C:\ws\my_repo".into(),
            source_checkout_path: r"C:\src".into(),
            sha: Some("abc".into()),
            ref_name: Some("wg/x".into()),
            target_ref: None,
            access: "read_write".into(),
            isolation: "worktree".into(),
            worktree_id: None,
            branch: Some("wg/x".into()),
            base: Some(json!({"source": "stack_base", "stack_base_sha": "abc"})),
        }],
        expected_changes: vec!["my repo".into()],
        verification: vec![json!({"name": "fmt", "command": "cargo fmt", "repository": null})],
        agent_checks: vec![],
        baseline_manifest_id: "m1".into(),
        change_set_id: None,
    }
}

fn read_only_entry() -> ManifestEntry {
    ManifestEntry {
        repository_id: "r2".into(),
        repository: "ro".into(),
        checkout_path: "p".into(),
        source_checkout_path: "s".into(),
        sha: None,
        ref_name: None,
        target_ref: Some("main".into()),
        access: "read_only".into(),
        isolation: "snapshot".into(),
        worktree_id: None,
        branch: None,
        base: None,
    }
}

#[test]
fn manifest_pretty_json_matches_python() {
    assert_eq!(fixture_manifest().to_pretty_json(), PYTHON_MANIFEST_JSON);
}

#[test]
fn entry_compact_json_matches_python() {
    let text = serde_json::to_string(&read_only_entry().to_json()).unwrap_or_default();
    assert_eq!(text, PYTHON_READ_ONLY_ENTRY);
}

#[test]
fn entry_round_trips_through_json() {
    let entry = fixture_manifest().repositories.remove(0);
    assert_eq!(ManifestEntry::from_json(&entry.to_json()), Some(entry));
    let ro = read_only_entry();
    assert_eq!(ManifestEntry::from_json(&ro.to_json()), Some(ro));
}

#[test]
fn worktree_dir_names_replace_unsafe_characters() {
    assert_eq!(worktree_dir_name("my repo"), "my_repo");
    assert_eq!(worktree_dir_name("a.b_c-d"), "a.b_c-d");
    assert_eq!(worktree_dir_name("Ärger/x"), "_rger_x");
}

fn facts<'a>(
    access: &'a str,
    isolation: &'a str,
    request: RepoRequest,
    target: Option<&'a str>,
    resolved: Option<&'a str>,
    head: Option<&'a str>,
) -> RepoFacts<'a> {
    RepoFacts {
        repository_id: "r1",
        repository: "repo",
        source_checkout_path: "/src/repo",
        access,
        access_mode: "read_write",
        isolation,
        request,
        target,
        resolved_target: resolved,
        head,
        current_branch: Some("main"),
        worktree_path: "/ws/repo",
        worktree_id: None,
    }
}

const ATTEMPT: AttemptRef<'static> = AttemptRef {
    task_key: "WG-46",
    attempt_number: 1,
    attempt_id: "abcdef0123456789",
};

fn plain_request() -> RepoRequest {
    RepoRequest {
        target_ref: None,
        base: None,
    }
}

#[test]
fn writable_worktree_creates_the_attempt_branch_from_the_resolved_target() {
    let request = RepoRequest {
        target_ref: Some("feature".into()),
        base: Some(json!({"source": "sibling_head"})),
    };
    let f = facts(
        "read_write",
        "worktree",
        request,
        Some("feature"),
        Some("c1"),
        Some("h1"),
    );
    let Ok(RepoPlan::Worktree { entry }) = plan_repo(&f, ATTEMPT) else {
        panic!("expected a worktree plan");
    };
    assert_eq!(entry.sha.as_deref(), Some("c1"));
    assert_eq!(entry.branch.as_deref(), Some("wg/wg-46/attempt-1-abcdef01"));
    assert_eq!(entry.ref_name, entry.branch);
    assert_eq!(entry.isolation, "worktree");
    assert_eq!(entry.target_ref.as_deref(), Some("feature"));
    assert_eq!(entry.checkout_path, "/ws/repo");
    assert_eq!(entry.base, Some(json!({"source": "sibling_head"})));
}

#[test]
fn unresolved_target_falls_back_to_head() {
    let f = facts(
        "read_write",
        "worktree",
        plain_request(),
        Some("gone"),
        None,
        Some("h1"),
    );
    let Ok(RepoPlan::Worktree { entry }) = plan_repo(&f, ATTEMPT) else {
        panic!("expected a worktree plan");
    };
    assert_eq!(entry.sha.as_deref(), Some("h1"));
}

#[test]
fn read_only_request_is_a_detached_snapshot() {
    let f = facts(
        "read_only",
        "worktree",
        plain_request(),
        None,
        None,
        Some("h1"),
    );
    let Ok(RepoPlan::Worktree { entry }) = plan_repo(&f, ATTEMPT) else {
        panic!("expected a worktree plan");
    };
    assert_eq!(entry.isolation, "snapshot");
    assert_eq!(entry.branch, None);
    assert_eq!(entry.ref_name, None);
    assert_eq!(entry.sha.as_deref(), Some("h1"));
    assert_eq!(entry.target_ref, None);
}

#[test]
fn in_place_write_uses_the_source_checkout() {
    let f = facts(
        "read_write",
        "in_place",
        plain_request(),
        None,
        None,
        Some("h1"),
    );
    let Ok(RepoPlan::InPlace { entry }) = plan_repo(&f, ATTEMPT) else {
        panic!("expected an in-place plan");
    };
    assert_eq!(entry.isolation, "in_place");
    assert_eq!(entry.checkout_path, "/src/repo");
    assert_eq!(entry.source_checkout_path, "/src/repo");
    assert_eq!(entry.sha.as_deref(), Some("h1"));
    assert_eq!(entry.ref_name.as_deref(), Some("main"));
    assert_eq!(entry.branch, None);
    assert_eq!(entry.base, None);
}

#[test]
fn write_to_a_read_only_repository_is_refused() {
    let mut f = facts(
        "read_write",
        "worktree",
        plain_request(),
        None,
        None,
        Some("h1"),
    );
    f.access_mode = "read_only";
    assert_eq!(
        plan_repo(&f, ATTEMPT),
        Err(PlanError::ReadOnly {
            repository: "repo".into()
        })
    );
}

#[test]
fn no_target_and_no_head_has_no_base() {
    let f = facts("read_only", "worktree", plain_request(), None, None, None);
    assert_eq!(
        plan_repo(&f, ATTEMPT),
        Err(PlanError::NoCommits {
            repository: "repo".into()
        })
    );
}

#[test]
fn empty_target_counts_as_no_target() {
    let request = RepoRequest {
        target_ref: Some(String::new()),
        base: None,
    };
    assert_eq!(effective_target(&request, Some("main")), None);
    let request = RepoRequest {
        target_ref: None,
        base: None,
    };
    assert_eq!(effective_target(&request, Some("main")), Some("main"));
    assert_eq!(effective_target(&request, None), None);
}

#[test]
fn stack_base_applies_only_without_a_chosen_target() {
    let info = json!({"source": "stack_base"});
    let applied = with_stack_base(plain_request(), Some(("c9".into(), info.clone())));
    assert_eq!(applied.target_ref.as_deref(), Some("c9"));
    assert_eq!(applied.base, Some(info));
    let chosen = RepoRequest {
        target_ref: Some("main".into()),
        base: None,
    };
    let kept = with_stack_base(chosen.clone(), Some(("c9".into(), json!({}))));
    assert_eq!(kept, chosen);
}

#[test]
fn expected_changes_default_to_writable_repositories() {
    let entries = vec![fixture_manifest().repositories.remove(0), read_only_entry()];
    assert_eq!(expected_changes(&[], &entries), vec!["my repo".to_owned()]);
    assert_eq!(
        expected_changes(&["x".to_owned()], &entries),
        vec!["x".to_owned()]
    );
}

#[test]
fn change_set_plan_covers_writable_entries_only() {
    let entries = vec![read_only_entry(), fixture_manifest().repositories.remove(0)];
    let plan = change_set_plan("WG-1", 3, &entries);
    let Some(plan) = plan else {
        panic!("expected a change set");
    };
    assert_eq!(plan.title, "WG-1 attempt 3");
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].repository_id, "r1");
    assert_eq!(plan.items[0].base_sha.as_deref(), Some("abc"));
    assert_eq!(plan.items[0].branch.as_deref(), Some("wg/x"));
    assert_eq!(change_set_plan("WG-1", 3, &[read_only_entry()]), None);
}

fn writable_entry(id: &str, name: &str) -> ManifestEntry {
    ManifestEntry {
        repository_id: id.into(),
        repository: name.into(),
        checkout_path: format!("/ws/{name}"),
        source_checkout_path: format!("/src/{name}"),
        sha: Some("s0".into()),
        ref_name: Some(attempt_branch_name("WG-1", 1, "abcdef01")),
        target_ref: None,
        access: "read_write".into(),
        isolation: "worktree".into(),
        worktree_id: None,
        branch: Some(attempt_branch_name("WG-1", 1, "abcdef01")),
        base: None,
    }
}

fn observed(head: Option<&str>, changed: &[&str], commits: &[&str]) -> RepoObservation {
    RepoObservation {
        head: head.map(str::to_owned),
        changed_files: changed.iter().map(|s| (*s).to_owned()).collect(),
        commits: commits.iter().map(|s| (*s).to_owned()).collect(),
    }
}

#[test]
fn commit_range_is_listed_only_when_the_head_moved() {
    assert!(needs_commit_range(Some("a"), Some("b")));
    assert!(!needs_commit_range(Some("a"), Some("a")));
    assert!(!needs_commit_range(None, Some("b")));
    assert!(!needs_commit_range(Some("a"), None));
}

#[test]
fn capture_commits_each_writable_item_and_applies_the_rollup() {
    let entries = vec![writable_entry("r1", "one"), writable_entry("r2", "two")];
    let plan = plan_capture(
        &entries,
        &[
            observed(Some("h1"), &[], &["c1"]),
            observed(Some("s0"), &[], &[]),
        ],
        true,
    );
    assert_eq!(plan.outcome[0].sha.as_deref(), Some("h1"));
    assert_eq!(plan.outcome[1].sha.as_deref(), Some("s0"));
    assert_eq!(plan.modified_repository_ids, vec!["r1".to_owned()]);
    assert!(plan.violations.is_empty());
    assert!(plan.dirty.is_empty());
    assert_eq!(plan.item_updates[0].status, I::Committed);
    assert_eq!(plan.item_updates[0].detail, None);
    assert_eq!(plan.item_updates[1].status, I::Pending);
    assert_eq!(
        plan.item_updates[1].detail.as_deref(),
        Some("no commits produced")
    );
    assert_eq!(plan.change_set_status, Some(C::PartiallyApplied));
}

#[test]
fn capture_without_commits_leaves_the_change_set_open() {
    let entries = vec![writable_entry("r1", "one")];
    let plan = plan_capture(&entries, &[observed(Some("s0"), &[], &[])], true);
    assert_eq!(plan.change_set_status, Some(C::Open));
    let plan = plan_capture(&entries, &[observed(Some("s0"), &[], &[])], false);
    assert_eq!(plan.change_set_status, None);
}

#[test]
fn capture_applies_applied_when_every_item_committed() {
    let entries = vec![writable_entry("r1", "one")];
    let plan = plan_capture(&entries, &[observed(Some("h1"), &[], &["c1"])], true);
    assert_eq!(plan.change_set_status, Some(C::Applied));
}

#[test]
fn capture_flags_read_only_repositories_that_changed() {
    let mut ro = read_only_entry();
    ro.repository = "ro".into();
    ro.sha = Some("s0".into());
    let plan = plan_capture(
        &[ro.clone(), ro],
        &[
            observed(Some("h1"), &["a.txt"], &["c1", "c2"]),
            observed(Some("s0"), &["b", "c"], &[]),
        ],
        false,
    );
    assert_eq!(
        plan.violations,
        vec![
            wg_exec::manifest::Violation {
                repository: "ro".into(),
                commits: 2,
                uncommitted_files: 1
            },
            wg_exec::manifest::Violation {
                repository: "ro".into(),
                commits: 0,
                uncommitted_files: 2
            },
        ]
    );
    assert!(plan.modified_repository_ids.is_empty());
    assert_eq!(plan.dirty.len(), 2);
}

#[test]
fn capture_truncates_dirty_file_lists() {
    let many: Vec<String> = (0..DIRTY_FILE_LIMIT + 10)
        .map(|i| format!("f{i}"))
        .collect();
    let entries = vec![writable_entry("r1", "one")];
    let obs = vec![RepoObservation {
        head: Some("s0".into()),
        changed_files: many,
        commits: vec![],
    }];
    let plan = plan_capture(&entries, &obs, false);
    assert_eq!(plan.dirty[0].1.len(), DIRTY_FILE_LIMIT);
    assert_eq!(plan.dirty[0].1[0], "f0");
}

#[test]
fn missing_checkout_drops_its_changed_files_and_head() {
    let entries = vec![writable_entry("r1", "one")];
    let plan = plan_capture(&entries, &[observed(None, &["x"], &["c1"])], false);
    assert_eq!(plan.outcome[0].sha, None);
    assert!(plan.dirty.is_empty());
    assert_eq!(plan.item_updates[0].status, I::Pending);
}

#[test]
fn item_statuses_drive_the_change_set_rollup() {
    // The rollup used by capture is the same one the change_set module exposes.
    assert_eq!(wg_exec::change_set::applied_status(&[I::Pending]), C::Open);
}
