//! The pure parts of the demo commands (`cli/commands/demo.py`, `demo/scenario.py`, `demo/seed.py`):
//! the clap command, the readers of its matches, the three demo repositories and the files they
//! carry, the seed table, the scripted agent modes and the human view of `demo seed`. Nothing here
//! touches the file system, Git or the store; [`crate::service`] does, and [`crate::cli`] prints.

pub mod e2e;

use std::path::PathBuf;

use clap::{ArgMatches, Command};
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::{flag, get, one, s};
use wg_iface::cli::tree::snapshot_command;
use wg_json::Value;
use wg_render::Console;
use wg_verify_feature::spec::config_write::ConfigBody;

/// The `demo` group, from the snapshot so help and parameters match `contracts/cli.json`.
#[must_use]
pub fn command() -> Command {
    snapshot_command("demo")
}

/// `demo e2e`'s `--home` (when given) and `--keep`.
#[must_use]
pub fn e2e_args(m: &ArgMatches) -> (Option<PathBuf>, bool) {
    (one(m, "--home").map(PathBuf::from), flag(m, "--keep"))
}

/// `demo seed`'s `--workspace` (default `demo`) and `--force`.
#[must_use]
pub fn seed_args(m: &ArgMatches) -> (String, bool) {
    (
        one(m, "--workspace").unwrap_or_else(|| "demo".to_owned()),
        flag(m, "--force"),
    )
}

/// The name of the scripted agent and the demo checks (DV4): a `workgraph-demo-agent` binary next
/// to `workgraph`, not `<python> scripted_agent.py`.
pub const DEMO_AGENT_BINARY: &str = "workgraph-demo-agent";

/// The `check` subcommand of the demo binary: the per-repository contract check.
pub const CONTRACT_CHECK_ARGS: [&str; 1] = ["check"];

/// The `integration` subcommand of the demo binary: the cross-repository check.
pub const INTEGRATION_CHECK_ARGS: [&str; 1] = ["integration"];

/// The `--mode` of each scripted agent `demo seed` configures, in the order Python sets them.
pub const SEED_AGENT_MODES: [(&str, &str); 3] = [
    ("scripted", "success"),
    ("scripted-buggy", "buggy"),
    ("scripted-fail", "fail"),
];

/// The arguments of a scripted agent in `mode`: `--mode <mode> {manifest}`, after the executable.
#[must_use]
pub fn agent_args(mode: &str) -> [String; 3] {
    [
        "--mode".to_owned(),
        mode.to_owned(),
        "{manifest}".to_owned(),
    ]
}

/// The step title of `demo e2e` step 1.
pub const STEP_WORKSPACE: &str = "Initialize a Workgraph workspace";

/// The step title of `demo e2e` step 2.
pub const STEP_REPOSITORIES: &str = "Register all three Git repositories";

/// One of the three demo repositories: its name, the file it seeds, the planned file and the
/// plan's content (`REPOS` of `demo/scenario.py`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepoSpec {
    pub name: &'static str,
    pub seed_file: &'static str,
    pub seed: &'static str,
    pub plan_file: &'static str,
    pub plan: &'static str,
}

/// The three demo repositories, in the order Python creates them.
pub const REPOS: [RepoSpec; 3] = [
    RepoSpec {
        name: "auth-service",
        seed_file: "auth/__init__.py",
        seed: "def login(user: str, password: str) -> bool:\n    return bool(user and password)\n",
        plan_file: "auth/refresh.py",
        plan: "# {task}: refresh token issuance\nMARKER = 'WORKGRAPH_DEMO_OK'\n\n\n\
               def issue_refresh_token(user: str) -> str:\n    return f'refresh-{user}'\n\n\n\
               class RefreshPolicy:\n    ttl_seconds = 3600\n",
    },
    RepoSpec {
        name: "shared-sdk",
        seed_file: "src/client.ts",
        seed: "export function request(path: string): string {\n  return path;\n}\n",
        plan_file: "src/token.ts",
        plan: "// {task}: token refresh helper\nexport const MARKER = 'WORKGRAPH_DEMO_OK';\n\n\
               export function refreshToken(token: string): string {\n  return `refreshed:${token}`;\n}\n\n\
               export interface TokenPair {\n  access: string;\n  refresh: string;\n}\n",
    },
    RepoSpec {
        name: "web-client",
        seed_file: "app/main.js",
        seed: "export function render() {\n  return 'login';\n}\n",
        plan_file: "app/session.js",
        plan: "// {task}: use the refresh flow\nexport const MARKER = 'WORKGRAPH_DEMO_OK';\n\n\
               export function keepSessionAlive(client) {\n  return client.refresh();\n}\n",
    },
];

/// `[ n] ok  title`, the line `say` prints in `demo/scenario.py` for a finished step.
#[must_use]
pub fn step_line(number: u32, title: &str) -> String {
    format!("[{number:>2}] ok  {title}")
}

/// The outcome of a file write, as the CLI failure `demo e2e` reports for it.
///
/// # Errors
///
/// Returns a CLI failure naming `path` when the write failed.
pub fn written<T>(result: std::io::Result<T>, path: &std::path::Path) -> Result<T, CliFailure> {
    result.map_err(|err| CliFailure::fail(format!("cannot write {}: {err}", path.display()), 1))
}

/// The repository's `README.md`.
#[must_use]
pub fn readme(name: &str) -> String {
    format!("# {name}\n")
}

/// The commit message of a repository's first commit.
#[must_use]
pub fn initial_commit_message(name: &str) -> String {
    format!("Initial {name}")
}

/// `workgraph-demo.json` of a repository: `json.dumps({"file": ..., "content": ...})`, byte for
/// byte, so the seeded commits hash the same as Python's.
#[must_use]
pub fn plan_document(repo: &RepoSpec) -> String {
    format!(
        "{{\"file\": {}, \"content\": {}}}",
        json_string(repo.plan_file),
        json_string(repo.plan)
    )
}

/// `json.dumps` of one string with Python's defaults: `ensure_ascii`, and the short escapes for
/// quote, backslash, `\n`, `\r`, `\t`, `\b` and `\f`.
#[must_use]
pub fn json_string(text: &str) -> String {
    let mut out = String::from("\"");
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            ' '..='~' => out.push(ch),
            _ => {
                let mut units = [0u16; 2];
                for unit in ch.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

/// `check.py`, the contract check the Python demo runs in each repository. The demo binary runs
/// the same check now (DV4); the file stays in the repository so the commits match.
pub const CHECK_SCRIPT: &str = "import json, pathlib, sys
plan = json.loads(pathlib.Path('workgraph-demo.json').read_text())
target = pathlib.Path(plan['file'])
if not target.exists():
    print(f\"FAILED tests/test_contract.py::test_{target.stem}_exists - missing {target}\")
    sys.exit(1)
if 'WORKGRAPH_DEMO_OK' not in target.read_text():
    print(f\"FAILED tests/test_contract.py::test_{target.stem}_contract - marker missing\")
    sys.exit(1)
print('1 passed')
";

/// The cross-repository integration check, configured through `WORKGRAPH_REPO_<NAME>_PATH`.
pub const INTEGRATION_SCRIPT: &str = "import os, pathlib, sys
missing = []
for repo in ('AUTH_SERVICE', 'SHARED_SDK', 'WEB_CLIENT'):
    path = os.environ.get(f'WORKGRAPH_REPO_{repo}_PATH')
    if path and not pathlib.Path(path).exists():
        missing.append(repo)
print('integration ok' if not missing else f'missing {missing}')
sys.exit(1 if missing else 0)
";

/// One task of `demo seed` (`TASKS` of `demo/seed.py`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedTask {
    pub title: &'static str,
    pub repos: &'static [&'static str],
    pub agent: &'static str,
    /// Whether the first attempt is followed by a retry with the `scripted` agent.
    pub retry: bool,
    pub description: &'static str,
    pub priority: &'static str,
}

/// The seven tasks of `demo seed`, in creation order.
pub const SEED_TASKS: [SeedTask; 7] = [
    SeedTask {
        title: "Fix crash when the login token expires",
        repos: &["auth-service"],
        agent: "scripted",
        retry: false,
        description: "Users see an exception and the app crashes after one hour.",
        priority: "critical",
    },
    SeedTask {
        title: "Add dark mode to the web client",
        repos: &["web-client"],
        agent: "scripted",
        retry: false,
        description: "Implement a theme toggle in the UI.",
        priority: "medium",
    },
    SeedTask {
        title: "Refactor SDK request handling",
        repos: &["shared-sdk"],
        agent: "scripted-buggy",
        retry: true,
        description: "Simplify and restructure the client module.",
        priority: "medium",
    },
    SeedTask {
        title: "Improve token validation performance",
        repos: &["auth-service", "shared-sdk"],
        agent: "scripted",
        retry: false,
        description: "Token validation is slow; reduce latency with caching.",
        priority: "high",
    },
    SeedTask {
        title: "Document the session refresh flow",
        repos: &["web-client"],
        agent: "scripted",
        retry: false,
        description: "Write docs and a README section about refresh tokens.",
        priority: "low",
    },
    SeedTask {
        title: "Upgrade SDK dependencies",
        repos: &["shared-sdk"],
        agent: "scripted-fail",
        retry: false,
        description: "Bump dependency versions and fix deprecations.",
        priority: "low",
    },
    SeedTask {
        title: "Investigate flaky integration tests",
        repos: &["auth-service", "web-client"],
        agent: "scripted",
        retry: false,
        description: "Research why integration tests fail intermittently.",
        priority: "medium",
    },
];

/// The project a seeded task is filed under: `frontend` for the web client alone, else `platform`.
#[must_use]
pub fn seed_project(repos: &[&str]) -> &'static str {
    if repos == ["web-client"] {
        "frontend"
    } else {
        "platform"
    }
}

/// The message `demo seed` fails with when the home already holds data and `--force` is absent.
pub const SEED_HAS_DATA: &str =
    "this Workgraph home already has data; use --force or --home <dir> for an isolated demo";

/// The human line of `demo seed` (`demo data ready in workspace ...`).
#[must_use]
pub fn seed_view(_console: &Console, summary: &Value) -> String {
    format!(
        "demo data ready in workspace {}: {} tasks, {} attempts, {} commits\n",
        s(get(summary, "workspace")),
        s(get(summary, "tasks")),
        s(get(summary, "attempts")),
        s(get(summary, "commits")),
    )
}

/// `str.title()` for a slug: the first letter of each run of letters upper-cased, the rest lower.
#[must_use]
pub fn title_case(text: &str) -> String {
    let mut out = String::new();
    let mut start = true;
    for ch in text.chars() {
        if ch.is_alphabetic() {
            if start {
                out.extend(ch.to_uppercase());
            } else {
                out.extend(ch.to_lowercase());
            }
            start = false;
        } else {
            out.push(ch);
            start = true;
        }
    }
    out
}

/// The automation rule of `demo seed`.
#[must_use]
pub fn seed_rule() -> serde_json::Value {
    serde_json::json!({
        "name": "triage critical bugs",
        "trigger": {"type": "event", "event_types": ["task.created"]},
        "conditions": {"field": "task.priority", "op": "eq", "value": "critical"},
        "actions": [
            {"type": "add_tags", "tags": ["triage"]},
            {"type": "notify", "level": "warning", "title": "Critical task {{task.key}}: {{task.title}}"},
        ],
    })
}

/// The two projects of `demo seed`, as `ProjectCreate` bodies.
#[must_use]
pub fn seed_projects() -> [serde_json::Value; 2] {
    let all: Vec<&str> = REPOS.iter().map(|repo| repo.name).collect();
    [
        serde_json::json!({"slug": "platform", "name": "Platform", "repositories": all}),
        serde_json::json!({"slug": "frontend", "name": "Frontend", "repositories": ["web-client", "shared-sdk"]}),
    ]
}

/// The verification configs of `demo seed` (a contract check per repository, then the optional
/// integration check of `platform`), as `VerificationConfigBody`s running the demo binary `agent`.
#[must_use]
pub fn seed_verification(agent: &str) -> Vec<ConfigBody> {
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
            checks: vec![serde_json::json!({
                "name": format!("{} contract tests", repo.name),
                "kind": "test",
                "command": command(&CONTRACT_CHECK_ARGS),
            })],
        })
        .collect();
    bodies.push(ConfigBody {
        scope_type: "project".to_owned(),
        scope: Some("platform".to_owned()),
        name: "integration".to_owned(),
        checks: vec![serde_json::json!({
            "name": "cross-repo integration",
            "kind": "integration",
            "command": command(&INTEGRATION_CHECK_ARGS),
            "required": false,
        })],
    });
    bodies
}

/// The `TaskCreate` body of a seed task.
#[must_use]
pub fn task_body(task: &SeedTask) -> serde_json::Value {
    serde_json::json!({
        "title": task.title,
        "description": task.description,
        "repositories": task.repos,
        "projects": [seed_project(task.repos)],
        "priority": task.priority,
        "status": "ready",
        "acceptance_criteria": ["Contract tests pass"],
    })
}

/// The umbrella task `demo seed` parents the first four tasks under.
#[must_use]
pub fn umbrella_body() -> serde_json::Value {
    serde_json::json!({
        "title": "Session reliability initiative",
        "description": "Umbrella for session-related improvements.",
        "projects": ["platform"],
        "repositories": REPOS.iter().map(|repo| repo.name).collect::<Vec<_>>(),
        "priority": "high",
    })
}

/// The follow-up task, which depends on the first two seed tasks.
#[must_use]
pub fn follow_body(first: &str, second: &str) -> serde_json::Value {
    serde_json::json!({
        "title": "Handle refresh failures gracefully",
        "description": "Show a re-login prompt when refresh fails.",
        "repositories": ["web-client"],
        "projects": ["frontend"],
        "status": "ready",
        "depends_on": [first, second],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_string_matches_python_dumps() {
        assert_eq!(json_string("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
        assert_eq!(json_string("é"), "\"\\u00e9\"");
        assert_eq!(json_string("\u{1F600}"), "\"\\ud83d\\ude00\"");
        assert_eq!(json_string("\u{1}"), "\"\\u0001\"");
    }
}
