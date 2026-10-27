//! Running one scenario against one implementation.
//!
//! An implementation is an executable plus arguments and environment. Every
//! run gets a fresh `WORKGRAPH_HOME` and a fresh temporary directory that also
//! holds the repositories the setup steps create.

use crate::gitfix;
use crate::http;
use crate::scenario::{Assertion, HeaderExpect, JsonExpect, Scenario, Setup, Step};
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
    /// Wall-clock window of the step in microseconds since the epoch: just before it started to
    /// just after it returned. The normaliser attributes timestamps to the step whose window
    /// holds them. `None` for steps that never ran and for replayed records.
    pub window: Option<(i64, i64)>,
}

/// Now, in microseconds since the epoch.
fn now_micros() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_micros()).unwrap_or(i64::MAX))
}

/// Runs `f` and stamps the record it returns with the wall-clock window it took.
fn windowed(f: impl FnOnce() -> StepRecord) -> StepRecord {
    let start = now_micros();
    let rec = f();
    StepRecord {
        window: Some((start, now_micros())),
        ..rec
    }
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
    /// The stdio MCP session of `mcp` steps, started by the first one and dropped with the context.
    mcp: Option<crate::mcp::StdioClient>,
    /// The `grants` the running MCP session was started with.
    mcp_grants: Vec<String>,
    /// The `serve` process, started by the first `http` step and killed when the run ends.
    server: Option<http::Server>,
    /// Directories that `PATH` prefixes of the run add (from the implementation and from steps).
    path_dirs: Vec<String>,
    /// The headers of the last `http` response, read by `header` assertions.
    last_headers: Option<Vec<(String, String)>>,
}

fn display(p: &Path) -> String {
    p.display().to_string()
}

pub(crate) fn env_for(
    imp: &Implementation,
    sb: &Sandbox,
    extra: &[(String, String)],
) -> Vec<(String, String)> {
    // HOME and USERPROFILE point into the sandbox, so a command that reads the user's settings
    // (`~/.claude/settings.json`) reads the sandbox's copy on every machine.
    let mut env = vec![
        ("WORKGRAPH_HOME".to_owned(), display(&sb.home)),
        ("HOME".to_owned(), display(&sb.home)),
        ("USERPROFILE".to_owned(), display(&sb.home)),
        ("COLUMNS".to_owned(), "120".to_owned()),
        ("NO_COLOR".to_owned(), "1".to_owned()),
        ("TMPDIR".to_owned(), display(&sb.tmp)),
        ("TMP".to_owned(), display(&sb.tmp)),
        ("TEMP".to_owned(), display(&sb.tmp)),
    ];
    env.extend(imp.env.iter().cloned());
    env.extend(extra.iter().cloned());
    merge_path_prefixes(env)
}

/// The separator of a `PATH` value on this platform.
fn path_separator() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

/// Every `PATH` entry in `env` is a prefix: the entries it names come first, then the `PATH` the
/// runner itself inherited, so a fake agent in a `--env PATH=<dir>` prefix is found while the
/// system tools still are. Both implementations resolve executables from the `PATH` they run
/// with, so the merged value is what `workgraph-demo-agent` is looked up through.
pub(crate) fn merge_path_prefixes(env: Vec<(String, String)>) -> Vec<(String, String)> {
    let (paths, mut rest): (Vec<_>, Vec<_>) = env
        .into_iter()
        .partition(|(k, _)| k.eq_ignore_ascii_case("PATH"));
    if paths.is_empty() {
        return rest;
    }
    let inherited = std::env::var("PATH").unwrap_or_default();
    let value = paths
        .iter()
        .map(|(_, v)| v.as_str())
        .chain(std::iter::once(inherited.as_str()))
        .filter(|v| !v.is_empty())
        .collect::<Vec<_>>()
        .join(path_separator());
    rest.push(("PATH".to_owned(), value));
    rest
}

/// The directories a `PATH` prefix adds, as written in `value`.
fn path_prefix_dirs(value: &str) -> Vec<String> {
    std::env::split_paths(value)
        .map(|p| p.display().to_string())
        .filter(|d| !d.is_empty())
        .collect()
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
            // The platform path separator, so a scenario that asserts paths runs on every OS.
            ("sep", None) => Ok(std::path::MAIN_SEPARATOR.to_string()),
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
        self.resolve_with(text, false)
    }

    /// As `resolve`, and with `escape` each substituted value is escaped as the text of a JSON
    /// string, so a Windows path substituted into JSON stays valid JSON.
    fn resolve_with(&self, text: &str, escape: bool) -> Result<String, String> {
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
                    let value = r?;
                    out.push_str(&if escape { json_escape(&value) } else { value });
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

/// The JSON document a command printed. In `--json` mode Python first prints a banner line
/// (`worker <id> started ...`), so when the whole output is not JSON the document starts at the
/// first later line that begins with `{`. `Null` when there is none.
fn json_document(stdout: &str) -> Value {
    let mut start = 0;
    loop {
        if let Ok(doc) = serde_json::from_str::<Value>(&stdout[start..]) {
            return doc;
        }
        match stdout[start..].find("\n{") {
            Some(p) => start += p + 1,
            None => return Value::Null,
        }
    }
}

/// `text` as the inside of a JSON string literal (the quotes are not included).
fn json_escape(text: &str) -> String {
    let quoted = serde_json::to_string(text).unwrap_or_default();
    quoted
        .strip_prefix('"')
        .and_then(|q| q.strip_suffix('"'))
        .unwrap_or_default()
        .to_owned()
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
        window: None,
    }
}

fn fail(mut r: StepRecord, note: String) -> StepRecord {
    r.status = Status::Failed;
    r.note = note;
    r
}

/// Run one typed git operation (CR-03) in the repository at `dir`. `ticks` is the commit tick of
/// that repository, so every operation on it continues one count of dates.
fn git_op(
    dir: PathBuf,
    ticks: &mut u64,
    op: &str,
    args: &[String],
    files: &[(String, String)],
    message: Option<&str>,
) -> Result<Outcome, String> {
    let mut repo = gitfix::Repo::new(dir, ticks);
    let message = message.unwrap_or("commit");
    match (op, args) {
        ("init", []) => repo.init(),
        ("commit", []) => repo.commit(message, files, true),
        ("write", []) => repo.write(files),
        ("branch", [name]) => repo.branch(name),
        ("checkout", [name]) => repo.checkout(name, false),
        ("checkout", [flag, name]) if flag == "-b" => repo.checkout(name, true),
        ("worktree", [path, branch]) => repo.worktree(path, branch),
        ("merge", [branch]) => repo.merge(branch),
        ("delete", paths) if !paths.is_empty() => repo.delete(paths),
        ("move", [from, to]) => repo.move_path(from, to),
        ("rebase", [onto]) => repo.rebase(onto),
        ("cherry-pick", [rev]) => repo.cherry_pick(rev),
        ("reset", [rev]) => repo.reset(None, rev),
        ("reset", [mode, rev]) => repo.reset(Some(mode), rev),
        (op, args) => Err(format!(
            "git `{op}` does not take {} argument(s)",
            args.len()
        )),
    }
}

/// Run one setup operation in the named repository of the sandbox.
fn setup_step(ctx: &mut Ctx<'_>, s: &Setup) -> Result<Outcome, String> {
    let dir = ctx.sb.repos.join(&s.repo);
    let ticks = ctx.git_ticks.entry(display(&dir)).or_insert(0);
    git_op(dir, ticks, &s.git, &s.args, &s.files, s.message.as_deref())
}

/// A directory for a step's `cwd`: as given when absolute, else under the repositories directory.
fn step_dir(sb: &Sandbox, cwd: &str) -> PathBuf {
    let p = PathBuf::from(cwd);
    if p.is_absolute() { p } else { sb.repos.join(p) }
}

/// Whether `path` is a directory strictly inside the sandbox home (no `..`, not the home itself).
fn inside_home(sb: &Sandbox, path: &Path) -> bool {
    path.is_absolute()
        && path != sb.home
        && path.starts_with(&sb.home)
        && !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// The files a file assertion path names: the path itself when it is a file, or, when its file name
/// has `*`, every file of the directory whose name matches (sorted).
fn matching_files(path: &Path) -> Vec<PathBuf> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if !name.contains('*') {
        return if path.is_file() {
            vec![path.to_path_buf()]
        } else {
            Vec::new()
        };
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            e.path().is_file()
                && e.file_name()
                    .to_str()
                    .is_some_and(|n| wildcard_match(name, n))
        })
        .map(|e| e.path())
        .collect();
    found.sort();
    found
}

/// Whether `name` matches `pattern`, where `*` stands for any run of characters.
fn wildcard_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let Some((first, tail)) = parts.split_first() else {
        return false;
    };
    let Some((last, middle)) = tail.split_last() else {
        return pattern == name;
    };
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// JSON equality with numbers compared by value, so an integer expectation matches a float the
/// Python implementation writes for the same number (`30` and `30.0`).
fn same_json(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(x), Some(y)) => x == y,
            _ => x == y,
        },
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same_json(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.get(k).is_some_and(|w| same_json(v, w)))
        }
        _ => a == b,
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
        Assertion::Json { path, expect } => {
            let doc: Value = serde_json::from_str(&last()?.stdout)
                .map_err(|e| format!("last output is not JSON: {e}"))?;
            let got = doc.pointer(path);
            match (expect, got) {
                (JsonExpect::Equals(want), got) => {
                    let want = ctx.resolve_value(want)?;
                    match got {
                        Some(got) if same_json(got, &want) => Ok(()),
                        Some(got) => Err(format!("{path} is {got}, wanted {want}")),
                        None => Err(format!("no value at {path}")),
                    }
                }
                (JsonExpect::NotEquals(want), got) => {
                    let want = ctx.resolve_value(want)?;
                    match got {
                        Some(got) if *got != want => Ok(()),
                        Some(_) => Err(format!("{path} is {want}, wanted anything else")),
                        None => Err(format!("no value at {path}")),
                    }
                }
                (JsonExpect::Absent, None) => Ok(()),
                (JsonExpect::Absent, Some(got)) => Err(format!("{path} is present: {got}")),
                (JsonExpect::Excludes(item), got) => {
                    let item = ctx.resolve_value(item)?;
                    match got {
                        Some(Value::Array(items)) if !items.iter().any(|x| same_json(x, &item)) => {
                            Ok(())
                        }
                        Some(Value::Array(_)) => Err(format!("{path} contains {item}")),
                        Some(_) => Err(format!("{path} is not an array")),
                        None => Err(format!("no value at {path}")),
                    }
                }
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
            // A path whose file name has `*` matches every file of that name pattern in its directory.
            let found = matching_files(&p);
            match (!found.is_empty(), exists) {
                (true, true) => match contains {
                    Some(c) => {
                        let want = ctx.resolve(c)?;
                        let hit = found.iter().any(|f| {
                            std::fs::read_to_string(f).is_ok_and(|text| text.contains(&want))
                        });
                        if hit {
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
        Assertion::Header { name, expect } => {
            let headers = ctx
                .last_headers
                .as_ref()
                .ok_or("assertion needs an earlier http step")?;
            let found = headers
                .iter()
                .find(|(h, _)| h.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str());
            match (expect, found) {
                (HeaderExpect::Absent, None) => Ok(()),
                (HeaderExpect::Absent, Some(v)) => Err(format!("header {name} is present: {v}")),
                (HeaderExpect::Equals(want), found) => {
                    let want = ctx.resolve(want)?;
                    match found {
                        Some(got) if got == want => Ok(()),
                        Some(got) => Err(format!("header {name} is {got:?}, wanted {want:?}")),
                        None => Err(format!("no header {name}, wanted {want:?}")),
                    }
                }
            }
        }
        Assertion::Db { .. } => unreachable!("db assertions are skipped by the caller"),
    })
}

/// The environment of a `cli` step with its placeholders resolved, like its arguments. A `PATH`
/// value also records the directories it adds, which the report names with a placeholder.
fn resolve_env(
    ctx: &mut Ctx<'_>,
    env: &[(String, String)],
) -> Result<Vec<(String, String)>, String> {
    env.iter()
        .map(|(k, v)| {
            let value = ctx.resolve(v)?;
            if k.eq_ignore_ascii_case("PATH") {
                ctx.path_dirs.extend(path_prefix_dirs(&value));
            }
            Ok((k.clone(), value))
        })
        .collect()
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
            // Stdin that is JSON takes its placeholders escaped, so a Windows path stays valid JSON.
            let escape = stdin
                .as_deref()
                .is_some_and(|s| serde_json::from_str::<Value>(s).is_ok());
            let stdin = match stdin
                .as_deref()
                .map(|s| ctx.resolve_with(s, escape))
                .transpose()
            {
                Ok(s) => s,
                Err(e) => return fail(rec, e),
            };
            let env = match resolve_env(ctx, env) {
                Ok(e) => e,
                Err(e) => return fail(rec, e),
            };
            let outcome = match ctx.cli(&resolved, *json, stdin.as_deref(), &env) {
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
        Step::Mcp {
            tool,
            args,
            grants,
            save,
        } => {
            let rec = record(index, "mcp", tool.clone());
            let args = match ctx.resolve_value(args) {
                Ok(a) => a,
                Err(e) => return fail(rec, e),
            };
            let (program, imp_args, env, dir) = mcp_launch(ctx, grants);
            let (rec, outcome) = crate::mcp::tool_step(
                &mut ctx.mcp,
                || crate::mcp::StdioClient::start(&program, &imp_args, &env, &dir),
                rec,
                tool,
                &args,
            );
            finish_mcp_step(ctx, rec, outcome, save)
        }
        Step::McpResource { uri, save } => {
            let rec = record(index, "mcp_resource", uri.clone());
            let uri = match ctx.resolve(uri) {
                Ok(u) => u,
                Err(e) => return fail(rec, e),
            };
            let (program, imp_args, env, dir) = mcp_launch(ctx, &[]);
            let (rec, outcome) = crate::mcp::resource_step(
                &mut ctx.mcp,
                || crate::mcp::StdioClient::start(&program, &imp_args, &env, &dir),
                rec,
                &uri,
            );
            finish_mcp_step(ctx, rec, outcome, save)
        }
        Step::Http {
            method,
            path,
            body,
            headers,
            max_events,
            expect_status,
            save,
        } => http_step(
            ctx,
            index,
            HttpCall {
                method,
                path,
                body: body.as_ref(),
                headers,
                max_events: *max_events,
                expect_status: *expect_status,
                save,
            },
        ),
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
                let doc = json_document(&o.stdout);
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
        Step::Git {
            git,
            cwd,
            args,
            files,
            message,
        } => {
            let rec = record(index, "git", format!("{git} {cwd}"));
            let dir = match ctx.resolve(cwd) {
                Ok(c) => step_dir(ctx.sb, &c),
                Err(e) => return fail(rec, e),
            };
            let args: Result<Vec<String>, String> = args.iter().map(|a| ctx.resolve(a)).collect();
            let args = match args {
                Ok(a) => a,
                Err(e) => return fail(rec, e),
            };
            let message = match message.as_deref().map(|m| ctx.resolve(m)).transpose() {
                Ok(m) => m,
                Err(e) => return fail(rec, e),
            };
            let ticks = ctx.git_ticks.entry(display(&dir)).or_insert(0);
            match git_op(dir, ticks, git, &args, files, message.as_deref()) {
                Ok(o) => {
                    let failed = o.exit != 0;
                    let rec = StepRecord {
                        exit: Some(o.exit),
                        stdout: o.stdout,
                        stderr: o.stderr,
                        ..rec
                    };
                    if failed {
                        fail(rec, format!("git {git} failed"))
                    } else {
                        rec
                    }
                }
                Err(e) => fail(rec, e),
            }
        }
        Step::Rmtree { path } => {
            let rec = record(index, "rmtree", path.clone());
            let path = match ctx.resolve(path) {
                Ok(p) => PathBuf::from(p),
                Err(e) => return fail(rec, e),
            };
            if !inside_home(ctx.sb, &path) {
                return fail(
                    rec,
                    format!("{} is not inside the sandbox home", path.display()),
                );
            }
            match std::fs::remove_dir_all(&path) {
                Ok(()) => StepRecord {
                    exit: Some(0),
                    ..rec
                },
                Err(e) => fail(rec, format!("remove {}: {e}", path.display())),
            }
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

/// One `http` step as written in a scenario, borrowed from the step.
struct HttpCall<'a> {
    method: &'a str,
    path: &'a str,
    body: Option<&'a Value>,
    headers: &'a [(String, String)],
    max_events: Option<usize>,
    expect_status: Option<i64>,
    save: &'a [(String, String)],
}

/// Run one `http` step against the sandbox's `serve`, which starts on first use. Unless the step
/// names its own `authorization` header, the bearer token is sent. The exit field holds the HTTP
/// status; the output is the body, or with `max_events` the captured events, one JSON object per
/// line. The response becomes the last output, so `json` assertions and `{last:...}` read the body.
/// Placeholders `{port}` and `{token}` resolve to the running server's port and API token.
fn http_step(ctx: &mut Ctx<'_>, index: usize, call: HttpCall<'_>) -> StepRecord {
    let HttpCall {
        method,
        path,
        body,
        headers,
        max_events,
        expect_status,
        save,
    } = call;
    let rec = record(index, "http", format!("{method} {path}"));
    if ctx.server.is_none() {
        match http::Server::start(ctx.imp, ctx.sb) {
            Ok(s) => {
                ctx.vars.insert("port".to_owned(), s.port.to_string());
                ctx.vars.insert("token".to_owned(), s.token.clone());
                ctx.server = Some(s);
            }
            Err(e) => return fail(rec, e),
        }
    }
    let path = match ctx.resolve(path) {
        Ok(p) => p,
        Err(e) => return fail(rec, e),
    };
    let body = match body.map(|b| ctx.resolve_value(b)).transpose() {
        Ok(b) => b,
        Err(e) => return fail(rec, e),
    };
    let mut user_headers = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        match ctx.resolve(value) {
            Ok(v) => user_headers.push((name.clone(), v)),
            Err(e) => return fail(rec, e),
        }
    }
    let has = |list: &[(String, String)], name: &str| {
        list.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
    };
    let (payload, json_body): (Option<Vec<u8>>, bool) = match body {
        None => (None, false),
        Some(Value::String(text)) => (Some(text.into_bytes()), false),
        Some(json) => (Some(json.to_string().into_bytes()), true),
    };
    let mut all: Vec<(String, String)> = Vec::new();
    let Some(server) = ctx.server.as_ref() else {
        return fail(rec, "serve did not start".to_owned());
    };
    if !has(&user_headers, "authorization") {
        all.push((
            "authorization".to_owned(),
            format!("Bearer {}", server.token),
        ));
    }
    all.extend(user_headers.iter().cloned());
    if json_body && !has(&user_headers, "content-type") {
        all.push(("content-type".to_owned(), "application/json".to_owned()));
    }
    let req = http::Request {
        method,
        path: &path,
        headers: &all,
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
    let status = i64::from(exchange.status);
    let rec = StepRecord {
        exit: Some(status),
        stdout: stdout.clone(),
        ..rec
    };
    scan_keys(&text, &mut ctx.keys);
    scan_ids(&text, &mut ctx.ids);
    ctx.last = Some(Outcome {
        exit: status,
        stdout,
        stderr: String::new(),
    });
    ctx.last_headers = Some(exchange.headers.clone());
    for (name, ptr) in save {
        match pointer_text(&text, ptr) {
            Ok(v) => {
                ctx.vars.insert(name.clone(), v);
            }
            Err(e) => return fail(rec, format!("save {name}: {e}")),
        }
    }
    if let Some(want) = expect_status
        && status != want
    {
        return fail(rec, format!("HTTP status {status}, wanted {want}"));
    }
    StepRecord {
        status: Status::Passed,
        ..rec
    }
}

/// How to start the implementation's MCP server: program, arguments, environment and directory.
type McpLaunch = (String, Vec<String>, Vec<(String, String)>, PathBuf);

/// The launch description for an `mcp` step with `grants`. A session started with other grants
/// is dropped first, so the next call starts a server with the requested permissions.
fn mcp_launch(ctx: &mut Ctx<'_>, grants: &[String]) -> McpLaunch {
    if ctx.mcp_grants != grants {
        ctx.mcp = None;
        ctx.mcp_grants = grants.to_vec();
    }
    (
        ctx.imp.program.clone(),
        crate::mcp::mcp_args(&ctx.imp.args, grants),
        env_for(ctx.imp, ctx.sb, &[]),
        ctx.sb.tmp.clone(),
    )
}

/// Remember an MCP reply the way a `cli` reply is remembered, then apply the step's `save`.
fn finish_mcp_step(
    ctx: &mut Ctx<'_>,
    rec: StepRecord,
    outcome: Option<Outcome>,
    save: &[(String, String)],
) -> StepRecord {
    let Some(o) = outcome else {
        return rec;
    };
    ctx.remember(o);
    for (name, ptr) in save {
        match ctx.last_pointer(ptr) {
            Ok(v) => {
                ctx.vars.insert(name.clone(), v);
            }
            Err(e) => return fail(rec, format!("save {name}: {e}")),
        }
    }
    rec
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
        mcp: None,
        mcp_grants: Vec::new(),
        server: None,
        path_dirs: Vec::new(),
        last_headers: None,
    };
    let mut steps = Vec::new();
    let mut setup_ok = true;
    for (i, s) in scenario.setup.iter().enumerate() {
        let start = now_micros();
        let rec = record(i, "setup", format!("git {} {}", s.git, s.repo));
        let rec = match setup_step(&mut ctx, s) {
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
        steps.push(StepRecord {
            index: None,
            window: Some((start, now_micros())),
            ..rec
        });
        if !setup_ok {
            break;
        }
    }
    if setup_ok {
        for (i, step) in scenario.steps.iter().enumerate() {
            steps.push(windowed(|| run_step(&mut ctx, i, step)));
        }
    }
    // Stop the MCP and HTTP servers before their sandbox is removed.
    let mut path_dirs = std::mem::take(&mut ctx.path_dirs);
    drop(ctx);
    for (_, value) in imp
        .env
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("PATH"))
    {
        path_dirs.extend(path_prefix_dirs(value));
    }
    // The directories a `PATH` prefix adds (a fake agent's directory) and the checkout the run
    // starts in are machine-specific: they are named by placeholders, most specific first.
    let mut roots = vec![
        (display(&sb.repos), "<tmp>/repos"),
        (display(&sb.home), "<home>"),
        (display(&sb.tmp), "<tmp>"),
        (display(&sb.root), "<tmp>"),
    ];
    roots.extend(path_dirs.into_iter().map(|d| (d, "<bin>")));
    if let Ok(checkout) = std::env::current_dir()
        && checkout.parent().is_some()
    {
        roots.push((display(&checkout), "<repo>"));
    }
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
            mcp: None,
            mcp_grants: Vec::new(),
            server: None,
            path_dirs: Vec::new(),
            last_headers: None,
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
        assert_eq!(
            ctx.resolve("{home}{sep}data").unwrap(),
            format!("{}{}data", display(&sb.home), std::path::MAIN_SEPARATOR)
        );
        let _ = std::fs::remove_dir_all(&sb.root);
    }

    #[test]
    fn every_setup_operation_runs_without_a_failed_step() {
        let text = r#"{"id":"setup-ops","covers":[],"setup":[
          {"git":"init","repo":"r"},
          {"git":"commit","repo":"r","files":{"a.txt":"a\n","b.txt":"b\n"},"message":"base"},
          {"git":"delete","repo":"r","args":["a.txt"]},
          {"git":"move","repo":"r","args":["b.txt","d/b.txt"]},
          {"git":"commit","repo":"r","message":"tidy"},
          {"git":"branch","repo":"r","args":["keep"]},
          {"git":"checkout","repo":"r","args":["-b","feat"]},
          {"git":"commit","repo":"r","files":{"f.txt":"f\n"},"message":"feature"},
          {"git":"checkout","repo":"r","args":["main"]},
          {"git":"cherry-pick","repo":"r","args":["feat"]},
          {"git":"reset","repo":"r","args":["--hard","HEAD~1"]},
          {"git":"write","repo":"r","files":{"w.txt":"w\n"}},
          {"git":"rebase","repo":"r","args":["keep"]}
        ],"steps":[{"cli":["--version"]},{"assert":{"exit":0}}]}"#;
        let scenario = crate::scenario::parse_scenario(text).unwrap();
        let imp = Implementation::from_command("git", "git", Vec::new()).unwrap();
        let run = run_scenario(&scenario, &imp).unwrap();
        assert_eq!(run.steps.len(), 15);
        let failures: Vec<&StepRecord> = run
            .steps
            .iter()
            .filter(|s| s.status == Status::Failed)
            .collect();
        assert!(
            failures.is_empty(),
            "{:?}",
            failures.iter().map(|s| &s.note).collect::<Vec<_>>()
        );
    }

    fn failures_of(text: &str) -> Vec<String> {
        let scenario = crate::scenario::parse_scenario(text).unwrap();
        let imp = Implementation::from_command("git", "git", Vec::new()).unwrap();
        run_scenario(&scenario, &imp)
            .unwrap()
            .steps
            .iter()
            .filter(|s| s.status == Status::Failed)
            .map(|s| format!("{}: {}", s.label, s.note))
            .collect()
    }

    #[test]
    fn git_steps_commit_in_a_named_directory_between_steps() {
        let failed = failures_of(
            r#"{"id":"git-step","covers":[],"setup":[
              {"git":"init","repo":"r"},
              {"git":"commit","repo":"r","files":{"a.txt":"a\n"},"message":"base"}
            ],"steps":[
              {"git":"commit","cwd":"{repo:r}","files":{"b.txt":"b\n"},"message":"second"},
              {"git":"branch","cwd":"{repo:r}","args":["feat"]},
              {"git":"checkout","cwd":"{repo:r}","args":["main"]},
              {"git":"merge","cwd":"{repo:r}","args":["feat"]},
              {"cli":["--version"]},
              {"assert":{"exit":0}}
            ]}"#,
        );
        assert!(failed.is_empty(), "{failed:?}");
        // A git step in a directory that does not exist fails the step, not the run.
        let failed = failures_of(
            r#"{"id":"git-step-missing","covers":[],"steps":[
              {"git":"commit","cwd":"{home}/no-such-dir","message":"x"}
            ]}"#,
        );
        assert_eq!(failed.len(), 1, "{failed:?}");
    }

    #[test]
    fn git_step_arguments_take_placeholders() {
        // `{ws}` always resolves to `default`, so the branch created under a placeholder name
        // can be checked out under its resolved name only if the arguments were resolved.
        let failed = failures_of(
            r#"{"id":"git-step-placeholders","covers":[],"setup":[
              {"git":"init","repo":"r"},
              {"git":"commit","repo":"r","files":{"a.txt":"a\n"},"message":"base"}
            ],"steps":[
              {"git":"branch","cwd":"{repo:r}","args":["{ws}-feat"]},
              {"git":"checkout","cwd":"{repo:r}","args":["default-feat"]},
              {"git":"commit","cwd":"{repo:r}","files":{"b.txt":"b\n"},"message":"on {ws}"}
            ]}"#,
        );
        assert!(failed.is_empty(), "{failed:?}");
    }

    #[test]
    fn rmtree_removes_a_tree_under_home_and_refuses_the_rest() {
        let failed = failures_of(
            r#"{"id":"rmtree-step","covers":[],"steps":[
              {"git":"init","cwd":"{home}/gone"},
              {"git":"write","cwd":"{home}/gone","files":{"x.txt":"x\n"}},
              {"rmtree":"{home}/gone"},
              {"assert":{"file":"{home}/gone/x.txt","exists":false}},
              {"rmtree":"{home}/gone"}
            ]}"#,
        );
        // Removing the tree twice is a failure of the second step, and the first one passed.
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(failed[0].contains("remove"), "{failed:?}");
        let sb = Sandbox::create().unwrap();
        let outside = sb.root.join("elsewhere");
        assert!(inside_home(&sb, &sb.home.join("data").join("worktrees")));
        assert!(!inside_home(&sb, &sb.home));
        assert!(!inside_home(&sb, &outside));
        assert!(!inside_home(&sb, &sb.home.join("..").join("elsewhere")));
        assert!(!inside_home(&sb, Path::new("relative/dir")));
        let _ = std::fs::remove_dir_all(&sb.root);
    }

    #[test]
    fn home_is_the_sandbox_and_path_prefixes_come_before_the_inherited_path() {
        let imp = Implementation::from_command("t", "x", Vec::new()).unwrap();
        let sb = Sandbox::create().unwrap();
        let env = env_for(&imp, &sb, &[]);
        let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("HOME").as_deref(), Some(display(&sb.home).as_str()));
        assert_eq!(
            get("USERPROFILE").as_deref(),
            Some(display(&sb.home).as_str())
        );
        assert_eq!(get("PATH"), None, "no PATH prefix, no PATH override");
        // A PATH prefix from the implementation and from a step: both come first, then the rest.
        let prefixed = env_for(
            &Implementation::from_command("t", "x", vec![("PATH".into(), "/fake/bin".into())])
                .unwrap(),
            &sb,
            &[("PATH".into(), "/step/bin".into())],
        );
        let path = prefixed
            .iter()
            .find(|(n, _)| n == "PATH")
            .map(|(_, v)| v.clone())
            .unwrap();
        let inherited = std::env::var("PATH").unwrap_or_default();
        let expected = ["/fake/bin", "/step/bin", inherited.as_str()]
            .into_iter()
            .filter(|v| !v.is_empty())
            .collect::<Vec<_>>()
            .join(path_separator());
        assert_eq!(path, expected);
        assert_eq!(prefixed.iter().filter(|(n, _)| n == "PATH").count(), 1);
        let _ = std::fs::remove_dir_all(&sb.root);
    }

    #[test]
    fn step_env_and_stdin_placeholders_resolve() {
        let imp = Implementation::from_command("t", "x", Vec::new()).unwrap();
        let sb = Sandbox::create().unwrap();
        let mut ctx = ctx_for(&imp, &sb);
        let home = display(&sb.home);
        let env = resolve_env(
            &mut ctx,
            &[
                ("WG_PROBE".into(), "{home}/probe".into()),
                ("PATH".into(), "{home}/bin".into()),
            ],
        )
        .unwrap();
        assert_eq!(env[0].1, format!("{home}/probe"));
        assert_eq!(env[1].1, format!("{home}/bin"));
        assert_eq!(ctx.path_dirs, [format!("{home}/bin")]);
        // Inside JSON a Windows-style path stays valid: its backslashes are escaped.
        assert_eq!(json_escape(r"C:\repo\x"), r"C:\\repo\\x");
        ctx.vars.insert("p".into(), r"C:\tmp\b.zip".into());
        let json = ctx.resolve_with(r#"{"path": "{var:p}"}"#, true).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["path"], r"C:\tmp\b.zip");
        let _ = std::fs::remove_dir_all(&sb.root);
    }

    #[test]
    fn json_numbers_compare_by_value() {
        assert!(same_json(&json!(30), &json!(30.0)));
        assert!(same_json(&json!({"a": [1, 2.0]}), &json!({"a": [1.0, 2]})));
        assert!(!same_json(&json!(30), &json!(31.0)));
        assert!(!same_json(&json!({"a": 1}), &json!({"a": 1, "b": 2})));
        assert!(!same_json(&json!("30"), &json!(30)));
    }

    #[test]
    fn wildcards_match_file_names_in_a_directory() {
        assert!(wildcard_match("*.json", "rec-1.json"));
        assert!(wildcard_match("rec-*.json", "rec-1.json"));
        assert!(wildcard_match("a*a", "aba"));
        assert!(!wildcard_match("a*a", "a"));
        assert!(!wildcard_match("rec-*.json", "rec-1.txt"));
        assert!(wildcard_match("plain", "plain"));
        assert!(!wildcard_match("plain", "plainer"));
        let sb = Sandbox::create().unwrap();
        std::fs::write(sb.home.join("spool-1.json"), "{}").unwrap();
        std::fs::write(sb.home.join("spool-2.json"), "{}").unwrap();
        std::fs::write(sb.home.join("other.txt"), "x").unwrap();
        let found = matching_files(&sb.home.join("spool-*.json"));
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(matching_files(&sb.home.join("missing-*.json")).is_empty());
        let _ = std::fs::remove_dir_all(&sb.root);
    }

    #[test]
    fn header_assertions_parse_and_check_the_last_response() {
        let good = r#"{"id":"h","covers":[],"steps":[
          {"assert":{"header":{"name":"Access-Control-Allow-Origin","absent":true}}},
          {"assert":{"header":{"name":"X-Frame-Options","equals":"DENY"}}}
        ]}"#;
        let scenario = crate::scenario::parse_scenario(good).unwrap();
        assert_eq!(scenario.steps.len(), 2);
        assert!(
            crate::scenario::parse_scenario(
                r#"{"id":"h","covers":[],"steps":[{"assert":{"header":{"name":"X"}}}]}"#
            )
            .is_err()
        );
        let imp = Implementation::from_command("t", "x", Vec::new()).unwrap();
        let sb = Sandbox::create().unwrap();
        let mut ctx = ctx_for(&imp, &sb);
        let absent = Assertion::Header {
            name: "Access-Control-Allow-Origin".into(),
            expect: crate::scenario::HeaderExpect::Absent,
        };
        assert!(run_assertion(&ctx, &absent).is_err(), "no response yet");
        ctx.last_headers = Some(vec![("x-frame-options".into(), "DENY".into())]);
        assert!(run_assertion(&ctx, &absent).unwrap().is_ok());
        let equals = Assertion::Header {
            name: "X-Frame-Options".into(),
            expect: crate::scenario::HeaderExpect::Equals("DENY".into()),
        };
        assert!(run_assertion(&ctx, &equals).unwrap().is_ok());
        let wrong = Assertion::Header {
            name: "X-Frame-Options".into(),
            expect: crate::scenario::HeaderExpect::Equals("SAMEORIGIN".into()),
        };
        assert!(run_assertion(&ctx, &wrong).unwrap().is_err());
        let _ = std::fs::remove_dir_all(&sb.root);
    }

    #[test]
    fn path_prefix_directories_are_named_by_a_placeholder() {
        let dir = std::env::temp_dir().join("wgconf-fake-bin");
        let text = format!(
            r#"{{"id":"bin","covers":[],"steps":[{{"cli":["--version"],"env":{{"PATH":"{}"}}}}]}}"#,
            display(&dir).replace('\\', "\\\\")
        );
        let scenario = crate::scenario::parse_scenario(&text).unwrap();
        let imp = Implementation::from_command("git", "git", Vec::new()).unwrap();
        let run = run_scenario(&scenario, &imp).unwrap();
        assert!(
            run.roots
                .iter()
                .any(|(d, p)| *p == "<bin>" && *d == display(&dir)),
            "{:?}",
            run.roots
        );
    }

    #[test]
    fn drain_reads_the_json_after_the_worker_banner() {
        let plain = r#"{"completed": 1, "failed": 0}"#;
        assert_eq!(json_document(plain)["completed"], 1);
        let banner = "worker DESKTOP-1-2-abcdef started (concurrency 1)\n{\"completed\": 2, \"failed\": 1}\n";
        let doc = json_document(banner);
        assert_eq!(doc["completed"], 2);
        assert_eq!(doc["failed"], 1);
        assert_eq!(json_document("worker started\nno json here"), Value::Null);
    }
}
