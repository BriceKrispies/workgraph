//! The pure parts of `demo e2e` (`demo/scenario.py`): the step titles, the bodies of the
//! verification configs and tasks, the report shape and the readers of service replies. Nothing
//! here touches the store, Git, a process or a socket; [`crate::service`] does.

use serde_json::json;
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::{get, items, s};
use wg_json::Value;
use wg_verify_feature::spec::config_write::ConfigBody;

use super::{CONTRACT_CHECK_ARGS, INTEGRATION_CHECK_ARGS, REPOS};

pub const STEP_PROJECT: &str = "Create a project spanning the repositories";
pub const STEP_PARENT: &str = "Create a cross-repository parent task";
pub const STEP_CHILDREN: &str = "Create dependent child tasks";
pub const STEP_CLASSIFY: &str = "Classify the tasks with the deterministic provider";
pub const STEP_MCP_READY: &str = "Query ready tasks through MCP (stdio server subprocess)";
pub const STEP_MCP_CLAIM: &str = "Claim work through MCP";
pub const STEP_ATTEMPT: &str = "Create an agent attempt";
pub const STEP_WORKTREES: &str = "Prepare managed worktrees";
pub const STEP_ADAPTER: &str = "Simulate implementation with the local command adapter";
pub const STEP_COMMITS: &str = "Generate real Git commits";
pub const STEP_INGEST: &str = "Ingest and associate commits across repositories";
pub const STEP_TELEMETRY: &str = "Capture execution telemetry";
pub const STEP_VERIFY: &str = "Run configured verification checks";
pub const STEP_SCENARIOS: &str = "Record passing and failing verification scenarios";
pub const STEP_RETRY: &str = "Retry the failed attempt";
pub const STEP_ACCEPT: &str = "Accept the successfully verified tasks";
pub const STEP_CHURN: &str = "Calculate cumulative churn and final diff separately";
pub const STEP_COUPLING: &str = "Calculate cross-repository analytics";
pub const STEP_HISTORY: &str = "Query historical task results";
pub const STEP_RETRO: &str = "Generate a retrospective report";
pub const STEP_API: &str = "Confirm the results through the local HTTP API";
pub const STEP_UI: &str = "Verify the web frontend serves the persisted data";
pub const STEP_RESTART: &str = "Restart the application and confirm persistence and recovery";

/// The slug of the demonstration's project.
pub const PROJECT: &str = "session-refresh";

/// The verification configs of step 3: a contract check per repository, then the integration
/// check of the project, all running the demo binary `agent`.
#[must_use]
pub fn verification(agent: &str) -> Vec<ConfigBody> {
    let command = |args: &[&str]| {
        let mut command = vec![agent.to_owned()];
        command.extend(args.iter().map(|arg| (*arg).to_owned()));
        command
    };
    let mut bodies: Vec<ConfigBody> = REPOS
        .iter()
        .map(|repo| ConfigBody {
            scope_type: "repository".to_owned(),
            scope: Some(repo.name.to_owned()),
            name: "contract".to_owned(),
            checks: vec![json!({
                "name": format!("{} contract tests", repo.name),
                "kind": "test",
                "command": command(&CONTRACT_CHECK_ARGS),
            })],
        })
        .collect();
    bodies.push(ConfigBody {
        scope_type: "project".to_owned(),
        scope: Some(PROJECT.to_owned()),
        name: "integration".to_owned(),
        checks: vec![json!({
            "name": "cross-repo integration",
            "kind": "integration",
            "command": command(&INTEGRATION_CHECK_ARGS),
        })],
    });
    bodies
}

/// The project of step 3.
#[must_use]
pub fn project_body() -> serde_json::Value {
    let names: Vec<&str> = REPOS.iter().map(|repo| repo.name).collect();
    json!({"slug": PROJECT, "name": "Session refresh", "repositories": names})
}

/// The parent task of step 4.
#[must_use]
pub fn parent_body() -> serde_json::Value {
    let names: Vec<&str> = REPOS.iter().map(|repo| repo.name).collect();
    json!({
        "title": "Add session refresh across services",
        "description": "Users are logged out when tokens expire. Implement token refresh end to end.\n\n- Add a token refresh helper to the SDK\n- Issue refresh tokens in the auth service\n- Use the refresh flow in the web client",
        "acceptance_criteria": ["Expired sessions refresh transparently", "All contract tests pass"],
        "projects": [PROJECT],
        "repositories": names,
        "priority": "high",
    })
}

/// The SDK child of step 5.
#[must_use]
pub fn sdk_body(parent: &str) -> serde_json::Value {
    json!({
        "title": "Add token refresh helper to the SDK",
        "description": "Implement refreshToken in shared-sdk.",
        "parent": parent, "projects": [PROJECT], "repositories": ["shared-sdk"],
        "status": "ready", "priority": "high",
    })
}

/// The auth child of step 5, which depends on the SDK task.
#[must_use]
pub fn auth_body(parent: &str, sdk: &str) -> serde_json::Value {
    json!({
        "title": "Issue refresh tokens in auth-service",
        "description": "Add a refresh token endpoint implementation.",
        "parent": parent, "projects": [PROJECT], "repositories": ["auth-service"],
        "status": "ready", "depends_on": [sdk], "priority": "high",
    })
}

/// The web child of step 5, which depends on the SDK and auth tasks.
#[must_use]
pub fn web_body(parent: &str, sdk: &str, auth: &str) -> serde_json::Value {
    json!({
        "title": "Use refresh flow in web-client",
        "description": "Keep sessions alive with the SDK refresh helper.",
        "parent": parent, "projects": [PROJECT], "repositories": ["web-client", "shared-sdk"],
        "status": "ready", "depends_on": [sdk, auth],
    })
}

/// One finished step: `step`, `title`, `ok`, `seconds` (two decimals), then the details.
#[must_use]
pub fn step_entry<K: AsRef<str>>(
    number: u32,
    title: &str,
    seconds: f64,
    details: Vec<(K, Value)>,
) -> Value {
    let fields = vec![
        ("step", Value::Int(i128::from(number))),
        ("title", text(title)),
        ("ok", Value::Bool(true)),
        ("seconds", Value::Float((seconds * 100.0).round() / 100.0)),
    ];
    let mut entry = fields;
    entry.extend(
        details
            .iter()
            .map(|(name, value)| (name.as_ref(), value.clone())),
    );
    wg_json::dict(entry)
}

/// The report `demo e2e` prints with `--json`.
#[must_use]
pub fn report(home: &str, steps: Vec<Value>) -> Value {
    wg_json::dict(vec![
        ("ok", Value::Bool(!steps.is_empty())),
        ("home", text(home)),
        ("steps", Value::List(steps)),
    ])
}

/// The human line after a passed demonstration.
#[must_use]
pub fn passed_line(steps: usize, home: &str) -> String {
    format!("demonstration passed ({steps} steps); home: {home}\n")
}

/// The failure message of a step whose expectation is not met.
#[must_use]
pub fn failure_message(message: &str, home: &str) -> String {
    format!("demonstration failed: {message} (home kept at {home})")
}

/// A string value.
#[must_use]
pub fn text(value: &str) -> Value {
    Value::Str(value.into())
}

/// A list of strings.
#[must_use]
pub fn texts<S: AsRef<str>>(values: &[S]) -> Value {
    Value::List(values.iter().map(|value| text(value.as_ref())).collect())
}

/// An integer value.
#[must_use]
pub fn count(value: usize) -> Value {
    Value::Int(i128::try_from(value).unwrap_or(0))
}

/// The value at `path` of nested dicts.
#[must_use]
pub fn node<'a>(value: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut current = Some(value);
    for name in path {
        current = current.and_then(|inner| get(inner, name));
    }
    current
}

/// The text at `path` of nested dicts, empty when any part is missing.
#[must_use]
pub fn at(value: &Value, path: &[&str]) -> String {
    s(node(value, path))
}

/// The `name` of each row of a list.
#[must_use]
pub fn keys_of(rows: &Value, name: &str) -> Vec<String> {
    items(Some(rows))
        .iter()
        .map(|row| at(row, &[name]))
        .collect()
}

/// The integer a JSON value holds, `-1` when it holds none.
#[must_use]
pub fn int_of(value: Option<&Value>) -> i128 {
    match value {
        Some(Value::Int(number)) => *number,
        _ => -1,
    }
}

/// The `total` of a page.
#[must_use]
pub fn total(page: &Value) -> i128 {
    int_of(get(page, "total"))
}

/// The first 10 characters of a commit hash.
#[must_use]
pub fn short_sha(sha: &str) -> String {
    sha.chars().take(10).collect()
}

/// The sorted `a+b` names of the repository pairs of a coupling report's edges.
#[must_use]
pub fn coupling_pairs(report: &Value) -> Vec<String> {
    let mut pairs: Vec<String> = items(get(report, "edges"))
        .iter()
        .map(|edge| {
            let mut names = [at(edge, &["source_name"]), at(edge, &["target_name"])];
            names.sort();
            names.join("+")
        })
        .collect();
    pairs.sort();
    pairs.dedup();
    pairs
}

/// The note of step 24: where the browser-level checks are, or how to build the web UI.
#[must_use]
pub fn ui_note(built: bool) -> &'static str {
    if built {
        "browser-level checks: bun run --cwd web test:e2e"
    } else {
        "web UI not built; run bun run --cwd web build to serve it"
    }
}

/// The `{tasks, attempts, events}` dict of step 25.
#[must_use]
pub fn counts_value(counts: (i128, i128, usize)) -> Value {
    wg_json::dict(vec![
        ("tasks", Value::Int(counts.0)),
        ("attempts", Value::Int(counts.1)),
        ("events", count(counts.2)),
    ])
}

/// A failure with exit code 1.
#[must_use]
pub fn failed(message: impl Into<String>) -> CliFailure {
    CliFailure::fail(message, 1)
}

/// `expect`: a step whose condition does not hold fails the demonstration.
///
/// # Errors
///
/// Returns the demonstration failure when `condition` is false.
pub fn expect(home: &std::path::Path, condition: bool, message: &str) -> Result<(), CliFailure> {
    if condition {
        Ok(())
    } else {
        Err(failed(failure_message(
            message,
            &home.display().to_string(),
        )))
    }
}
