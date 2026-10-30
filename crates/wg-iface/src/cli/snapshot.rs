//! The command snapshot `contracts/cli.json` (contract C-series, plan 5.5): every leaf command of
//! the Python CLI with its arguments and options, as Click reports them.
//!
//! The snapshot drives two things: the placeholders for commands no task has implemented yet
//! (they parse like the real command and then report themselves), and the contract-coverage test
//! that compares a clap tree against it for the groups listed as implemented.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::Value as Json;
use wg_types::pyval::flag;

/// Whether a parameter is a positional argument or an option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Argument,
    Option,
}

/// One parameter of a command, as far as a clap tree can express and compare it.
///
/// The Click type (`int`, `path`, `datetime`...) and the default are not part of this: clap
/// validates values through value parsers that the snapshot does not describe, and the
/// scenarios cover those.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParamSpec {
    pub kind: Kind,
    /// The argument's name; options have none.
    pub name: Option<String>,
    /// The option's spellings, in Click's order (`--workspace`, `-w`). Empty for arguments.
    pub opts: Vec<String>,
    /// The negated spellings of a boolean flag (`--no-persist`).
    pub secondary_opts: Vec<String>,
    pub is_flag: bool,
    pub multiple: bool,
    pub required: bool,
    /// `1`, or `-1` for a variadic argument. `None` for options.
    pub nargs: Option<i64>,
    pub envvar: Option<String>,
    pub choices: Option<Vec<String>>,
}

impl ParamSpec {
    /// The key a parameter is compared by: the argument's name, or the option's first long
    /// spelling.
    #[must_use]
    pub fn key(&self) -> String {
        match self.kind {
            Kind::Argument => self.name.clone().unwrap_or_default(),
            Kind::Option => self
                .opts
                .iter()
                .find(|opt| opt.starts_with("--"))
                .or_else(|| self.opts.first())
                .cloned()
                .unwrap_or_default(),
        }
    }
}

/// A leaf command: its path below the root (`["task", "list"]`) and its parameters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSpec {
    pub path: Vec<String>,
    pub params: Vec<ParamSpec>,
}

impl CommandSpec {
    /// The command as a user types it: `workgraph task list`.
    #[must_use]
    pub fn display(&self) -> String {
        display_path(&self.path)
    }
}

/// `workgraph a b` for the path `["a", "b"]`.
#[must_use]
pub fn display_path(path: &[String]) -> String {
    let mut text = String::from("workgraph");
    for part in path {
        text.push(' ');
        text.push_str(part);
    }
    text
}

/// What an unimplemented command prints (as the `fail` message, exit code 1).
#[must_use]
pub fn not_implemented_message(path: &[String]) -> String {
    format!(
        "`{}` is not implemented in this build of the Rust rewrite yet",
        display_path(path)
    )
}

/// The parsed snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub root_options: Vec<ParamSpec>,
    pub commands: Vec<CommandSpec>,
    /// The top-level command and group names, in the CLI's order.
    pub top_level: Vec<String>,
}

/// The snapshot text is not the expected shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotError(pub String);

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid CLI snapshot: {}", self.0)
    }
}

impl std::error::Error for SnapshotError {}

fn bad<T>(what: &str) -> Result<T, SnapshotError> {
    Err(SnapshotError(what.to_owned()))
}

fn strings(value: Option<&Json>) -> Vec<String> {
    value
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn param(value: &Json) -> Result<ParamSpec, SnapshotError> {
    let Some(map) = value.as_object() else {
        return bad("a parameter is not an object");
    };
    let kind = match map.get("kind").and_then(Json::as_str) {
        Some("argument") => Kind::Argument,
        Some("option") => Kind::Option,
        _ => return bad("a parameter has no kind"),
    };
    let flag = |key: &str| flag(map.get(key));
    Ok(ParamSpec {
        kind,
        name: map.get("name").and_then(Json::as_str).map(str::to_owned),
        opts: strings(map.get("opts")),
        secondary_opts: strings(map.get("secondary_opts")),
        is_flag: flag("is_flag"),
        multiple: flag("multiple"),
        required: flag("required"),
        nargs: map.get("nargs").and_then(Json::as_i64),
        envvar: map.get("envvar").and_then(Json::as_str).map(str::to_owned),
        choices: map
            .get("choices")
            .filter(|choices| choices.is_array())
            .map(|choices| strings(Some(choices))),
    })
}

fn params(value: Option<&Json>) -> Result<Vec<ParamSpec>, SnapshotError> {
    value.and_then(Json::as_array).map_or_else(
        || bad("missing parameter list"),
        |items| items.iter().map(param).collect(),
    )
}

impl Snapshot {
    /// Parses the text of `contracts/cli.json`.
    pub fn parse(text: &str) -> Result<Snapshot, SnapshotError> {
        let root: Json =
            serde_json::from_str(text).map_err(|err| SnapshotError(err.to_string()))?;
        let Some(entries) = root.get("commands").and_then(Json::as_array) else {
            return bad("no `commands` list");
        };
        let mut commands = Vec::with_capacity(entries.len());
        for entry in entries {
            let Some(name) = entry.get("command").and_then(Json::as_str) else {
                return bad("a command has no name");
            };
            let mut words = name.split_whitespace();
            if words.next() != Some("workgraph") {
                return bad("a command does not start with `workgraph`");
            }
            commands.push(CommandSpec {
                path: words.map(str::to_owned).collect(),
                params: params(entry.get("params"))?,
            });
        }
        Ok(Snapshot {
            root_options: params(root.get("root_options"))?,
            commands,
            top_level: strings(root.get("top_level_names")),
        })
    }

    /// The leaf commands of the top-level command or group `name`.
    #[must_use]
    pub fn group(&self, name: &str) -> Vec<&CommandSpec> {
        self.commands
            .iter()
            .filter(|command| command.path.first().map(String::as_str) == Some(name))
            .collect()
    }
}

/// How `actual` differs from `expected`, one line per difference; empty when they agree.
///
/// Commands are matched by path. Arguments are compared in order; options by their first long
/// spelling, whatever their order.
#[must_use]
pub fn diff_commands(expected: &[&CommandSpec], actual: &[CommandSpec]) -> Vec<String> {
    let mut lines = Vec::new();
    let found: BTreeMap<&[String], &CommandSpec> = actual
        .iter()
        .map(|command| (command.path.as_slice(), command))
        .collect();
    for want in expected {
        match found.get(want.path.as_slice()) {
            None => lines.push(format!("{}: missing", want.display())),
            Some(have) => diff_params(want, have, &mut lines),
        }
    }
    let wanted: Vec<&[String]> = expected.iter().map(|c| c.path.as_slice()).collect();
    for command in actual {
        if !wanted.contains(&command.path.as_slice()) {
            lines.push(format!("{}: not in the snapshot", command.display()));
        }
    }
    lines
}

fn split(spec: &CommandSpec) -> (Vec<&ParamSpec>, Vec<&ParamSpec>) {
    spec.params.iter().partition(|p| p.kind == Kind::Argument)
}

fn diff_params(want: &CommandSpec, have: &CommandSpec, lines: &mut Vec<String>) {
    let name = want.display();
    let (want_args, want_opts) = split(want);
    let (have_args, have_opts) = split(have);
    if want_args.len() == have_args.len() {
        for (w, h) in want_args.iter().zip(&have_args) {
            if w != h {
                lines.push(format!(
                    "{name}: argument {}: expected {w:?}, found {h:?}",
                    w.key()
                ));
            }
        }
    } else {
        lines.push(format!(
            "{name}: expected {} arguments, found {}",
            want_args.len(),
            have_args.len()
        ));
    }
    let have_by_key: BTreeMap<String, &ParamSpec> =
        have_opts.iter().map(|p| (p.key(), *p)).collect();
    for w in &want_opts {
        match have_by_key.get(&w.key()) {
            None => lines.push(format!("{name}: option {} missing", w.key())),
            Some(h) if h != w => {
                lines.push(format!(
                    "{name}: option {}: expected {w:?}, found {h:?}",
                    w.key()
                ));
            }
            Some(_) => {}
        }
    }
    let want_keys: Vec<String> = want_opts.iter().map(|p| p.key()).collect();
    for h in &have_opts {
        if !want_keys.contains(&h.key()) {
            lines.push(format!("{name}: option {} not in the snapshot", h.key()));
        }
    }
}
