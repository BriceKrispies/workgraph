//! Running one scenario against one implementation.
//!
//! An implementation is an executable plus arguments and environment. Every
//! run gets a fresh `WORKGRAPH_HOME` and a fresh temporary directory that also
//! holds the repositories the setup steps create.

use crate::gitfix;
use crate::http;
use crate::scenario::{Assertion, Scenario, Setup, Step};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// An implementation under test: `program args...` is prefixed to every command line.
#[derive(Debug, Clone)]
pub struct Implementation {
    pub name: String,
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl Implementation {
    /// Build from a command line such as `uv run workgraph` (split on whitespace).
    pub fn from_command(
        name: &str,
        command: &str,
        env: Vec<(String, String)>,
    ) -> Result<Self, String> {
        let mut parts = command.split_whitespace().map(str::to_owned);
        let program = parts
            .next()
            .ok_or_else(|| format!("{name}: empty command"))?;
        Ok(Self {
            name: name.to_owned(),
            program,
            args: parts.collect(),
            env,
        })
    }
}

/// How a step ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Passed,
    Failed,
    /// The step kind has no driver yet (mcp, http, db assertions).
    Skipped,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Passed => "passed",
            Status::Failed => "failed",
            Status::Skipped => "skipped",
        }
    }
}

/// The recorded outcome of one step (one line of the step log).
#[derive(Debug, Clone)]
pub struct StepRecord {
    pub index: Option<usize>,
    pub kind: &'static str,
    pub label: String,
    pub exit: Option<i64>,
    pub stdout: String,
    pub stderr: String,
    pub status: Status,
    pub note: String,
}

impl StepRecord {
    pub fn to_json(&self) -> Value {
        json!({
            "index": self.index, "kind": self.kind, "label": self.label, "exit": self.exit,
            "stdout": self.stdout, "stderr": self.stderr, "status": self.status.as_str(), "note": self.note,
        })
    }
}

/// The result of running a whole scenario.
#[derive(Debug, Clone)]
pub struct RunResult {
    pub scenario: String,
    pub implementation: String,
    /// Directories to replace with placeholders when comparing: `(path, placeholder)`.
    pub roots: Vec<(String, &'static str)>,
    pub steps: Vec<StepRecord>,
}

impl RunResult {
    pub fn failed(&self) -> bool {
        self.steps.iter().any(|s| s.status == Status::Failed)
    }

    /// The step log: one JSON document with every step's raw output.
    pub fn log_json(&self) -> Value {
        json!({
            "scenario": self.scenario, "implementation": self.implementation,
            "steps": self.steps.iter().map(StepRecord::to_json).collect::<Vec<_>>(),
        })
    }
}

static RUN_COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Sandbox {
    pub(crate) root: PathBuf,
    pub(crate) home: PathBuf,
    pub(crate) tmp: PathBuf,
    pub(crate) repos: PathBuf,
}

impl Sandbox {
    pub(crate) fn create() -> Result<Self, String> {
        let n = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let root = std::env::temp_dir().join(format!("wgconf-{}-{nanos}-{n}", std::process::id()));
        let (home, tmp, repos) = (root.join("home"), root.join("tmp"), root.join("repos"));
        for d in [&home, &tmp, &repos] {
            std::fs::create_dir_all(d).map_err(|e| format!("create {}: {e}", d.display()))?;
        }
        Ok(Self {
            root,
            home,
            tmp,
            repos,
        })
    }
}

pub(crate) struct Outcome {
    pub(crate) exit: i64,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

struct Ctx<'a> {
    imp: &'a Implementation,
    sb: &'a Sandbox,
    vars: BTreeMap<String, String>,
    keys: Vec<String>,
    ids: Vec<String>,
    last: Option<Outcome>,
    /// Commit tick per repository name, so each repository's dates count from its own first commit.
    git_ticks: BTreeMap<String, u64>,
    /// The `serve` process, started by the first `http` step and killed when the run ends.
    server: Option<http::Server>,
}

fn display(p: &Path) -> String {
    p.display().to_string()
}

pub(crate) fn env_for(
    imp: &Implementation,
    sb: &Sandbox,
    extra: &[(String, String)],
) -> Vec<(String, String)> {
    let mut env = vec![
        ("WORKGRAPH_HOME".to_owned(), display(&sb.home)),
        ("COLUMNS".to_owned(), "120".to_owned()),
        ("NO_COLOR".to_owned(), "1".to_owned()),
        ("TMPDIR".to_owned(), display(&sb.tmp)),
        ("TMP".to_owned(), display(&sb.tmp)),
        ("TEMP".to_owned(), display(&sb.tmp)),
    ];
    env.extend(imp.env.iter().cloned());
    env.extend(extra.iter().cloned());
    env
}

pub(crate) fn run_process(cmd: &mut Command, stdin: Option<&str>) -> Result<Outcome, String> {
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(text.as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("wait failed: {e}"))?;
    Ok(Outcome {
        exit: i64::from(out.status.code().unwrap_or(-1)),
        stdout: String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n"),
        stderr: String::from_utf8_lossy(&out.stderr).replace("\r\n", "\n"),
    })
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_uppercase() || c.is_ascii_digit()
}

/// Task keys (`WG-12`) in order of first appearance.
fn scan_keys(text: &str, into: &mut Vec<String>) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let boundary = i == 0 || !(chars[i - 1].is_ascii_alphanumeric() || chars[i - 1] == '_');
        if boundary && chars[i].is_ascii_uppercase() {
            let mut j = i;
            while j < chars.len() && is_key_char(chars[j]) {
                j += 1;
            }
            if j < chars.len() && chars[j] == '-' {
                let mut k = j + 1;
                while k < chars.len() && chars[k].is_ascii_digit() {
                    k += 1;
                }
                let after_ok =
                    k >= chars.len() || !(chars[k].is_ascii_alphanumeric() || chars[k] == '_');
                if k > j + 1 && after_ok {
                    let key: String = chars[i..k].iter().collect();
                    if !into.contains(&key) {
                        into.push(key);
                    }
                    i = k;
                    continue;
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
}

/// UUIDs in order of first appearance.
fn scan_ids(text: &str, into: &mut Vec<String>) {
    let b = text.as_bytes();
    let groups = [8usize, 4, 4, 4, 12];
    let mut i = 0;
    while i + 36 <= b.len() {
        let mut p = i;
        let mut ok = true;
        for (gi, g) in groups.iter().enumerate() {
            if gi > 0 {
                if b[p] != b'-' {
                    ok = false;
                    break;
                }
                p += 1;
            }
            if !b[p..p + g].iter().all(u8::is_ascii_hexdigit) {
                ok = false;
                break;
            }
            p += g;
        }
        if ok {
            let id = text[i..i + 36].to_ascii_lowercase();
            if !into.contains(&id) {
                into.push(id);
            }
            i += 36;
        } else {
            i += 1;
        }
    }
}

impl Ctx<'_> {
    fn lookup(&self, name: &str, arg: Option<&str>) -> Option<Result<String, String>> {
        let n = |s: &str| s.parse::<usize>().ok().filter(|n| *n >= 1);
        Some(match (name, arg) {
            ("ws", None) => Ok(self
                .vars
                .get("ws")
                .cloned()
                .unwrap_or_else(|| "default".to_owned())),
            ("home", None) => Ok(display(&self.sb.home)),
            ("tmp", None) => Ok(display(&self.sb.tmp)),
            ("repo", Some(r)) => Ok(display(&self.sb.repos.join(r))),
            ("key", Some(a)) => n(a)
                .and_then(|n| self.keys.get(n - 1).cloned())
                .ok_or_else(|| format!("placeholder {{key:{a}}}: no such key seen yet")),
            ("id", Some(a)) => n(a)
                .and_then(|n| self.ids.get(n - 1).cloned())
                .ok_or_else(|| format!("placeholder {{id:{a}}}: no such id seen yet")),
            ("var", Some(v)) => self
                .vars
                .get(v)
                .cloned()
                .ok_or_else(|| format!("placeholder {{var:{v}}}: not saved")),
            ("last", Some(ptr)) => self.last_pointer(ptr),
            _ => return None,
        })
    }

    fn last_pointer(&self, ptr: &str) -> Result<String, String> {
        let last = self
            .last
            .as_ref()
            .ok_or("placeholder {last:..}: no earlier cli step")?;
        pointer_text(&last.stdout, ptr)
    }

    /// Replace `{name}` and `{name:arg}` placeholders; other braces stay as they are.
    fn resolve(&self, text: &str) -> Result<String, String> {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let tail = &rest[open + 1..];
            let Some(close) = tail.find('}') else {
                out.push_str(&rest[open..]);
                return Ok(out);
            };
            let inner = &tail[..close];
            let (name, arg) = match inner.split_once(':') {
                Some((n, a)) => (n, Some(a)),
                None => (inner, None),
            };
            let plain = !name.is_empty() && name.bytes().all(|c| c.is_ascii_lowercase());
            match plain.then(|| self.lookup(name, arg)).flatten() {
                Some(r) => {
                    out.push_str(&r?);
                    rest = &tail[close + 1..];
                }
                None => {
                    out.push('{');
                    rest = tail;
                }
            }
        }
        out.push_str(rest);
        Ok(out)
    }

    fn resolve_value(&self, v: &Value) -> Result<Value, String> {
        Ok(match v {
            Value::String(s) => Value::String(self.resolve(s)?),
            Value::Array(a) => Value::Array(
                a.iter()
                    .map(|x| self.resolve_value(x))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Object(m) => {
                let mut out = Map::new();
                for (k, x) in m {
                    out.insert(k.clone(), self.resolve_value(x)?);
                }
                Value::Object(out)
            }
            other => other.clone(),
        })
    }

    fn remember(&mut self, o: Outcome) {
        scan_keys(&o.stdout, &mut self.keys);
        scan_ids(&o.stdout, &mut self.ids);
        self.last = Some(o);
    }

    fn cli(
        &self,
        args: &[String],
        json_mode: bool,
        stdin: Option<&str>,
        extra: &[(String, String)],
    ) -> Result<Outcome, String> {
        run_cli(self.imp, self.sb, args, json_mode, stdin, extra)
    }
}

/// Run one CLI command of `imp` in `sb`: the implementation's arguments, `--json` when asked,
/// then `args`, with the sandbox environment and no inherited `WORKGRAPH_*` variables.
pub(crate) fn run_cli(
    imp: &Implementation,
    sb: &Sandbox,
    args: &[String],
    json_mode: bool,
    stdin: Option<&str>,
    extra: &[(String, String)],
) -> Result<Outcome, String> {
    let mut cmd = Command::new(&imp.program);
    cmd.args(&imp.args);
    if json_mode {
        cmd.arg("--json");
    }
    cmd.args(args).current_dir(&sb.repos);
    for (k, _) in std::env::vars().filter(|(k, _)| k.starts_with("WORKGRAPH_")) {
        cmd.env_remove(k);
    }
    cmd.envs(env_for(imp, sb, extra));
    run_process(&mut cmd, stdin)
}

fn pointer_text(stdout: &str, ptr: &str) -> Result<String, String> {
    let doc: Value =
        serde_json::from_str(stdout).map_err(|e| format!("last output is not JSON: {e}"))?;
    match doc.pointer(ptr) {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(v) => Ok(v.to_string()),
        None => Err(format!(
            "no value at JSON pointer `{ptr}` in the last output"
        )),
    }
}

fn record(index: usize, kind: &'static str, label: String) -> StepRecord {
    StepRecord {
        index: Some(index),
        kind,
        label,
        exit: None,
        stdout: String::new(),
        stderr: String::new(),
        status: Status::Passed,
        note: String::new(),
    }
}

fn fail(mut r: StepRecord, note: String) -> StepRecord {
    r.status = Status::Failed;
    r.note = note;
    r
}

/// Run one typed git setup operation (CR-03) in the named repository.
fn git_step(ctx: &mut Ctx<'_>, s: &Setup) -> Result<Outcome, String> {
    let dir = ctx.sb.repos.join(&s.repo);
    let tick = ctx.git_ticks.entry(s.repo.clone()).or_insert(0);
    let mut repo = gitfix::Repo::new(dir, tick);
    let message = s.message.as_deref().unwrap_or("commit");
    match (s.git.as_str(), s.args.as_slice()) {
        ("init", []) => repo.init(),
        ("commit", []) => repo.commit(message, &s.files, true),
        ("write", []) => repo.write(&s.files),
        ("branch", [name]) => repo.branch(name),
        ("checkout", [name]) => repo.checkout(name, false),
        ("checkout", [flag, name]) if flag == "-b" => repo.checkout(name, true),
        ("worktree", [path, branch]) => repo.worktree(path, branch),
        ("merge", [branch]) => repo.merge(branch),
        (op, args) => Err(format!(
            "setup `{op}` does not take {} argument(s)",
            args.len()
        )),
    }
}

fn run_assertion(ctx: &Ctx<'_>, a: &Assertion) -> Result<Result<(), String>, String> {
    let last = || {
        ctx.last
            .as_ref()
            .ok_or_else(|| "assertion needs an earlier cli step".to_owned())
    };
    Ok(match a {
        Assertion::Exit(want) => {
            let got = last()?.exit;
            if got == *want {
                Ok(())
            } else {
                Err(format!("exit code {got}, wanted {want}"))
            }
        }
        Assertion::StdoutContains(s) => {
            let want = ctx.resolve(s)?;
            if last()?.stdout.contains(&want) {
                Ok(())
            } else {
                Err(format!("stdout does not contain {want:?}"))
            }
        }
        Assertion::StderrContains(s) => {
            let want = ctx.resolve(s)?;
            if last()?.stderr.contains(&want) {
                Ok(())
            } else {
                Err(format!("stderr does not contain {want:?}"))
            }
        }
        Assertion::Json { path, equals } => {
            let want = ctx.resolve_value(equals)?;
            let doc: Value = serde_json::from_str(&last()?.stdout)
                .map_err(|e| format!("last output is not JSON: {e}"))?;
            match doc.pointer(path) {
                Some(got) if *got == want => Ok(()),
                Some(got) => Err(format!("{path} is {got}, wanted {want}")),
                None => Err(format!("no value at {path}")),
            }
        }
        Assertion::File {
            path,
            exists,
            contains,
        } => {
            let p = PathBuf::from(ctx.resolve(path)?);
            let p = if p.is_absolute() {
                p
            } else {
                ctx.sb.home.join(p)
            };
            match (p.is_file(), exists) {
                (true, true) => match contains {
                    Some(c) => {
                        let want = ctx.resolve(c)?;
                        let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
                        if text.contains(&want) {
                            Ok(())
                        } else {
                            Err(format!("{} does not contain {want:?}", p.display()))
                        }
                    }
                    None => Ok(()),
                },
                (false, false) => Ok(()),
                (have, _) => Err(format!(
                    "{} {}",
                    p.display(),
                    if have { "exists" } else { "is missing" }
                )),
            }
        }
        Assertion::Db { .. } => unreachable!("db assertions are skipped by the caller"),
    })
}

fn run_step(ctx: &mut Ctx<'_>, index: usize, step: &Step) -> StepRecord {
    match step {
        Step::Cli {
            args,
            json,
            stdin,
            env,
            save,
            expect_exit,
        } => {
            let resolved: Result<Vec<String>, String> =
                args.iter().map(|a| ctx.resolve(a)).collect();
            let label = args.join(" ");
            let rec = record(index, "cli", label);
            let resolved = match resolved {
                Ok(r) => r,
                Err(e) => return fail(rec, e),
            };
            let stdin = match stdin.as_deref().map(|s| ctx.resolve(s)).transpose() {
                Ok(s) => s,
                Err(e) => return fail(rec, e),
            };
            let outcome = match ctx.cli(&resolved, *json, stdin.as_deref(), env) {
                Ok(o) => o,
                Err(e) => return fail(rec, e),
            };
            let mut rec = StepRecord {
                exit: Some(outcome.exit),
                stdout: outcome.stdout.clone(),
                stderr: outcome.stderr.clone(),
                ..rec
            };
            let exit = outcome.exit;
            ctx.remember(outcome);
            for (name, ptr) in save {
                match ctx.last_pointer(ptr) {
                    Ok(v) => {
                        ctx.vars.insert(name.clone(), v);
                    }
                    Err(e) => return fail(rec, format!("save {name}: {e}")),
                }
            }
            if let Some(want) = expect_exit
                && exit != *want
            {
                return fail(rec, format!("exit code {exit}, wanted {want}"));
            }
            rec.status = Status::Passed;
            rec
        }
        Step::Mcp { tool, .. } => skipped(
            index,
            "mcp",
            tool.clone(),
            "the MCP driver is not part of the runner skeleton",
        ),
        Step::Http {
            method,
            path,
            body,
            max_events,
            save,
        } => http_step(ctx, index, method, path, body.as_ref(), *max_events, save),
        Step::Drain { max_jobs } => {
            let mut rec = record(index, "drain", format!("max_jobs {max_jobs}"));
            let (mut done, mut failed) = (0u64, 0u64);
            while done + failed < *max_jobs {
                let o = match ctx.cli(
                    &["worker".into(), "run".into(), "--once".into()],
                    true,
                    None,
                    &[],
                ) {
                    Ok(o) => o,
                    Err(e) => return fail(rec, e),
                };
                if o.exit != 0 {
                    rec.exit = Some(o.exit);
                    rec.stderr = o.stderr;
                    return fail(rec, "worker run --once failed".to_owned());
                }
                let doc: Value = serde_json::from_str(&o.stdout).unwrap_or(Value::Null);
                let n = |k: &str| doc.get(k).and_then(Value::as_u64).unwrap_or(0);
                let (c, f) = (n("completed"), n("failed"));
                if c + f == 0 {
                    break;
                }
                done += c;
                failed += f;
            }
            rec.exit = Some(0);
            rec.stdout = format!("completed={done} failed={failed}\n");
            rec
        }
        Step::Assert(a) => {
            let rec = record(index, "assert", format!("{a:?}"));
            if matches!(a, Assertion::Db { .. }) {
                return skipped(
                    index,
                    "assert",
                    rec.label,
                    "the database driver is not part of the runner skeleton",
                );
            }
            match run_assertion(ctx, a) {
                Ok(Ok(())) => rec,
                Ok(Err(e)) | Err(e) => fail(rec, e),
            }
        }
    }
}

/// Run one `http` step against the sandbox's `serve`, which starts on first use. The bearer
/// token is sent on every step. The exit field holds the HTTP status; the output is the body,
/// or with `max_events` the captured events, one JSON object per line.
fn http_step(
    ctx: &mut Ctx<'_>,
    index: usize,
    method: &str,
    path: &str,
    body: Option<&Value>,
    max_events: Option<usize>,
    save: &[(String, String)],
) -> StepRecord {
    let rec = record(index, "http", format!("{method} {path}"));
    let path = match ctx.resolve(path) {
        Ok(p) => p,
        Err(e) => return fail(rec, e),
    };
    let body = match body.map(|b| ctx.resolve_value(b)).transpose() {
        Ok(b) => b,
        Err(e) => return fail(rec, e),
    };
    let (payload, mut headers): (Option<Vec<u8>>, Vec<(String, String)>) = match body {
        None => (None, Vec::new()),
        Some(Value::String(text)) => (Some(text.into_bytes()), Vec::new()),
        Some(json) => (
            Some(json.to_string().into_bytes()),
            vec![("content-type".to_owned(), "application/json".to_owned())],
        ),
    };
    if ctx.server.is_none() {
        match http::Server::start(ctx.imp, ctx.sb) {
            Ok(s) => ctx.server = Some(s),
            Err(e) => return fail(rec, e),
        }
    }
    let Some(server) = ctx.server.as_ref() else {
        return fail(rec, "serve did not start".to_owned());
    };
    headers.insert(
        0,
        (
            "authorization".to_owned(),
            format!("Bearer {}", server.token),
        ),
    );
    let req = http::Request {
        method,
        path: &path,
        headers: &headers,
        body: payload.as_deref(),
    };
    let exchange = match http::send(server.port, &req, max_events) {
        Ok(e) => e,
        Err(e) => return fail(rec, e),
    };
    let text = String::from_utf8_lossy(&exchange.body).replace("\r\n", "\n");
    let stdout = if max_events.is_some() {
        exchange
            .events
            .iter()
            .map(|e| format!("{e}\n"))
            .collect::<String>()
    } else {
        text.clone()
    };
    scan_keys(&text, &mut ctx.keys);
    scan_ids(&text, &mut ctx.ids);
    for (name, ptr) in save {
        match pointer_text(&text, ptr) {
            Ok(v) => {
                ctx.vars.insert(name.clone(), v);
            }
            Err(e) => {
                return fail(
                    StepRecord {
                        exit: Some(i64::from(exchange.status)),
                        stdout,
                        ..rec
                    },
                    format!("save {name}: {e}"),
                );
            }
        }
    }
    StepRecord {
        exit: Some(i64::from(exchange.status)),
        stdout,
        status: Status::Passed,
        ..rec
    }
}

fn skipped(index: usize, kind: &'static str, label: String, why: &str) -> StepRecord {
    StepRecord {
        status: Status::Skipped,
        note: why.to_owned(),
        ..record(index, kind, label)
    }
}

/// Run `scenario` against `imp` in a fresh sandbox, which is removed afterwards.
pub fn run_scenario(scenario: &Scenario, imp: &Implementation) -> Result<RunResult, String> {
    let sb = Sandbox::create()?;
    let mut ctx = Ctx {
        imp,
        sb: &sb,
        vars: BTreeMap::new(),
        keys: Vec::new(),
        ids: Vec::new(),
        last: None,
        git_ticks: BTreeMap::new(),
        server: None,
    };
    let mut steps = Vec::new();
    let mut setup_ok = true;
    for (i, s) in scenario.setup.iter().enumerate() {
        let rec = record(i, "setup", format!("git {} {}", s.git, s.repo));
        let rec = match git_step(&mut ctx, s) {
            Ok(o) if o.exit == 0 => rec,
            Ok(o) => {
                setup_ok = false;
                fail(
                    StepRecord {
                        exit: Some(o.exit),
                        stderr: o.stderr,
                        ..rec
                    },
                    "git failed".to_owned(),
                )
            }
            Err(e) => {
                setup_ok = false;
                fail(rec, e)
            }
        };
        steps.push(StepRecord { index: None, ..rec });
        if !setup_ok {
            break;
        }
    }
    if setup_ok {
        for (i, step) in scenario.steps.iter().enumerate() {
            steps.push(run_step(&mut ctx, i, step));
        }
    }
    drop(ctx);
    let roots = vec![
        (display(&sb.repos), "<tmp>/repos"),
        (display(&sb.home), "<home>"),
        (display(&sb.tmp), "<tmp>"),
        (display(&sb.root), "<tmp>"),
    ];
    let _ = std::fs::remove_dir_all(&sb.root);
    Ok(RunResult {
        scenario: scenario.id.clone(),
        implementation: imp.name.clone(),
        roots,
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx_for<'a>(imp: &'a Implementation, sb: &'a Sandbox) -> Ctx<'a> {
        Ctx {
            imp,
            sb,
            vars: BTreeMap::new(),
            keys: Vec::new(),
            ids: Vec::new(),
            last: None,
            git_ticks: BTreeMap::new(),
            server: None,
        }
    }

    #[test]
    fn placeholders_resolve_and_braces_survive() {
        let imp = Implementation::from_command("t", "x", Vec::new()).unwrap();
        let sb = Sandbox::create().unwrap();
        let mut ctx = ctx_for(&imp, &sb);
        scan_keys("created WG-7 and WG-9, then WG-7", &mut ctx.keys);
        scan_ids("id 123e4567-e89b-12d3-a456-426614174000 x", &mut ctx.ids);
        assert_eq!(ctx.keys, ["WG-7", "WG-9"]);
        assert_eq!(
            ctx.resolve("{key:2}/{ws}/{id:1}").unwrap(),
            "WG-9/default/123e4567-e89b-12d3-a456-426614174000"
        );
        assert_eq!(
            ctx.resolve(r#"{"a": {x}} {unknown:1}"#).unwrap(),
            r#"{"a": {x}} {unknown:1}"#
        );
        assert!(ctx.resolve("{key:3}").is_err());
        let _ = std::fs::remove_dir_all(&sb.root);
    }
}
