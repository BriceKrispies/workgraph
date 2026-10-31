//! `demo e2e` (`demo/scenario.py`, Shell): the 25 numbered steps over an isolated home. Each step
//! calls the service the matching command calls, in this process, except the MCP and HTTP steps
//! which drive the current executable as a child ([`super::net`]). The decisions (titles, bodies,
//! the report) are in [`crate::spec::e2e`].

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::json;
use wg_analytics_feature::service as analytics;
use wg_analytics_feature::service::retrospective::retrospective;
use wg_analytics_feature::spec::query::QueryRequest;
use wg_events_feature::service as events;
use wg_events_feature::spec::ListArgs;
use wg_exec_feature::service::agents::LiveAgents;
use wg_exec_feature::service::attempts::{ListQuery, attempt_get, attempt_list, attempt_manifests};
use wg_exec_feature::service::run_flow::run_start;
use wg_exec_feature::service::worktrees::recovery_out;
use wg_exec_feature::spec::runs::StartRequest;
use wg_git_feature::service::{git_changes, git_sync, task_commits};
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::{get, items, s};
use wg_intel_feature::service::entry::{classify_request, classify_show};
use wg_intel_feature::spec::requests::RunOptions;
use wg_json::Value;
use wg_registry_feature::service::{project_create, repo_list, repo_register, workspace_create};
use wg_registry_feature::spec::registry::workspace_create_body;
use wg_registry_feature::spec::repos::RegisterInput;
use wg_runtime::{App, Feature, RequestContext, human_context, system_context};
use wg_tasks_feature::service::acceptance::{task_accept, task_accept_manually, task_acceptances};
use wg_tasks_feature::service::{
    task_claim, task_complete, task_create, task_history, task_list, task_show, task_update,
};
use wg_types::parse_id;
use wg_verify_feature::service::api as verification;

use super::net::{self, ApiServer, McpSession};
use super::{configure_demo, create_repositories, demo_agent_path, drain_home};
use crate::spec::e2e::{self as spec, at, count, expect, keys_of, node, text, texts};
use crate::spec::{self as base, REPOS};

/// What a step reports besides its number, title and time.
type Details = Vec<(&'static str, Value)>;

/// The state of a demonstration: the open application, the keys of its tasks and the steps so far.
struct Demo<'a> {
    app: App,
    operator: RequestContext,
    home: PathBuf,
    features: &'static [&'static Feature],
    lines: &'a mut Vec<String>,
    steps: Vec<Value>,
    parent: String,
    sdk: String,
    auth: String,
    web: String,
    sdk_attempt: String,
    auth_attempt: String,
    commits: Vec<String>,
}

/// `run_demo`: the steps in order. Returns the report `demo e2e --json` prints.
///
/// # Errors
///
/// Returns the first step whose expectation is not met, or the first service that fails.
pub fn e2e(
    home: &Path,
    features: &'static [&'static Feature],
    lines: &mut Vec<String>,
) -> Result<Value, CliFailure> {
    std::fs::create_dir_all(home)
        .map_err(|err| CliFailure::fail(format!("cannot create {}: {err}", home.display()), 1))?;
    let home = std::path::absolute(home).unwrap_or_else(|_| home.to_path_buf());
    let mut demo = Demo {
        app: App::open(Some(&home))?,
        operator: human_context(Some("demo-operator"))?,
        home,
        features,
        lines,
        steps: Vec::new(),
        parent: String::new(),
        sdk: String::new(),
        auth: String::new(),
        web: String::new(),
        sdk_attempt: String::new(),
        auth_attempt: String::new(),
        commits: Vec::new(),
    };
    demo.run_all()?;
    Ok(spec::report(&demo.home.display().to_string(), demo.steps))
}

impl Demo<'_> {
    fn run_all(&mut self) -> Result<(), CliFailure> {
        self.step(1, base::STEP_WORKSPACE, Self::s1)?;
        self.step(2, base::STEP_REPOSITORIES, Self::s2)?;
        self.step(3, spec::STEP_PROJECT, Self::s3)?;
        self.step(4, spec::STEP_PARENT, Self::s4)?;
        self.step(5, spec::STEP_CHILDREN, Self::s5)?;
        self.step(6, spec::STEP_CLASSIFY, Self::s6)?;
        self.step(7, spec::STEP_MCP_READY, Self::s7)?;
        self.step(8, spec::STEP_MCP_CLAIM, Self::s8)?;
        self.step(9, spec::STEP_ATTEMPT, Self::s9)?;
        self.step(10, spec::STEP_WORKTREES, Self::s10)?;
        self.step(11, spec::STEP_ADAPTER, |_| {
            Ok(vec![("adapter", text("command"))])
        })?;
        self.step(12, spec::STEP_COMMITS, Self::s12)?;
        self.step(13, spec::STEP_INGEST, Self::s13)?;
        self.step(14, spec::STEP_TELEMETRY, Self::s14)?;
        self.step(15, spec::STEP_VERIFY, Self::s15)?;
        self.step(16, spec::STEP_SCENARIOS, |_| {
            Ok(vec![("recorded", Value::Bool(true))])
        })?;
        self.step(17, spec::STEP_RETRY, Self::s17)?;
        self.step(18, spec::STEP_ACCEPT, Self::s18)?;
        self.step(19, spec::STEP_CHURN, Self::s19)?;
        self.step(20, spec::STEP_COUPLING, Self::s20)?;
        self.step(21, spec::STEP_HISTORY, Self::s21)?;
        self.step(22, spec::STEP_RETRO, Self::s22)?;
        let server = ApiServer::start(&self.home)?;
        self.step(23, spec::STEP_API, |demo| demo.s23(&server))?;
        self.step(24, spec::STEP_UI, |demo| demo.s24(&server))?;
        drop(server);
        self.step(25, spec::STEP_RESTART, Self::s25)
    }

    /// `DemoReport.step` and `say`: times `check`, records the entry and prints its line.
    fn step<K: AsRef<str>>(
        &mut self,
        number: u32,
        title: &str,
        check: impl FnOnce(&mut Self) -> Result<Vec<(K, Value)>, CliFailure>,
    ) -> Result<(), CliFailure> {
        let started = Instant::now();
        let details = check(self)?;
        let entry = spec::step_entry(number, title, started.elapsed().as_secs_f64(), details);
        self.steps.push(entry);
        self.lines.push(base::step_line(number, title));
        Ok(())
    }

    fn drain(&self) -> Result<(), CliFailure> {
        drain_home(Some(self.home.clone()), self.features)
    }

    fn task_status(&mut self, key: &str) -> Result<String, CliFailure> {
        let shown = task_show(
            &mut self.app,
            &self.operator,
            Some("demo"),
            &[key.to_owned()],
        )?;
        Ok(shown
            .items
            .first()
            .map(|item| s(get(item, "status")))
            .unwrap_or_default())
    }

    fn attempt(&mut self, id: &str) -> Result<Value, CliFailure> {
        Ok(attempt_get(&mut self.app, &self.operator, id)?)
    }

    fn start(
        &mut self,
        key: &str,
        agent: &str,
        retry_of: Option<&str>,
    ) -> Result<Value, CliFailure> {
        let request = StartRequest {
            agent: Some(agent.to_owned()),
            retry_of: retry_of.map(parse_id).transpose()?,
            ..StartRequest::default()
        };
        let key = key.to_owned();
        Ok(run_start(
            &mut self.app,
            &self.operator,
            &mut LiveAgents,
            Some("demo"),
            &key,
            &request,
        )?)
    }

    /// `_accept_once`: accepts a verified task unless verification already accepted it.
    fn accept_once(&mut self, key: &str) -> Result<(), CliFailure> {
        if self.task_status(key)? != "accepted" {
            task_accept(&mut self.app, &self.operator, Some("demo"), key, "")?;
        }
        let status = self.task_status(key)?;
        expect(&self.home, status == "accepted", &format!("{key} accepted"))
    }

    fn create_task(&mut self, body: &serde_json::Value) -> Result<Value, CliFailure> {
        Ok(task_create(
            &mut self.app,
            &self.operator,
            Some("demo"),
            body,
        )?)
    }

    fn s1(&mut self) -> Result<Details, CliFailure> {
        configure_demo(
            &mut self.app,
            &[("scripted", "success"), ("scripted-buggy", "buggy")],
            false,
        )?;
        workspace_create(
            &mut self.app,
            &self.operator,
            &workspace_create_body("demo", Some("Demo"), "WG", ""),
            true,
        )?;
        self.app.api_token(true)?;
        let database = self.app.paths.database_file().to_string();
        Ok(vec![
            ("workspace", text("demo")),
            ("database", text(&database)),
        ])
    }

    fn s2(&mut self) -> Result<Details, CliFailure> {
        let paths = create_repositories(&self.home.join("repos"))?;
        for (_, path) in &paths {
            let input = RegisterInput {
                path: path.display().to_string(),
                ..RegisterInput::default()
            };
            repo_register(&mut self.app, &self.operator, Some("demo"), &input)?;
        }
        let mut names: Vec<String> =
            repo_list(&mut self.app, &self.operator, Some("demo"), false, None)?
                .iter()
                .map(|row| s(get(row, "name")))
                .collect();
        names.sort();
        let mut expected: Vec<&str> = REPOS.iter().map(|repo| repo.name).collect();
        expected.sort_unstable();
        expect(
            &self.home,
            names == expected,
            &format!("registered {names:?}"),
        )?;
        for repo in &REPOS {
            git_sync(
                &mut self.app,
                &self.operator,
                Some("demo"),
                Some(repo.name),
                false,
            )?;
        }
        Ok(vec![("repositories", texts(&names))])
    }

    fn s3(&mut self) -> Result<Details, CliFailure> {
        let project = project_create(
            &mut self.app,
            &self.operator,
            Some("demo"),
            &spec::project_body(),
        )?;
        let repositories = items(get(&project, "repository_ids")).len();
        expect(
            &self.home,
            repositories == 3,
            "project should span 3 repositories",
        )?;
        for body in spec::verification(&demo_agent_path()?) {
            verification::create_config(&mut self.app, &self.operator, "demo", &body)?;
        }
        Ok(vec![("project", text(&at(&project, &["slug"])))])
    }

    fn s4(&mut self) -> Result<Details, CliFailure> {
        let parent = self.create_task(&spec::parent_body())?;
        self.parent = at(&parent, &["key"]);
        Ok(vec![("parent", text(&self.parent))])
    }

    fn s5(&mut self) -> Result<Details, CliFailure> {
        let sdk = self.create_task(&spec::sdk_body(&self.parent))?;
        self.sdk = at(&sdk, &["key"]);
        let auth = self.create_task(&spec::auth_body(&self.parent, &self.sdk))?;
        self.auth = at(&auth, &["key"]);
        let web = self.create_task(&spec::web_body(&self.parent, &self.sdk, &self.auth))?;
        self.web = at(&web, &["key"]);
        let blocked = at(&auth, &["status"]) == "blocked" && at(&web, &["status"]) == "blocked";
        expect(&self.home, blocked, "dependent children must start blocked")?;
        Ok(vec![(
            "children",
            texts(&[&self.sdk, &self.auth, &self.web]),
        )])
    }

    fn s6(&mut self) -> Result<Vec<(String, Value)>, CliFailure> {
        let keys = [
            self.parent.clone(),
            self.sdk.clone(),
            self.auth.clone(),
            self.web.clone(),
        ];
        for key in &keys {
            classify_request(
                &mut self.app,
                &self.operator,
                Some("demo"),
                key,
                &RunOptions::default(),
            )?;
        }
        self.drain()?;
        let mut categories = Vec::new();
        for key in [self.sdk.clone(), self.auth.clone()] {
            let shown = classify_show(&mut self.app, &self.operator, Some("demo"), &key)?;
            let current = node(&shown, &["current"]).filter(|c| !matches!(c, Value::Null));
            let heuristic = current.is_some_and(|c| at(c, &["provider"]) == "heuristic");
            expect(&self.home, heuristic, "classified")?;
            categories.push((
                key,
                current.map(|c| at(c, &["category"])).unwrap_or_default(),
            ));
        }
        Ok(categories
            .into_iter()
            .map(|(key, category)| (key, text(&category)))
            .collect())
    }

    fn s7(&mut self) -> Result<Details, CliFailure> {
        let mut mcp = McpSession::start(&self.home, "demo-mcp-agent")?;
        let tools = mcp.tool_names()?;
        let ready = mcp.call("task_get_ready", &json!({"workspace": "demo"}))?;
        let claimed = mcp.call(
            "task_claim",
            &json!({"task": self.sdk, "workspace": "demo"}),
        )?;
        expect(
            &self.home,
            tools.iter().any(|name| name == "task_get_ready") && tools.len() >= 40,
            &format!("MCP tools: {}", tools.len()),
        )?;
        let ready_keys: Vec<String> = ready
            .get("tasks")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|task| task.get("key").and_then(serde_json::Value::as_str))
            .map(str::to_owned)
            .collect();
        expect(
            &self.home,
            ready_keys == [self.sdk.clone()],
            &format!("ready via MCP: {ready_keys:?}"),
        )?;
        let by = claimed
            .get("claimed_by")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        expect(
            &self.home,
            by == "agent:demo-mcp-agent",
            &format!("claimed_by {by}"),
        )?;
        Ok(vec![
            ("mcp_tools", count(tools.len())),
            ("ready", texts(&ready_keys)),
            ("claimed_by", text(&by)),
        ])
    }

    fn s8(&mut self) -> Result<Details, CliFailure> {
        Ok(vec![("task", text(&self.sdk)), ("status", text("claimed"))])
    }

    fn s9(&mut self) -> Result<Details, CliFailure> {
        let key = self.sdk.clone();
        let attempt = self.start(&key, "scripted", None)?;
        self.sdk_attempt = at(&attempt, &["id"]);
        expect(
            &self.home,
            at(&attempt, &["status"]) == "queued",
            "attempt queued",
        )?;
        Ok(vec![
            ("attempt", text(&self.sdk_attempt)),
            ("provider", text(&at(&attempt, &["provider"]))),
        ])
    }

    fn s10(&mut self) -> Result<Details, CliFailure> {
        self.drain()?;
        let id = self.sdk_attempt.clone();
        let attempt = self.attempt(&id)?;
        let manifests = attempt_manifests(&mut self.app, &self.operator, &id)?;
        let kinds = keys_of(&manifests, "kind");
        let has = |kind: &str| kinds.iter().any(|k| k == kind);
        expect(
            &self.home,
            has("baseline") && has("outcome"),
            &format!("manifests {kinds:?}"),
        )?;
        let status = at(&attempt, &["status"]);
        let error = at(&attempt, &["error"]);
        expect(
            &self.home,
            status == "succeeded",
            &format!("sdk attempt {status}: {error}"),
        )?;
        let key = self.sdk.clone();
        let commits = task_commits(&mut self.app, &self.operator, Some("demo"), &key, 0.0)?;
        let rows = items(Some(&commits));
        let linked = rows.first().is_some_and(|row| {
            items(get(row, "links"))
                .iter()
                .any(|l| at(l, &["source"]) == "execution_context")
        });
        expect(&self.home, rows.len() == 1 && linked, "commit linked")?;
        let tokens = spec::int_of(node(&attempt, &["usage", "input_tokens"]));
        let no_cost = matches!(get(&attempt, "cost_usd"), None | Some(Value::Null));
        expect(&self.home, tokens == 1200 && no_cost, "telemetry honesty")?;
        self.commits = rows
            .iter()
            .map(|row| spec::short_sha(&at(row, &["sha"])))
            .collect();
        Ok(vec![
            ("manifests", texts(&kinds)),
            ("commits", texts(&self.commits)),
            (
                "usage",
                node(&attempt, &["usage"]).cloned().unwrap_or(Value::Null),
            ),
            (
                "duration_ms",
                get(&attempt, "duration_ms").cloned().unwrap_or(Value::Null),
            ),
        ])
    }

    fn s12(&mut self) -> Result<Details, CliFailure> {
        Ok(vec![("commits", texts(&self.commits))])
    }

    fn s13(&mut self) -> Result<Details, CliFailure> {
        let sdk = self.sdk.clone();
        self.accept_once(&sdk)?;
        self.drain()?;
        let auth = self.auth.clone();
        let run = self.start(&auth, "scripted-buggy", None)?;
        self.auth_attempt = at(&run, &["id"]);
        let web = self.web.clone();
        let blocked = self.task_status(&web)?;
        expect(
            &self.home,
            blocked == "blocked",
            "web client still blocked by auth",
        )?;
        self.drain()?;
        Ok(vec![
            ("sdk", text("accepted")),
            ("auth_attempt", text(&self.auth_attempt)),
        ])
    }

    fn s14(&mut self) -> Result<Details, CliFailure> {
        let id = self.auth_attempt.clone();
        let attempt = self.attempt(&id)?;
        let auth = self.auth.clone();
        let history = task_history(&mut self.app, &self.operator, Some("demo"), &auth, &[])?;
        let progress = items(Some(&history))
            .iter()
            .filter(|event| at(event, &["event_type"]) == "attempt.progress")
            .count();
        expect(&self.home, progress > 0, "progress telemetry recorded")?;
        Ok(vec![
            ("progress_events", count(progress)),
            (
                "queue_wait_ms",
                get(&attempt, "queue_wait_ms")
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
        ])
    }

    fn s15(&mut self) -> Result<Details, CliFailure> {
        let id = self.auth_attempt.clone();
        let first = self.attempt(&id)?;
        let state = format!(
            "{} {}",
            at(&first, &["status"]),
            at(&first, &["exit_reason"])
        );
        expect(
            &self.home,
            state == "failed verification_failed",
            &format!("first {state}"),
        )?;
        let failures =
            verification::failures(&mut self.app, &self.operator, "demo", None, None, None)?;
        let auth = self.auth.clone();
        let mine = items(Some(&failures))
            .iter()
            .find(|f| at(f, &["task_key"]) == auth);
        let signatures = mine
            .map(|f| items(get(f, "failure_signatures")).to_vec())
            .unwrap_or_default();
        let recorded = signatures
            .first()
            .is_some_and(|sig| s(Some(sig)).starts_with("test:"));
        expect(&self.home, recorded, "failure signature recorded")?;
        let sdk = self.sdk.clone();
        let runs = verification::task_runs(&mut self.app, &self.operator, "demo", &sdk)?;
        let passed = items(Some(&runs)).first().cloned().unwrap_or(Value::Null);
        expect(
            &self.home,
            at(&passed, &["status"]) == "passed",
            "sdk verification passed",
        )?;
        Ok(vec![
            ("passing_run", text(&at(&passed, &["id"]))),
            ("failing_run_signatures", Value::List(signatures)),
        ])
    }

    fn s17(&mut self) -> Result<Details, CliFailure> {
        let (auth, first) = (self.auth.clone(), self.auth_attempt.clone());
        let retry = self.start(&auth, "scripted", Some(&first))?;
        self.drain()?;
        let out = self.attempt(&at(&retry, &["id"]))?;
        let good =
            at(&out, &["status"]) == "succeeded" && at(&out, &["retry_of_attempt_id"]) == first;
        expect(
            &self.home,
            good,
            &format!("retry {}", at(&out, &["status"])),
        )?;
        self.accept_once(&auth)?;
        self.drain()?;
        let web = self.web.clone();
        let run = self.start(&web, "scripted", None)?;
        self.drain()?;
        let done = self.attempt(&at(&run, &["id"]))?;
        expect(
            &self.home,
            at(&done, &["status"]) == "succeeded",
            "web attempt succeeded",
        )?;
        Ok(vec![("retry_attempt", text(&at(&retry, &["id"])))])
    }

    fn s18(&mut self) -> Result<Details, CliFailure> {
        let web = self.web.clone();
        self.accept_once(&web)?;
        let acceptances = task_acceptances(&mut self.app, &self.operator, Some("demo"), &web)?;
        let automated = items(Some(&acceptances))
            .iter()
            .any(|a| at(a, &["mode"]) == "automated");
        expect(&self.home, automated, "automated acceptance")?;
        let parent = self.parent.clone();
        let shown = task_show(
            &mut self.app,
            &self.operator,
            Some("demo"),
            std::slice::from_ref(&parent),
        )?;
        let accepted = shown
            .items
            .first()
            .map(|t| spec::int_of(node(t, &["rollup", "accepted"])));
        expect(&self.home, accepted == Some(3), "all children accepted")?;
        let (app, op) = (&mut self.app, &self.operator);
        task_update(app, op, Some("demo"), &parent, &json!({"status": "ready"}))?;
        task_claim(app, op, Some("demo"), &parent, &json!({}))?;
        task_complete(
            app,
            op,
            Some("demo"),
            &parent,
            &json!({"summary": "children accepted"}),
        )?;
        // The parent has no implementation of its own and agent branches are never merged
        // automatically, so it is accepted by a recorded human decision.
        task_accept_manually(
            app,
            op,
            Some("demo"),
            &parent,
            "all child tasks accepted after passing verification",
        )?;
        self.drain()?;
        Ok(vec![(
            "accepted",
            texts(&[&self.sdk, &self.auth, &self.web, &self.parent]),
        )])
    }

    fn s19(&mut self) -> Result<Details, CliFailure> {
        let auth = self.auth.clone();
        let changes = git_changes(&mut self.app, &self.operator, Some("demo"), &auth, 0.5)?;
        let churn = node(&changes, &["cumulative_churn"])
            .cloned()
            .unwrap_or(Value::Null);
        let surface = items(get(&changes, "final_change_surface")).to_vec();
        expect(
            &self.home,
            spec::int_of(get(&churn, "commits")) == 2,
            "auth churn commits",
        )?;
        expect(&self.home, surface.len() == 1, "final surface computed")?;
        let insertions = |v: &Value| spec::int_of(get(v, "insertions"));
        expect(
            &self.home,
            insertions(&churn) > insertions(&surface[0]),
            "cumulative churn exceeds final change",
        )?;
        let pick = |v: &Value, keys: &[&'static str]| {
            wg_json::dict(
                keys.iter()
                    .map(|key| (*key, get(v, key).cloned().unwrap_or(Value::Null)))
                    .collect(),
            )
        };
        Ok(vec![
            (
                "cumulative_churn",
                pick(&churn, &["commits", "insertions", "deletions"]),
            ),
            (
                "final_change_surface",
                pick(
                    &surface[0],
                    &[
                        "insertions",
                        "deletions",
                        "files_changed",
                        "functions_affected",
                    ],
                ),
            ),
        ])
    }

    fn s20(&mut self) -> Result<Details, CliFailure> {
        let report = analytics::coupling(&mut self.app, &self.operator, Some("demo"), None, 1)?;
        let pairs = spec::coupling_pairs(&report);
        expect(
            &self.home,
            pairs.iter().any(|pair| pair == "shared-sdk+web-client"),
            &format!("pairs {pairs:?}"),
        )?;
        Ok(vec![
            ("edges", texts(&pairs)),
            (
                "summary",
                get(&report, "summary").cloned().unwrap_or(Value::Null),
            ),
        ])
    }

    fn s21(&mut self) -> Result<Details, CliFailure> {
        let history =
            analytics::history(&mut self.app, &self.operator, Some("demo"), None, None, 50)?;
        let accepted = items(Some(&history)).len();
        expect(&self.home, accepted == 4, &format!("history {accepted}"))?;
        let request = QueryRequest {
            metric: "attempts_per_task".to_owned(),
            period: "week".to_owned(),
            ..QueryRequest::default()
        };
        let answer = analytics::query(&mut self.app, &self.operator, Some("demo"), &request)?;
        let row = items(get(&answer, "rows"))
            .first()
            .cloned()
            .unwrap_or(Value::Null);
        Ok(vec![
            ("accepted_tasks", count(accepted)),
            (
                "attempts_per_task_median",
                get(&row, "median").cloned().unwrap_or(Value::Null),
            ),
        ])
    }

    fn s22(&mut self) -> Result<Details, CliFailure> {
        let retro = retrospective(
            &mut self.app,
            &self.operator,
            Some("demo"),
            &(7, None, None, true, None),
        )?;
        let content = at(&retro, &["content"]);
        expect(
            &self.home,
            content.contains("## Work completed"),
            "retrospective content",
        )?;
        expect(
            &self.home,
            content.contains(&self.auth) || content.contains("accepted"),
            "retro mentions work",
        )?;
        Ok(vec![
            ("report_id", text(&at(&retro, &["id"]))),
            ("narrative_source", text(&at(&retro, &["narrative_source"]))),
        ])
    }

    fn s23(&mut self, server: &ApiServer) -> Result<Details, CliFailure> {
        let token = self.app.api_token(false)?;
        let base = &server.base;
        let tasks = net::get_json(&format!("{base}/api/v1/ws/demo/tasks?limit=100"), &token)?;
        let total = tasks
            .get("total")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(-1);
        expect(&self.home, total == 4, &format!("API tasks {total}"))?;
        let accepted = tasks
            .get("items")
            .and_then(serde_json::Value::as_array)
            .map_or(0, |rows| {
                rows.iter()
                    .filter(|t| t.get("status") == Some(&json!("accepted")))
                    .count()
            });
        expect(&self.home, accepted == 4, "all accepted via API")?;
        let (status, _) = net::get(&format!("{base}/api/v1/ws/demo/tasks"), None)?;
        expect(&self.home, status == 401, "token required")?;
        let overview = net::get_json(&format!("{base}/api/v1/ws/demo/analytics/overview"), &token)?;
        let sum: f64 = overview
            .get("accepted_by_period")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| row.get("value").and_then(serde_json::Value::as_f64))
            .sum();
        expect(&self.home, (sum - 4.0).abs() < f64::EPSILON, "overview")?;
        let graph = net::get_json(&format!("{base}/api/v1/ws/demo/graph"), &token)?;
        let edges = graph
            .get("edges")
            .and_then(serde_json::Value::as_array)
            .map_or(0, Vec::len);
        Ok(vec![
            ("api_tasks", Value::Int(i128::from(total))),
            ("graph_edges", count(edges)),
        ])
    }

    fn s24(&mut self, server: &ApiServer) -> Result<Details, CliFailure> {
        let (status, body) = net::get(&format!("{}/", server.base), None)?;
        expect(&self.home, status == 200, "index served")?;
        let built = body.contains("<div id=\"root\">");
        if built {
            expect(
                &self.home,
                body.contains("name=\"workgraph-token\""),
                "token injected into UI",
            )?;
        }
        Ok(vec![
            ("ui_built", Value::Bool(built)),
            ("ui_status", Value::Int(i128::from(status))),
            ("note", text(spec::ui_note(built))),
        ])
    }

    /// Step 25: counts, a restart (the application is closed and opened again over the same
    /// home), the same counts, then the recovery passes and the event projection.
    fn s25(&mut self) -> Result<Details, CliFailure> {
        let before = self.counts()?;
        self.app = App::open(Some(&self.home))?;
        let after = self.counts()?;
        expect(
            &self.home,
            before == after,
            &format!("persistence mismatch {before:?} vs {after:?}"),
        )?;
        let system = system_context("demo")?;
        let recovered = self
            .app
            .worker_recover(&system, |app| recovery_out(app, &system_context("demo")?))?;
        let report = events::verify(&mut self.app, &self.operator, Some("demo"))?;
        expect(
            &self.home,
            report.consistent(),
            &format!("projection {:?}", report.mismatches),
        )?;
        Ok(vec![
            ("before", spec::counts_value(before)),
            ("after", spec::counts_value(after)),
            (
                "recovered_jobs",
                count(items(get(&recovered, "recovered_jobs")).len()),
            ),
            (
                "orphaned_worktrees",
                count(items(node(&recovered, &["worktrees", "orphaned"])).len()),
            ),
            ("event_projection", text("consistent")),
        ])
    }

    /// The tasks, attempts and events of the workspace.
    fn counts(&mut self) -> Result<(i128, i128, usize), CliFailure> {
        let tasks = task_list(&mut self.app, &self.operator, Some("demo"), &json!({}))?;
        let query = ListQuery {
            workspace: Some("demo"),
            task: None,
            statuses: &[],
            provider: None,
            limit: 1,
            offset: 0,
        };
        let attempts = attempt_list(&mut self.app, &self.operator, &query)?;
        let args = ListArgs {
            entity: None,
            prefix: None,
            after: None,
            limit: 5000,
        };
        let logged = events::list(&mut self.app, &self.operator, Some("demo"), &args)?;
        Ok((spec::total(&tasks), spec::total(&attempts), logged.len()))
    }
}
