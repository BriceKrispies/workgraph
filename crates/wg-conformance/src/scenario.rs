//! Scenario files: parsing and validation against `scenario.schema.json`.
//!
//! The validator is hand-written and mirrors the schema (unknown keys are
//! rejected, exactly one step kind per step); a test keeps the two in step.

use serde_json::{Map, Value};

/// A setup action run before the steps (a git command in a named repository).
#[derive(Debug, Clone, PartialEq)]
pub struct Setup {
    pub git: String,
    pub repo: String,
    pub files: Vec<(String, String)>,
    pub message: Option<String>,
    pub args: Vec<String>,
}

/// An assertion over the most recent `cli` result, or over files.
#[derive(Debug, Clone, PartialEq)]
pub enum Assertion {
    Exit(i64),
    StdoutContains(String),
    StderrContains(String),
    Json {
        path: String,
        equals: Value,
    },
    File {
        path: String,
        exists: bool,
        contains: Option<String>,
    },
    Db {
        table: String,
        filter: Value,
        count: u64,
    },
}

/// One scenario step.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Cli {
        args: Vec<String>,
        json: bool,
        stdin: Option<String>,
        env: Vec<(String, String)>,
        save: Vec<(String, String)>,
        expect_exit: Option<i64>,
    },
    Mcp {
        tool: String,
        args: Value,
        save: Vec<(String, String)>,
    },
    Http {
        method: String,
        path: String,
        body: Option<Value>,
        /// Read the response as server-sent events and stop after this many.
        max_events: Option<usize>,
        save: Vec<(String, String)>,
    },
    Drain {
        max_jobs: u64,
    },
    Assert(Assertion),
}

/// A validated scenario.
#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    pub id: String,
    pub covers: Vec<String>,
    pub setup: Vec<Setup>,
    pub steps: Vec<Step>,
    pub not_observable: Vec<(String, String)>,
}

type Res<T> = Result<T, String>;

fn obj<'a>(v: &'a Value, at: &str) -> Res<&'a Map<String, Value>> {
    v.as_object()
        .ok_or_else(|| format!("{at}: expected an object"))
}

fn only_keys(m: &Map<String, Value>, allowed: &[&str], at: &str) -> Res<()> {
    match m.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("{at}: unknown key `{k}`")),
        None => Ok(()),
    }
}

fn string(v: &Value, at: &str) -> Res<String> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("{at}: expected a string"))
}

fn strings(v: &Value, at: &str) -> Res<Vec<String>> {
    let arr = v
        .as_array()
        .ok_or_else(|| format!("{at}: expected an array"))?;
    arr.iter()
        .enumerate()
        .map(|(i, s)| string(s, &format!("{at}[{i}]")))
        .collect()
}

fn str_map(v: &Value, at: &str) -> Res<Vec<(String, String)>> {
    let m = obj(v, at)?;
    m.iter()
        .map(|(k, s)| Ok((k.clone(), string(s, &format!("{at}.{k}"))?)))
        .collect()
}

fn boolean(v: &Value, at: &str) -> Res<bool> {
    v.as_bool()
        .ok_or_else(|| format!("{at}: expected a boolean"))
}

fn integer(v: &Value, at: &str) -> Res<i64> {
    v.as_i64()
        .ok_or_else(|| format!("{at}: expected an integer"))
}

fn id_ok(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn cover_ok(s: &str) -> bool {
    let letters = s.chars().take_while(char::is_ascii_uppercase).count();
    let digits = &s[letters..];
    letters > 0 && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

fn parse_setup(v: &Value, at: &str) -> Res<Setup> {
    let m = obj(v, at)?;
    only_keys(m, &["git", "repo", "files", "message", "args"], at)?;
    let get = |k: &str| m.get(k).ok_or_else(|| format!("{at}: missing `{k}`"));
    let git = string(get("git")?, &format!("{at}.git"))?;
    if git.is_empty() {
        return Err(format!("{at}.git: must not be empty"));
    }
    let repo = string(get("repo")?, &format!("{at}.repo"))?;
    if repo.is_empty() {
        return Err(format!("{at}.repo: must not be empty"));
    }
    let args = m
        .get("args")
        .map(|a| strings(a, &format!("{at}.args")))
        .transpose()?
        .unwrap_or_default();
    check_git_op(&git, &args).map_err(|e| format!("{at}.git: {e}"))?;
    Ok(Setup {
        git,
        repo,
        files: m
            .get("files")
            .map(|f| str_map(f, &format!("{at}.files")))
            .transpose()?
            .unwrap_or_default(),
        message: m
            .get("message")
            .map(|s| string(s, &format!("{at}.message")))
            .transpose()?,
        args,
    })
}

/// The setup operations of `gitfix`. The runner dispatches on the same argument shapes that
/// [`check_git_op`] accepts, so a scenario that passes `validate` also runs.
pub const GIT_OPS: &[&str] = &[
    "init",
    "commit",
    "write",
    "branch",
    "checkout",
    "worktree",
    "merge",
    "delete",
    "move",
    "rebase",
    "cherry-pick",
    "reset",
];

/// Check that `op` names a setup operation and that `args` have a shape it accepts.
pub fn check_git_op(op: &str, args: &[String]) -> Res<()> {
    let shape = |ok: bool| {
        if ok {
            Ok(())
        } else {
            Err(format!(
                "`{op}` does not take {} argument(s) in that form",
                args.len()
            ))
        }
    };
    match op {
        "init" | "commit" | "write" => shape(args.is_empty()),
        "branch" | "merge" | "rebase" | "cherry-pick" => shape(args.len() == 1),
        "checkout" => shape(args.len() == 1 || (args.len() == 2 && args[0] == "-b")),
        "worktree" | "move" => shape(args.len() == 2),
        "delete" => shape(!args.is_empty()),
        "reset" => shape(
            args.len() == 1
                || (args.len() == 2 && ["--soft", "--mixed", "--hard"].contains(&args[0].as_str())),
        ),
        _ => Err(format!(
            "unknown setup operation `{op}` (one of {})",
            GIT_OPS.join(", ")
        )),
    }
}

fn parse_assert(v: &Value, at: &str) -> Res<Assertion> {
    let m = obj(v, at)?;
    only_keys(
        m,
        &[
            "exit",
            "stdout_contains",
            "stderr_contains",
            "json",
            "file",
            "exists",
            "contains",
            "db",
            "where",
            "count",
        ],
        at,
    )?;
    if m.is_empty() {
        return Err(format!("{at}: empty assertion"));
    }
    let key = |k: &str| format!("{at}.{k}");
    if let Some(e) = m.get("exit") {
        return Ok(Assertion::Exit(integer(e, &key("exit"))?));
    }
    if let Some(s) = m.get("stdout_contains") {
        return Ok(Assertion::StdoutContains(string(
            s,
            &key("stdout_contains"),
        )?));
    }
    if let Some(s) = m.get("stderr_contains") {
        return Ok(Assertion::StderrContains(string(
            s,
            &key("stderr_contains"),
        )?));
    }
    if let Some(j) = m.get("json") {
        let jm = obj(j, &key("json"))?;
        only_keys(jm, &["path", "equals"], &key("json"))?;
        let path = string(
            jm.get("path")
                .ok_or_else(|| format!("{at}.json: missing `path`"))?,
            &key("json.path"),
        )?;
        let equals = jm
            .get("equals")
            .cloned()
            .ok_or_else(|| format!("{at}.json: missing `equals`"))?;
        return Ok(Assertion::Json { path, equals });
    }
    if let Some(f) = m.get("file") {
        return Ok(Assertion::File {
            path: string(f, &key("file"))?,
            exists: m
                .get("exists")
                .map(|b| boolean(b, &key("exists")))
                .transpose()?
                .unwrap_or(true),
            contains: m
                .get("contains")
                .map(|s| string(s, &key("contains")))
                .transpose()?,
        });
    }
    if let Some(t) = m.get("db") {
        let count = m
            .get("count")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("{at}: db assertion needs a non-negative `count`"))?;
        return Ok(Assertion::Db {
            table: string(t, &key("db"))?,
            filter: m.get("where").cloned().unwrap_or(Value::Object(Map::new())),
            count,
        });
    }
    Err(format!("{at}: assertion has only modifier keys"))
}

fn parse_step(v: &Value, at: &str) -> Res<Step> {
    let m = obj(v, at)?;
    let kinds: Vec<&str> = ["cli", "mcp", "http", "drain", "assert"]
        .into_iter()
        .filter(|k| m.contains_key(*k))
        .collect();
    let [kind] = kinds.as_slice() else {
        return Err(format!(
            "{at}: expected exactly one of cli, mcp, http, drain, assert"
        ));
    };
    let save = |m: &Map<String, Value>| -> Res<Vec<(String, String)>> {
        m.get("save")
            .map(|s| str_map(s, &format!("{at}.save")))
            .transpose()
            .map(Option::unwrap_or_default)
    };
    match *kind {
        "cli" => {
            only_keys(
                m,
                &["cli", "json", "stdin", "env", "save", "expect_exit"],
                at,
            )?;
            Ok(Step::Cli {
                args: strings(&m["cli"], &format!("{at}.cli"))?,
                json: m
                    .get("json")
                    .map(|b| boolean(b, &format!("{at}.json")))
                    .transpose()?
                    .unwrap_or(false),
                stdin: m
                    .get("stdin")
                    .map(|s| string(s, &format!("{at}.stdin")))
                    .transpose()?,
                env: m
                    .get("env")
                    .map(|e| str_map(e, &format!("{at}.env")))
                    .transpose()?
                    .unwrap_or_default(),
                save: save(m)?,
                expect_exit: m
                    .get("expect_exit")
                    .map(|e| integer(e, &format!("{at}.expect_exit")))
                    .transpose()?,
            })
        }
        "mcp" => {
            only_keys(m, &["mcp", "args", "save"], at)?;
            save(m)?;
            Ok(Step::Mcp {
                tool: string(&m["mcp"], &format!("{at}.mcp"))?,
                args: m.get("args").cloned().unwrap_or(Value::Object(Map::new())),
                save: save(m)?,
            })
        }
        "http" => {
            only_keys(m, &["http", "path", "body", "save", "max_events"], at)?;
            let method = string(&m["http"], &format!("{at}.http"))?;
            if !["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&method.as_str()) {
                return Err(format!("{at}.http: unknown method `{method}`"));
            }
            let path = string(
                m.get("path")
                    .ok_or_else(|| format!("{at}: missing `path`"))?,
                &format!("{at}.path"),
            )?;
            let max_events = match m.get("max_events") {
                None => None,
                Some(n) => Some(
                    n.as_u64()
                        .and_then(|n| usize::try_from(n).ok())
                        .filter(|n| *n >= 1)
                        .ok_or_else(|| format!("{at}.max_events: expected an integer >= 1"))?,
                ),
            };
            Ok(Step::Http {
                method,
                path,
                body: m.get("body").cloned(),
                max_events,
                save: save(m)?,
            })
        }
        "drain" => {
            only_keys(m, &["drain"], at)?;
            let d = obj(&m["drain"], &format!("{at}.drain"))?;
            only_keys(d, &["max_jobs"], &format!("{at}.drain"))?;
            let max_jobs = match d.get("max_jobs") {
                None => 50,
                Some(n) => n
                    .as_u64()
                    .filter(|n| *n >= 1)
                    .ok_or_else(|| format!("{at}.drain.max_jobs: expected an integer >= 1"))?,
            };
            Ok(Step::Drain { max_jobs })
        }
        _ => {
            only_keys(m, &["assert"], at)?;
            Ok(Step::Assert(parse_assert(
                &m["assert"],
                &format!("{at}.assert"),
            )?))
        }
    }
}

/// Parse and validate one scenario document.
pub fn parse_scenario(text: &str) -> Result<Scenario, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    let m = obj(&v, "scenario")?;
    only_keys(
        m,
        &["id", "covers", "setup", "steps", "not_observable"],
        "scenario",
    )?;
    let need = |k: &str| m.get(k).ok_or_else(|| format!("scenario: missing `{k}`"));
    let id = string(need("id")?, "id")?;
    if !id_ok(&id) {
        return Err(format!("id: `{id}` does not match ^[a-z0-9][a-z0-9-]*$"));
    }
    let covers = strings(need("covers")?, "covers")?;
    if let Some(c) = covers.iter().find(|c| !cover_ok(c)) {
        return Err(format!("covers: `{c}` is not a contract id"));
    }
    let setup = match m.get("setup") {
        None => Vec::new(),
        Some(s) => s
            .as_array()
            .ok_or("setup: expected an array")?
            .iter()
            .enumerate()
            .map(|(i, x)| parse_setup(x, &format!("setup[{i}]")))
            .collect::<Res<_>>()?,
    };
    let raw_steps = need("steps")?
        .as_array()
        .ok_or("steps: expected an array")?;
    if raw_steps.is_empty() {
        return Err("steps: must not be empty".into());
    }
    let steps = raw_steps
        .iter()
        .enumerate()
        .map(|(i, x)| parse_step(x, &format!("steps[{i}]")))
        .collect::<Res<_>>()?;
    let not_observable = match m.get("not_observable") {
        None => Vec::new(),
        Some(n) => n
            .as_array()
            .ok_or("not_observable: expected an array")?
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let at = format!("not_observable[{i}]");
                let o = obj(x, &at)?;
                only_keys(o, &["case", "covered_by"], &at)?;
                let get = |k: &str| {
                    string(
                        o.get(k).ok_or_else(|| format!("{at}: missing `{k}`"))?,
                        &format!("{at}.{k}"),
                    )
                };
                Ok((get("case")?, get("covered_by")?))
            })
            .collect::<Res<_>>()?,
    };
    Ok(Scenario {
        id,
        covers,
        setup,
        steps,
        not_observable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "id": "stack-bisect-ejects-culprit",
      "covers": ["C135", "M16"],
      "setup": [{"git": "init", "repo": "api"},
                {"git": "commit", "repo": "api", "files": {"check.txt": "ok"}, "message": "base"}],
      "steps": [
        {"cli": ["init"]},
        {"cli": ["repo", "register", "{repo:api}"], "json": true},
        {"mcp": "task_create", "args": {"title": "Parent"}},
        {"http": "POST", "path": "/api/v1/ws/{ws}/stacks", "body": {"parent": "{key:1}"}},
        {"drain": {"max_jobs": 50}},
        {"assert": {"db": "stack_items", "where": {"state": "ejected"}, "count": 1}}
      ],
      "not_observable": []
    }"#;

    #[test]
    fn accepts_the_documented_example() {
        let s = parse_scenario(SAMPLE).unwrap();
        assert_eq!(s.steps.len(), 6);
        assert_eq!(s.setup.len(), 2);
    }

    #[test]
    fn http_steps_take_max_events_and_save() {
        let text = r#"{"id":"a","covers":[],"steps":[
          {"http": "GET", "path": "/api/v1/ws/default/events/stream", "max_events": 3,
           "save": {"first": "/0/data/seq"}},
          {"http": "POST", "path": "/x", "body": {"a": 1}}
        ]}"#;
        let s = parse_scenario(text).unwrap();
        assert_eq!(
            s.steps[0],
            Step::Http {
                method: "GET".into(),
                path: "/api/v1/ws/default/events/stream".into(),
                body: None,
                max_events: Some(3),
                save: vec![("first".into(), "/0/data/seq".into())],
            }
        );
        assert!(
            parse_scenario(
                r#"{"id":"a","covers":[],"steps":[{"http":"GET","path":"/","max_events":0}]}"#
            )
            .is_err()
        );
    }

    fn setup_of(op: &str, args: &[&str]) -> String {
        let args: Vec<String> = args.iter().map(|a| format!("\"{a}\"")).collect();
        format!(
            r#"{{"id":"a","covers":[],"setup":[{{"git":"{op}","repo":"r","args":[{}]}}],"steps":[{{"cli":["x"]}}]}}"#,
            args.join(",")
        )
    }

    #[test]
    fn setup_operations_take_the_shapes_the_runner_dispatches_on() {
        let ok: &[(&str, &[&str])] = &[
            ("init", &[]),
            ("commit", &[]),
            ("write", &[]),
            ("branch", &["feat"]),
            ("checkout", &["feat"]),
            ("checkout", &["-b", "feat"]),
            ("worktree", &["wt", "feat"]),
            ("merge", &["feat"]),
            ("delete", &["a.txt", "dir"]),
            ("move", &["a.txt", "dir/b.txt"]),
            ("rebase", &["main"]),
            ("cherry-pick", &["abc123"]),
            ("reset", &["HEAD~1"]),
            ("reset", &["--hard", "HEAD~1"]),
        ];
        for (op, args) in ok {
            let s = parse_scenario(&setup_of(op, args)).unwrap_or_else(|e| panic!("{op}: {e}"));
            assert_eq!(s.setup[0].git, *op);
            let want: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
            assert_eq!(s.setup[0].args, want);
        }
        let bad: &[(&str, &[&str])] = &[
            ("init", &["x"]),
            ("move", &["a.txt"]),
            ("delete", &[]),
            ("rebase", &[]),
            ("cherry-pick", &["a", "b"]),
            ("reset", &["--keep", "HEAD"]),
            ("reset", &["--hard", "HEAD", "x"]),
            ("squash", &[]),
        ];
        for (op, args) in bad {
            assert!(
                parse_scenario(&setup_of(op, args)).is_err(),
                "{op} {args:?} should be rejected"
            );
        }
    }

    #[test]
    fn rejects_unknown_keys_and_bad_ids() {
        assert!(parse_scenario(&SAMPLE.replace("\"covers\"", "\"cover\"")).is_err());
        assert!(parse_scenario(&SAMPLE.replace("stack-bisect-ejects-culprit", "Bad Id")).is_err());
        assert!(parse_scenario(&SAMPLE.replace("C135", "c135")).is_err());
        assert!(
            parse_scenario(r#"{"id":"a","covers":[],"steps":[{"cli":["x"],"bogus":1}]}"#).is_err()
        );
        assert!(parse_scenario(r#"{"id":"a","covers":[],"steps":[]}"#).is_err());
        assert!(
            parse_scenario(r#"{"id":"a","covers":[],"steps":[{"cli":[],"drain":{}}]}"#).is_err()
        );
    }
}
