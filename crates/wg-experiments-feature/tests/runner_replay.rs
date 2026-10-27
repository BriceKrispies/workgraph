//! `trial show`, `replay scan`, `replay create`, `trial start` and `trial grade` end to end
//! (C075, C076, C081, C082, C083): a real Workgraph home, a temporary git repository with one
//! replayable commit, and scripted setup and oracle commands (`git` itself, which every machine
//! running the suite has).

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

use rpds::VectorSync;
use serde_json::{Value as Json, json};
use wg_experiments_feature::service::replay::{CreateOptions, ScanOptions};
use wg_experiments_feature::service::{replay, runner, trials};
use wg_experiments_feature::spec::trial::AddInput;
use wg_json::{Value, http_json};
use wg_model::entity::{Experiment, Repository, RepositoryCheckout};
use wg_model::{Decision, Write, derive_id};
use wg_runtime::{App, RequestContext, human_context};
use wg_types::{AccessMode, ErrorCode, Id, Timestamp};

type TestResult = Result<(), Box<dyn Error>>;

fn entropy(seed: u8) -> [u8; 32] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

fn id(n: u32) -> Id {
    derive_id(&entropy(21), n)
}

fn at() -> Result<Timestamp, Box<dyn Error>> {
    Ok(Timestamp::from_components(2026, 10, 25, 12, 0, 0, 0)?)
}

fn object(value: Json) -> serde_json::Map<String, Json> {
    match value {
        Json::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

/// A home with the workspace `main`, and the context the checks act under.
struct Home {
    app: App,
    ctx: RequestContext,
    dir: PathBuf,
}

fn home(name: &str) -> Result<Home, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("runner_replay")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let mut app = App::open(Some(&dir.join("home")))?;
    let ctx = human_context(Some("tester"))?;
    let dump =
        object(json!({"slug": "main", "name": "Main", "description": "", "key_prefix": "WG"}));
    app.runtime.decide(
        &ctx,
        |session| session.registry_world(None),
        |slice, stamp| wg_registry::workspaces::create(slice, stamp, &dump),
    )?;
    let mut config = app.config()?;
    config.insert("default_workspace".to_owned(), json!("main"));
    app.save_config(&config)?;
    Ok(Home { app, ctx, dir })
}

fn git(dir: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output()?;
    if !output.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&output.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn write(dir: &Path, path: &str, text: &str) -> TestResult {
    let full = dir.join(path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(full, text)?;
    Ok(())
}

/// A repository whose second commit changes a source file and a test file together.
fn source_repo(root: &Path) -> Result<(PathBuf, String), Box<dyn Error>> {
    let repo = root.join("source");
    std::fs::create_dir_all(&repo)?;
    git(&repo, &["init", "-q", "-b", "main"])?;
    git(&repo, &["config", "user.name", "tester"])?;
    git(&repo, &["config", "user.email", "tester@example.com"])?;
    git(&repo, &["config", "core.autocrlf", "false"])?;
    write(
        &repo,
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )?;
    write(&repo, "src/lib.rs", "pub fn one() -> i32 {\n    1\n}\n")?;
    write(&repo, "tests/it.rs", "// no tests yet\n")?;
    git(&repo, &["add", "-A"])?;
    git(&repo, &["commit", "-q", "-m", "Start the crate"])?;
    write(
        &repo,
        "src/lib.rs",
        "pub fn one() -> i32 {\n    1\n}\n\npub fn two() -> i32 {\n    2\n}\n",
    )?;
    write(
        &repo,
        "tests/it.rs",
        "// no tests yet\nfn two_is_two() {\n    assert_eq!(2, 2);\n}\n",
    )?;
    git(&repo, &["add", "-A"])?;
    git(&repo, &["commit", "-q", "-m", "Add two\n\nTwo joins one."])?;
    let sha = git(&repo, &["rev-parse", "HEAD"])?;
    Ok((repo, sha))
}

impl Home {
    fn scope(&mut self) -> Result<Id, Box<dyn Error>> {
        Ok(self.app.workspace_or_default(Some("main"))?)
    }

    fn commit(&mut self, write: Write) -> TestResult {
        self.app
            .runtime
            .store
            .commit_decision(&Decision::new(()).write(write))?;
        Ok(())
    }

    /// Registers `repo` as the repository `demo` with one available primary checkout.
    fn register(&mut self, repo: &Path) -> TestResult {
        let scope = self.scope()?;
        self.commit(Write::insert(Repository {
            id: id(1),
            workspace_id: scope,
            name: "demo".into(),
            description: String::new(),
            identity_fingerprint: None,
            default_branch: Some("main".into()),
            tracked_branches: VectorSync::new_sync(),
            access_mode: AccessMode::ReadWrite,
            health_status: "ok".into(),
            health_detail: Default::default(),
            health_checked_at: None,
            is_shallow: false,
            last_scanned_at: None,
            archived_at: None,
            metadata: Default::default(),
            created_at: at()?,
            updated_at: at()?,
        }))?;
        self.commit(Write::insert(RepositoryCheckout {
            id: id(2),
            repository_id: id(1),
            path: repo.display().to_string(),
            kind: "clone".into(),
            is_primary: true,
            access_mode: None,
            available: true,
            last_seen_at: None,
            created_at: at()?,
            updated_at: at()?,
        }))
    }

    /// A running experiment `exp` whose protocol clones the base, runs a scripted setup and
    /// grades with a scripted oracle.
    fn experiment(&mut self) -> TestResult {
        let scope = self.scope()?;
        self.commit(Write::insert(Experiment {
            id: id(3),
            workspace_id: scope,
            slug: "exp".into(),
            title: "experiment exp".into(),
            question: None,
            status: "running".into(),
            protocol: object(json!({
                "arms": [{"name": "a", "model": "m1"}],
                "bands": {"min": 0, "max": 8},
                "clone": "isolated",
                "brief_template": "replay-v1",
                "setup": [["git", "config", "user.name", "tester"]],
                "oracle_template": ["git", "rev-parse", "--is-inside-work-tree"],
            })),
            frozen_at: None,
            created_at: at()?,
            updated_at: at()?,
        }))
    }

    fn add_trial(&mut self, task: &str) -> Result<String, Box<dyn Error>> {
        let out = trials::trial_add(
            &mut self.app,
            &self.ctx,
            Some("main"),
            "exp",
            task,
            AddInput {
                arm: "a",
                phase: "staircase",
                band: None,
            },
        )?;
        Ok(text(&out, "id"))
    }
}

/// A value as plain JSON, to read fields without matching on the output model.
fn plain(value: &Value) -> Json {
    match http_json(value) {
        Ok(text) => serde_json::from_str(&text).unwrap_or(Json::Null),
        Err(err) => panic!("not serialisable: {err:?}"),
    }
}

fn text(value: &Value, name: &str) -> String {
    plain(value)[name].as_str().unwrap_or_default().to_owned()
}

fn scan_options(rev: &str) -> ScanOptions<'_> {
    ScanOptions {
        rev,
        limit: None,
        min_lines: 1,
        max_lines: None,
        include_excluded: false,
    }
}

fn create_options() -> CreateOptions {
    CreateOptions {
        validate: false,
        oracle_template: None,
        env: Vec::new(),
        timeout: 1800.0,
    }
}

// covers: C076
#[test]
fn scan_lists_the_commit_that_changes_source_and_tests_together() -> TestResult {
    let mut h = home("scan")?;
    let (repo, sha) = source_repo(&h.dir)?;
    h.register(&repo)?;
    let found = plain(&replay::replay_scan(
        &mut h.app,
        &h.ctx,
        Some("main"),
        "demo",
        &scan_options("HEAD"),
    )?);
    let list = found.as_array().ok_or("a list")?;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["sha"], json!(sha));
    assert_eq!(list[0]["subject"], json!("Add two"));
    assert_eq!(list[0]["test_paths"], json!(["tests/it.rs"]));
    assert_eq!(list[0]["source_paths"], json!(["src/lib.rs"]));
    assert_eq!(list[0]["features"]["src_lines"], json!(4));
    assert_eq!(list[0]["features"]["band"], json!(2));
    assert_eq!(
        list[0]["oracle"],
        json!([["cargo", "test", "-p", "demo", "--test", "it"]])
    );
    assert_eq!(list[0]["excluded"], json!([]));

    let all = plain(&replay::replay_scan(
        &mut h.app,
        &h.ctx,
        Some("main"),
        "demo",
        &ScanOptions {
            include_excluded: true,
            ..scan_options("HEAD")
        },
    )?);
    let reasons: Vec<&Json> = all
        .as_array()
        .ok_or("a list")?
        .iter()
        .map(|c| &c["excluded"])
        .collect();
    assert_eq!(reasons, [&json!([]), &json!(["root"])]);

    let too_big = plain(&replay::replay_scan(
        &mut h.app,
        &h.ctx,
        Some("main"),
        "demo",
        &ScanOptions {
            min_lines: 5,
            ..scan_options("HEAD")
        },
    )?);
    assert_eq!(too_big, json!([]));
    Ok(())
}

// covers: C076
#[test]
fn scan_refuses_a_ref_git_would_read_as_an_option() -> TestResult {
    let mut h = home("scan_ref")?;
    let (repo, _) = source_repo(&h.dir)?;
    h.register(&repo)?;
    let err = replay::replay_scan(
        &mut h.app,
        &h.ctx,
        Some("main"),
        "demo",
        &scan_options("--all"),
    )
    .err()
    .ok_or("a refusal")?;
    assert_eq!(err.code, ErrorCode::GitError);
    assert_eq!(err.message, "invalid git ref '--all'");
    Ok(())
}

// covers: C075
#[test]
fn create_makes_the_base_and_the_task_once() -> TestResult {
    let mut h = home("create")?;
    let (repo, sha) = source_repo(&h.dir)?;
    h.register(&repo)?;
    let shas = [sha.clone()];
    let first = plain(&replay::replay_create(
        &mut h.app,
        &h.ctx,
        Some("main"),
        "demo",
        &shas,
        &create_options(),
    )?);
    assert_eq!(first[0]["status"], json!("created"));
    assert_eq!(first[0]["task_key"], json!("WG-1"));
    let branch = format!("wg/replay/{}", &sha[..12]);
    let base = git(&repo, &["rev-parse", "--verify", &branch])?;
    // The base holds the commit's tests on the parent's source, and never names the solution.
    assert_eq!(
        git(&repo, &["show", &format!("{branch}:tests/it.rs")])?,
        git(&repo, &["show", &format!("{sha}:tests/it.rs")])?
    );
    assert_eq!(
        git(&repo, &["show", &format!("{branch}:src/lib.rs")])?,
        git(&repo, &["show", &format!("{sha}~1:src/lib.rs")])?
    );
    assert!(!git(&repo, &["log", "-1", "--format=%B", &base])?.contains(&sha));

    let page = plain(&wg_tasks_feature::service::task_list(
        &mut h.app,
        &h.ctx,
        Some("main"),
        &json!({"tags": ["replay"]}),
    )?);
    let task = &page["items"][0];
    assert_eq!(task["key"], json!("WG-1"));
    assert_eq!(task["tags"], json!(["experiment-pool", "replay"]));
    assert_eq!(
        task["title"],
        json!(format!("Replay {}: Add two", &sha[..12]))
    );
    assert_eq!(task["metadata"]["replay"]["sha"], json!(sha));
    assert_eq!(task["metadata"]["replay"]["base_ref"], json!(branch));
    assert_eq!(
        task["metadata"]["replay"]["classifier"],
        json!("replay-classifier-2")
    );
    assert_eq!(task["metadata"]["replay"]["validated"], json!(false));

    let again = plain(&replay::replay_create(
        &mut h.app,
        &h.ctx,
        Some("main"),
        "demo",
        &shas,
        &create_options(),
    )?);
    assert_eq!(again[0]["status"], json!("exists"));
    assert_eq!(again[0]["task_key"], json!("WG-1"));
    Ok(())
}

// covers: C082
#[test]
fn show_prints_a_planned_trial_and_refuses_an_unknown_one() -> TestResult {
    let mut h = home("show")?;
    let (repo, sha) = source_repo(&h.dir)?;
    h.register(&repo)?;
    h.experiment()?;
    replay::replay_create(
        &mut h.app,
        &h.ctx,
        Some("main"),
        "demo",
        &[sha],
        &create_options(),
    )?;
    let trial = h.add_trial("WG-1")?;
    let shown = plain(&trials::trial_show(
        &mut h.app,
        &h.ctx,
        Some("main"),
        &trial,
    )?);
    assert_eq!(shown["id"], json!(trial));
    assert_eq!(shown["task_key"], json!("WG-1"));
    assert_eq!(shown["arm"], json!("a"));
    assert_eq!(shown["requested_model"], json!("m1"));
    assert_eq!(shown["status"], json!("planned"));
    assert_eq!(shown["seq"], json!(1));
    assert_eq!(shown["outcome"], Json::Null);
    assert_eq!(shown["worktree_path"], Json::Null);

    let unknown = trials::trial_show(&mut h.app, &h.ctx, Some("main"), "nope")
        .err()
        .ok_or("a refusal")?;
    assert_eq!(unknown.code, ErrorCode::NotFound);
    assert_eq!(unknown.message, "trial 'nope' not found");
    Ok(())
}

// covers: C083, C081
#[test]
fn start_clones_the_base_and_grade_records_a_pass() -> TestResult {
    let mut h = home("pass")?;
    let (repo, sha) = source_repo(&h.dir)?;
    h.register(&repo)?;
    h.experiment()?;
    replay::replay_create(
        &mut h.app,
        &h.ctx,
        Some("main"),
        "demo",
        std::slice::from_ref(&sha),
        &create_options(),
    )?;
    let trial = h.add_trial("WG-1")?;

    let started = plain(&runner::trial_start_run(
        &mut h.app,
        &h.ctx,
        Some("main"),
        &trial,
    )?);
    let worktree = PathBuf::from(started["worktree"].as_str().ok_or("a worktree")?);
    assert!(worktree.join("tests/it.rs").is_file());
    assert_eq!(
        started["branch"],
        json!(format!("wg/trial/{}", &trial[..8]))
    );
    assert_eq!(started["trial"]["status"], json!("running"));
    assert_eq!(started["trial"]["worktree_path"], started["worktree"]);
    assert_eq!(started["base_sha"], started["trial"]["base_ref"]);
    let brief = started["brief"].as_str().ok_or("a brief")?;
    assert!(brief.contains("WG-1"), "{brief}");
    assert!(brief.contains(&trial), "{brief}");
    // The clone holds one branch and no remote, so the solution is not reachable by any ref.
    assert_eq!(git(&worktree, &["remote"])?, "");
    let branches = git(&worktree, &["branch", "--format=%(refname:short)"])?;
    let base_branch = format!("wg/replay/{}", &sha[..12]);
    assert_eq!(
        branches.lines().collect::<Vec<_>>(),
        [
            base_branch.as_str(),
            started["branch"].as_str().ok_or("a branch")?
        ]
    );
    assert!(git(&worktree, &["cat-file", "-e", &sha]).is_err());

    let again = runner::trial_start_run(&mut h.app, &h.ctx, Some("main"), &trial)
        .err()
        .ok_or("a refusal")?;
    assert_eq!(again.code, ErrorCode::Conflict);
    assert_eq!(
        again.message,
        format!("trial {trial} is running; only a planned trial can start")
    );

    let graded = plain(&runner::trial_grade_run(
        &mut h.app,
        &h.ctx,
        Some("main"),
        &trial,
        60.0,
    )?);
    assert_eq!(graded["status"], json!("graded"));
    assert_eq!(graded["outcome"], json!("pass"));
    let detail = &graded["outcome_detail"];
    assert_eq!(detail["base_sha"], started["base_sha"]);
    assert_eq!(detail["head_sha"], started["base_sha"]);
    assert_eq!(detail["oracle"][0]["exit"], json!(0));
    assert_eq!(
        detail["oracle"][0]["argv"],
        json!(["git", "rev-parse", "--is-inside-work-tree"])
    );
    assert_eq!(detail["oracle"][0]["output_tail"], json!("true\n"));
    assert_eq!(detail["gates"], json!([]));
    assert_eq!(detail["flags"], json!({}));
    assert_eq!(detail["run"], Json::Null);
    assert_eq!(detail["escape_check"], json!("no_run"));
    assert_eq!(graded["features"]["actual"]["src_lines"], json!(0));
    assert_eq!(graded["failure_cause"], Json::Null);
    Ok(())
}

// covers: C081
#[test]
fn grade_flags_a_modified_test_as_gaming() -> TestResult {
    let mut h = home("gaming")?;
    let (repo, sha) = source_repo(&h.dir)?;
    h.register(&repo)?;
    h.experiment()?;
    replay::replay_create(
        &mut h.app,
        &h.ctx,
        Some("main"),
        "demo",
        &[sha],
        &create_options(),
    )?;
    let trial = h.add_trial("WG-1")?;
    let started = plain(&runner::trial_start_run(
        &mut h.app,
        &h.ctx,
        Some("main"),
        &trial,
    )?);
    let worktree = PathBuf::from(started["worktree"].as_str().ok_or("a worktree")?);
    write(
        &worktree,
        "tests/it.rs",
        "// the test now passes by itself\n",
    )?;
    write(
        &worktree,
        "src/lib.rs",
        "pub fn two() -> i32 {\n    todo!()\n}\n",
    )?;

    let graded = plain(&runner::trial_grade_run(
        &mut h.app,
        &h.ctx,
        Some("main"),
        &trial,
        60.0,
    )?);
    assert_eq!(graded["status"], json!("graded"));
    assert_eq!(graded["outcome"], json!("fail"));
    let flags = &graded["outcome_detail"]["flags"];
    assert_eq!(flags["tests_modified"], json!(["tests/it.rs"]));
    assert_eq!(flags["stubs"][0]["path"], json!("src/lib.rs"));
    assert_eq!(flags["stubs"][0]["line"], json!("    todo!()"));
    // The oracle itself passed; the flags failed the trial.
    assert_eq!(graded["outcome_detail"]["oracle"][0]["exit"], json!(0));
    assert_eq!(graded["failure_cause"], json!("gaming"));
    assert_eq!(graded["failure_cause_source"], json!("mechanical"));
    assert_eq!(
        graded["notes"],
        json!("tests modified: tests/it.rs; stubs in: src/lib.rs")
    );
    Ok(())
}

// covers: C081
#[test]
fn grade_records_a_failing_gate_and_a_command_that_cannot_start() -> TestResult {
    let mut h = home("fail")?;
    let (repo, sha) = source_repo(&h.dir)?;
    h.register(&repo)?;
    h.experiment()?;
    replay::replay_create(
        &mut h.app,
        &h.ctx,
        Some("main"),
        "demo",
        &[sha],
        &create_options(),
    )?;
    // A gate that cannot start records exit -1, and so fails the trial without flags.
    let scope = h.scope()?;
    h.commit(Write::update(Experiment {
        id: id(3),
        workspace_id: scope,
        slug: "exp".into(),
        title: "experiment exp".into(),
        question: None,
        status: "running".into(),
        protocol: object(json!({
            "arms": [{"name": "a", "model": "m1"}],
            "bands": {"min": 0, "max": 8},
            "clone": "isolated",
            "brief_template": "replay-v1",
            "setup": [],
            "oracle_template": ["git", "rev-parse", "--is-inside-work-tree"],
            "gates": [["git", "cat-file", "-e", "0000000000000000000000000000000000000000"], ["no-such-program-wg"]],
        })),
        frozen_at: None,
        created_at: at()?,
        updated_at: at()?,
    }))?;
    let trial = h.add_trial("WG-1")?;
    runner::trial_start_run(&mut h.app, &h.ctx, Some("main"), &trial)?;
    let graded = plain(&runner::trial_grade_run(
        &mut h.app,
        &h.ctx,
        Some("main"),
        &trial,
        60.0,
    )?);
    assert_eq!(graded["outcome"], json!("fail"));
    let gates = &graded["outcome_detail"]["gates"];
    assert_ne!(gates[0]["exit"], json!(0));
    assert_eq!(gates[1]["exit"], json!(-1));
    assert_eq!(graded["outcome_detail"]["flags"], json!({}));
    // No flags, so nothing classifies it.
    assert_eq!(graded["failure_cause"], Json::Null);
    Ok(())
}
