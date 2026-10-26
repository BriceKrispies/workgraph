//! CORE-05: the rename-term plan. Ports `tests/unit/test_rename_term.py` as decisions on values,
//! and adds plan cases for the record kinds the Python service rewrites.

mod common;

use common::id;
use serde_json::{Value, json};
use wg_tasks::rename::{
    ConfigRecord, FileRecord, ProjectRecord, RepositoryRecord, TaskRecord, Term, plan_rename,
};
use wg_types::{ErrorCode, TaskStatus};

fn renamed(text: &str, keep: &[&str]) -> String {
    Term::new("Rho", "Nyn", keep)
        .unwrap_or_else(|e| panic!("term: {e}"))
        .apply(text)
        .0
}

// Plan task CORE-05.
#[test]
fn every_spelling_of_the_word_moves() {
    assert_eq!(renamed("rho Rho RHO", &[]), "nyn Nyn NYN");
    assert_eq!(
        renamed("crates/rho-kernel/src/a.rho", &[]),
        "crates/nyn-kernel/src/a.nyn"
    );
    assert_eq!(
        renamed("rho_kernel RhoKernel RHO_VULKAN_TIER rho.exe", &[]),
        "nyn_kernel NynKernel NYN_VULKAN_TIER nyn.exe"
    );
}

// Plan task CORE-05.
#[test]
fn a_word_that_only_contains_it_is_left_alone() {
    let text = "neighbourhood rhombus Rhodes RHOCAP01 librho";
    assert_eq!(renamed(text, &[]), text);
}

// Plan task CORE-05.
#[test]
fn newline_and_tab_escapes_are_boundaries() {
    assert_eq!(
        renamed(r#""\nrho" "\trho" "xnrho""#, &[]),
        r#""\nnyn" "\tnyn" "xnrho""#
    );
}

// Plan task CORE-05.
#[test]
fn kept_tokens_survive_beside_renamed_ones() {
    assert_eq!(
        renamed(
            "`RHO_CORP` and RHO_TIER read rho.rnd1 beside a.rho",
            &["`RHO_CORP`", "rho.rnd1"]
        ),
        "`RHO_CORP` and NYN_TIER read rho.rnd1 beside a.nyn"
    );
}

// Plan task CORE-05.
#[test]
fn the_count_is_the_number_of_sites() {
    let term = Term::new("Rho", "Nyn", &[]).unwrap_or_else(|e| panic!("term: {e}"));
    assert_eq!(term.apply("rho, Rho and rhombus").1, 2);
}

// Plan task CORE-05.
#[test]
fn json_strings_and_keys_are_renamed_and_nothing_else() {
    let value = json!({
        "command": ["bash", "scripts/rho.sh"],
        "env": {"RHO_TIER": "device"},
        "timeout": 9,
        "ok": true
    });
    let term = Term::new("Rho", "Nyn", &[]).unwrap_or_else(|e| panic!("term: {e}"));
    let (out, sites) = term.deep(&value);
    assert_eq!(
        out,
        json!({
            "command": ["bash", "scripts/nyn.sh"],
            "env": {"NYN_TIER": "device"},
            "timeout": 9,
            "ok": true
        })
    );
    assert_eq!(sites, 2);
}

// Plan task CORE-05.
#[test]
fn a_name_must_be_one_new_ascii_word() {
    for (old, new) in [
        ("rho-lang", "nyn"),
        ("Rho", "n1n"),
        ("Rho", "rho"),
        ("", "Nyn"),
    ] {
        let error = match Term::new(old, new, &[]) {
            Ok(term) => panic!("expected {old}/{new} refused, got {term:?}"),
            Err(error) => error,
        };
        assert_eq!(error.code, ErrorCode::ValidationFailed);
    }
}

fn project() -> ProjectRecord {
    ProjectRecord {
        id: id(10),
        slug: "rho-lang".to_owned(),
        name: "Rho".to_owned(),
        description: "The rho language".to_owned(),
    }
}

fn file(path: &str) -> FileRecord {
    FileRecord {
        path: path.to_owned(),
        anchor: None,
        note: String::new(),
    }
}

fn task(n: u128, status: TaskStatus, archived: bool, tags: &[&str]) -> TaskRecord {
    TaskRecord {
        id: id(n),
        key: format!("WG-{n}"),
        status,
        archived,
        version: 3,
        title: "Rho kernel".to_owned(),
        description: "Fix rho.rs".to_owned(),
        acceptance_criteria: vec!["rho builds".to_owned()],
        files: vec![file("crates/rho/src/lib.rs")],
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
    }
}

fn term() -> Term {
    Term::new("Rho", "Nyn", &[]).unwrap_or_else(|e| panic!("term: {e}"))
}

// Plan task CORE-05.
#[test]
fn open_tasks_have_text_and_files_rewritten_and_closed_tasks_keep_it() {
    let open = task(1, TaskStatus::Ready, false, &["rho", "keep"]);
    let closed = task(2, TaskStatus::Accepted, false, &["rho"]);
    let plan = plan_rename(&term(), &project(), &[], &[], &[open, closed]);

    assert_eq!(plan.tasks.len(), 2);
    let open_change = &plan.tasks[0];
    assert!(!open_change.closed);
    assert_eq!(open_change.title.as_deref(), Some("Nyn kernel"));
    assert_eq!(open_change.description.as_deref(), Some("Fix nyn.rs"));
    assert_eq!(
        open_change.acceptance_criteria.as_deref(),
        Some(&["nyn builds".to_owned()][..])
    );
    assert_eq!(
        open_change
            .files
            .as_ref()
            .map(|files| files[0].path.as_str()),
        Some("crates/nyn/src/lib.rs")
    );
    assert_eq!(
        open_change.tags.as_deref(),
        Some(&["nyn".to_owned(), "keep".to_owned()][..])
    );

    // The closed task only gets its tag renamed; its text is a kept mention.
    let closed_change = &plan.tasks[1];
    assert!(closed_change.closed);
    assert!(closed_change.title.is_none() && closed_change.description.is_none());
    assert_eq!(closed_change.fields, vec![("tags", 1)]);
    assert_eq!(plan.closed_kept, 1);
    assert_eq!(plan.tags_renamed, 2);
}

// Plan task CORE-05.
#[test]
fn archived_tasks_are_closed_even_when_open() {
    let archived = task(3, TaskStatus::Backlog, true, &[]);
    let plan = plan_rename(&term(), &project(), &[], &[], &[archived]);
    assert!(plan.tasks.is_empty());
    assert_eq!(plan.closed_kept, 1);
}

// Plan task CORE-05.
#[test]
fn project_repositories_and_scoped_configs_are_planned() {
    let repo = RepositoryRecord {
        id: id(20),
        name: "rho-core".to_owned(),
        description: String::new(),
    };
    let other = RepositoryRecord {
        id: id(21),
        name: "unrelated".to_owned(),
        description: String::new(),
    };
    let scoped = ConfigRecord {
        name: "gate".to_owned(),
        scope_type: "repository".to_owned(),
        scope_id: id(20),
        checks: json!([{"command": "cargo test", "cwd": "/work/rho"}]),
    };
    let elsewhere = ConfigRecord {
        name: "workspace".to_owned(),
        scope_type: "workspace".to_owned(),
        scope_id: id(99),
        checks: json!([{"command": "rho"}]),
    };
    let plan = plan_rename(
        &term(),
        &project(),
        &[repo, other],
        &[scoped, elsewhere],
        &[],
    );

    assert_eq!(plan.repositories.len(), 1);
    assert_eq!(plan.repositories[0].name.as_deref(), Some("nyn-core"));
    assert_eq!(plan.repositories[0].description, None);

    let project_change = plan
        .project
        .clone()
        .unwrap_or_else(|| panic!("project change"));
    assert_eq!(project_change.new_slug, "nyn-lang");
    assert_eq!(project_change.slug.as_deref(), Some("nyn-lang"));
    assert_eq!(project_change.name.as_deref(), Some("Nyn"));
    assert_eq!(
        project_change.description.as_deref(),
        Some("The nyn language")
    );

    assert_eq!(plan.verification_configs.len(), 1);
    let config = &plan.verification_configs[0];
    assert_eq!(config.name, "gate");
    assert_eq!(config.sites, 1);
    assert_eq!(
        config.checks,
        json!([{"command": "cargo test", "cwd": "/work/nyn"}])
    );
    assert_eq!(
        config.path_rewrites,
        vec![("/work/rho".to_owned(), "/work/nyn".to_owned())]
    );
}

// Plan task CORE-05.
#[test]
fn a_task_with_nothing_to_rename_is_not_planned() {
    let mut quiet = task(4, TaskStatus::Ready, false, &["other"]);
    quiet.title = "Plain".to_owned();
    quiet.description = String::new();
    quiet.acceptance_criteria = Vec::new();
    quiet.files = Vec::new();
    let plan = plan_rename(&term(), &project(), &[], &[], &[quiet]);
    assert!(plan.tasks.is_empty());
    assert_eq!(plan.tags_renamed, 0);
}

// Plan task CORE-05.
#[test]
fn repository_description_only_change_keeps_the_name() {
    let repo = RepositoryRecord {
        id: id(30),
        name: "plain".to_owned(),
        description: "A Rho checkout".to_owned(),
    };
    let plan = plan_rename(&term(), &project(), &[repo], &[], &[]);
    assert_eq!(plan.repositories[0].name, None);
    assert_eq!(
        plan.repositories[0].description.as_deref(),
        Some("A Nyn checkout")
    );
    assert_eq!(plan.repositories[0].description_sites, 1);
}

// Plan task CORE-05.
#[test]
fn path_rewrites_skip_relative_and_unchanged_strings() {
    let before: Value = json!(["relative/rho", "/abs/rho", "/abs/none"]);
    let after: Value = json!(["relative/nyn", "/abs/nyn", "/abs/none"]);
    assert_eq!(
        wg_tasks::rename::path_rewrites(&before, &after),
        vec![("/abs/rho".to_owned(), "/abs/nyn".to_owned())]
    );
}
