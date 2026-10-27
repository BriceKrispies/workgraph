//! `stack eject` of a merged child, on a real database and a real temporary git repository: the
//! child is ejected, and the integration branch is rebuilt from the stack's base with the merges
//! of the items still on it (`StackVerifier.eject` then `StackLander.rebuild_without`).

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use wg_model::entity::{Repository, RepositoryCheckout, StackItem, Workspace};
use wg_model::{Decision, Write};
use wg_runtime::{App, human_context};
use wg_stacks_feature::service::{self, reads};
use wg_types::{AccessMode, ErrorCode, StackItemState, Timestamp, WgError};

type TestResult = Result<(), Box<dyn Error>>;

fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("eject_rebuild")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn git(path: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test Author")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_COMMITTER_NAME", "Test Author")
        .env("GIT_COMMITTER_EMAIL", "author@example.com")
        .output()?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Commits `file` with `text` on a new branch off `main`, and returns the branch's head.
fn change(repo: &Path, branch: &str, file: &str, text: &str) -> Result<String, Box<dyn Error>> {
    git(repo, &["checkout", "-q", "-b", branch, "main"])?;
    std::fs::write(repo.join(file), text)?;
    git(repo, &["add", "-A"])?;
    git(repo, &["commit", "-q", "-m", &format!("add {file}")])?;
    let head = git(repo, &["rev-parse", "HEAD"])?;
    git(repo, &["checkout", "-q", "main"])?;
    Ok(head)
}

fn as_json(value: &wg_json::Value) -> Result<serde_json::Value, Box<dyn Error>> {
    let text = wg_json::http_json(value).map_err(|err| format!("{err:?}"))?;
    Ok(serde_json::from_str(&text)?)
}

fn id(slot: u32) -> wg_types::Id {
    wg_model::derive_id(&[7; 32], slot)
}

fn app_with_repository(name: &str) -> Result<(App, PathBuf), Box<dyn Error>> {
    let root = scratch(name)?;
    let repo = root.join("alpha");
    std::fs::create_dir_all(&repo)?;
    git(&repo, &["init", "-q", "-b", "main"])?;
    git(&repo, &["config", "core.autocrlf", "false"])?;
    std::fs::write(repo.join("README.md"), "# alpha\n")?;
    git(&repo, &["add", "-A"])?;
    git(&repo, &["commit", "-q", "-m", "initial commit"])?;
    let mut app = App::open(Some(&root.join("home")))?;
    let mut config = app.config()?;
    config.insert(
        "llm".to_owned(),
        json!({"auto_classify": false, "auto_embed": false}),
    );
    app.save_config(&config)?;
    let at = Timestamp::parse_iso("2026-01-01T00:00:00Z")?;
    let decision = Decision::new(())
        .write(Write::insert(Workspace {
            id: id(0),
            slug: "main".to_owned(),
            name: "Main".to_owned(),
            description: String::new(),
            key_prefix: "WG".to_owned(),
            next_task_number: 1,
            settings: serde_json::Map::new(),
            archived_at: None,
            created_at: at,
            updated_at: at,
        }))
        .write(Write::insert(Repository {
            id: id(1),
            workspace_id: id(0),
            name: "alpha".to_owned(),
            description: String::new(),
            identity_fingerprint: None,
            default_branch: Some("main".to_owned()),
            tracked_branches: rpds::VectorSync::new_sync(),
            access_mode: AccessMode::ReadWrite,
            health_status: "unknown".to_owned(),
            health_detail: serde_json::Map::new(),
            health_checked_at: None,
            is_shallow: false,
            last_scanned_at: None,
            archived_at: None,
            metadata: serde_json::Map::new(),
            created_at: at,
            updated_at: at,
        }))
        .write(Write::insert(RepositoryCheckout {
            id: id(2),
            repository_id: id(1),
            path: repo.display().to_string(),
            kind: "clone".to_owned(),
            is_primary: true,
            access_mode: None,
            available: true,
            last_seen_at: None,
            created_at: at,
            updated_at: at,
        }));
    app.runtime.store.commit_decision(&decision)?;
    Ok((app, repo))
}

// covers: C137
#[test]
fn ejecting_a_merged_child_rebuilds_the_integration_branch_without_it() -> TestResult {
    let (mut app, repo) = app_with_repository("merged_child")?;
    let first_head = change(&repo, "feat-a", "a.txt", "a\n")?;
    let second_head = change(&repo, "feat-b", "b.txt", "b\n")?;
    let context = human_context(None)?;
    wg_tasks_feature::service::task_create(
        &mut app,
        &context,
        Some("main"),
        &json!({"title": "Parent", "repositories": ["alpha"]}),
    )?;
    for title in ["First change", "Second change"] {
        wg_tasks_feature::service::task_create(
            &mut app,
            &context,
            Some("main"),
            &json!({"title": title, "parent": "WG-1", "repositories": ["alpha"], "status": "ready"}),
        )?;
    }
    let created = service::create(&mut app, &context, Some("main"), "WG-1")?;
    let created = as_json(&created)?;
    let stack_id = wg_types::parse_id(created["id"].as_str().ok_or("no stack id")?)?;

    // Both children are merged on the integration branch, in order.
    let heads = [("WG-2", first_head), ("WG-3", second_head)];
    app.runtime.store.write(|session| {
        let items = reads::stack_items(session, stack_id)?;
        for (index, item) in items.into_iter().enumerate() {
            let (_, head) = &heads[index];
            let detail = json!({
                "merges": {"alpha": {"head": head, "merge_commit": head}},
                "conflicts": {},
            });
            session.apply_decision(&Decision::new(()).write(Write::update(StackItem {
                state: StackItemState::Merged,
                merge_order: Some(index as i64 + 1),
                detail: Some(detail.to_string()),
                ..item
            })))?;
        }
        Ok(())
    })?;

    let ejected = service::exec::eject(
        &mut app,
        &context,
        Some("main"),
        "WG-1",
        "WG-2",
        "it breaks the build",
    )?;
    let ejected = as_json(&ejected)?;
    assert_eq!(ejected["status"], "ejected");
    assert_eq!(ejected["task"], "WG-2");

    // The ejected item is out of the stack; the other is still merged, on a rebuilt branch.
    let states = app.runtime.store.read(|session| {
        Ok(reads::stack_items(session, stack_id)?
            .into_iter()
            .map(|item| item.state)
            .collect::<Vec<_>>())
    })?;
    assert_eq!(
        states,
        [StackItemState::Ejected, StackItemState::Merged],
        "item states after the ejection"
    );
    let branch = "wg/stack/wg-1";
    let files = git(&repo, &["ls-tree", "-r", "--name-only", branch])?;
    assert!(files.lines().any(|name| name == "b.txt"), "{files}");
    assert!(!files.lines().any(|name| name == "a.txt"), "{files}");
    let base = git(&repo, &["rev-parse", "main"])?;
    assert!(git(&repo, &["merge-base", "--is-ancestor", &base, branch]).is_ok());
    Ok(())
}

// covers: C137
#[test]
fn ejecting_a_child_that_is_not_merged_is_a_conflict() -> TestResult {
    let (mut app, _repo) = app_with_repository("unmerged_child")?;
    let context = human_context(None)?;
    wg_tasks_feature::service::task_create(
        &mut app,
        &context,
        Some("main"),
        &json!({"title": "Parent", "repositories": ["alpha"]}),
    )?;
    wg_tasks_feature::service::task_create(
        &mut app,
        &context,
        Some("main"),
        &json!({"title": "Only change", "parent": "WG-1", "repositories": ["alpha"], "status": "ready"}),
    )?;
    service::create(&mut app, &context, Some("main"), "WG-1")?;
    let refused: WgError = service::exec::eject(
        &mut app,
        &context,
        Some("main"),
        "WG-1",
        "WG-2",
        "no reason",
    )
    .err()
    .ok_or("an unmerged child must not be ejected")?;
    assert_eq!(refused.code, ErrorCode::Conflict);
    Ok(())
}
