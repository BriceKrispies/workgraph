//! The clap command tree, and its conversion to and from the snapshot's parameter model.
//!
//! Click shows `--help` only (no `-h`) and has no `help` subcommand; [`command`] builds a
//! command that way. Typer prints a group's help when it is run without a subcommand
//! (`no_args_is_help`), which is [`clap::Command::arg_required_else_help`].

use crate::cli::{CommandSpec, Kind, ParamSpec, Snapshot};
use clap::builder::PossibleValuesParser;
use clap::{Arg, ArgAction, Command};

/// The snapshot of the Python CLI's commands, embedded at build time.
pub const SNAPSHOT_JSON: &str = include_str!("../../../../contracts/cli.json");

/// The help of the root command.
const ROOT_ABOUT: &str = "Workgraph - local-first engineering control plane for agents and humans.";

/// Suffix of the hidden twin that carries a boolean flag's negated spelling (`--no-x`).
const SECONDARY: &str = "__secondary";

/// A command with Click's help conventions: `--help` only, no `help` subcommand.
#[must_use]
pub fn command(name: &str) -> Command {
    Command::new(name.to_owned())
        .disable_help_flag(true)
        .disable_help_subcommand(true)
        .disable_version_flag(true)
        .arg(
            Arg::new("help")
                .long("help")
                .help("Show this message and exit.")
                .action(ArgAction::Help),
        )
}

/// A command that only groups subcommands: running it alone prints its help.
fn group_command(name: &str) -> Command {
    command(name)
        .subcommand_required(true)
        .arg_required_else_help(true)
}

fn upper(text: &str) -> String {
    text.trim_start_matches('-')
        .to_uppercase()
        .replace('-', "_")
}

/// The clap arguments for one snapshot parameter (a flag with a negated spelling takes two).
#[must_use]
pub fn to_args(param: &ParamSpec) -> Vec<Arg> {
    let key = param.key();
    if param.kind == Kind::Argument {
        let mut arg = Arg::new(key.clone())
            .value_name(upper(&key))
            .required(param.required);
        arg = if param.nargs == Some(-1) {
            arg.num_args(1..).action(ArgAction::Append)
        } else {
            arg.num_args(1).action(ArgAction::Set)
        };
        return vec![arg];
    }
    let mut arg = Arg::new(key.clone()).required(param.required);
    for opt in &param.opts {
        if let Some(long) = opt.strip_prefix("--") {
            arg = arg.long(long.to_owned());
        } else if let Some(short) = opt.strip_prefix('-').and_then(|s| s.chars().next()) {
            arg = arg.short(short);
        }
    }
    if param.is_flag {
        arg = arg.action(ArgAction::SetTrue);
        let mut args = vec![arg];
        if let Some(negated) = param
            .secondary_opts
            .iter()
            .find_map(|opt| opt.strip_prefix("--"))
        {
            let twin_id = format!("{key}{SECONDARY}");
            args[0] = args[0].clone().overrides_with(twin_id.clone());
            args.push(
                Arg::new(twin_id)
                    .long(negated.to_owned())
                    .action(ArgAction::SetTrue)
                    .overrides_with(key)
                    .hide(true),
            );
        }
        return args;
    }
    // Click takes the token after a value-taking option as its value whatever it looks like, so
    // `-d '- step one'` and `--arg --mode` are values, not options.
    arg = arg
        .value_name(upper(&key))
        .num_args(1)
        .allow_hyphen_values(true)
        .action(if param.multiple {
            ArgAction::Append
        } else {
            ArgAction::Set
        });
    if let Some(var) = &param.envvar {
        arg = arg.env(var.clone());
    }
    if let Some(choices) = &param.choices {
        arg = arg.value_parser(PossibleValuesParser::new(choices.clone()));
    }
    vec![arg]
}

/// One node of the tree being built from the snapshot.
#[derive(Default)]
struct Node {
    children: Vec<(String, Node)>,
    params: Option<Vec<ParamSpec>>,
}

impl Node {
    fn insert(&mut self, path: &[String], params: &[ParamSpec]) {
        let Some((first, rest)) = path.split_first() else {
            self.params = Some(params.to_vec());
            return;
        };
        let at = match self.children.iter().position(|(name, _)| name == first) {
            Some(at) => at,
            None => {
                self.children.push((first.clone(), Node::default()));
                self.children.len() - 1
            }
        };
        self.children[at].1.insert(rest, params);
    }

    fn command(&self, name: &str) -> Command {
        match &self.params {
            Some(params) if self.children.is_empty() => {
                let mut cmd = command(name);
                for arg in params.iter().flat_map(to_args) {
                    cmd = cmd.arg(arg);
                }
                cmd
            }
            _ => {
                let mut cmd = group_command(name);
                for (child, node) in &self.children {
                    cmd = cmd.subcommand(node.command(child));
                }
                cmd
            }
        }
    }
}

/// The command for the top-level command or group `name`, built from the snapshot.
#[must_use]
pub fn placeholder(snapshot: &Snapshot, name: &str) -> Command {
    let mut node = Node::default();
    for spec in snapshot.group(name) {
        node.insert(&spec.path[1..], &spec.params);
    }
    node.command(name)
}

/// The command `name` as the snapshot declares it, for a command whose parameters need no
/// hand-written shape (a snapshot that does not parse leaves a bare command).
#[must_use]
pub fn snapshot_command(name: &str) -> Command {
    match Snapshot::parse(SNAPSHOT_JSON) {
        Ok(snapshot) => placeholder(&snapshot, name),
        Err(_) => command(name),
    }
}

/// The root command: the global options of the snapshot, `--version`, and every top-level
/// command: `implemented` gives the hand-written command of an implemented group, and every
/// other name is built from the snapshot.
#[must_use]
pub fn root(snapshot: &Snapshot, implemented: &dyn Fn(&str) -> Option<Command>) -> Command {
    let mut cmd = group_command("workgraph")
        .about(ROOT_ABOUT)
        .version(env!("CARGO_PKG_VERSION"))
        .arg(
            Arg::new("version")
                .long("version")
                .action(ArgAction::Version),
        );
    for arg in snapshot.root_options.iter().flat_map(to_args) {
        cmd = cmd.arg(arg);
    }
    for name in &snapshot.top_level {
        cmd = cmd.subcommand(match implemented(name) {
            Some(command) => command,
            None => placeholder(snapshot, name),
        });
    }
    cmd
}

/// Every leaf command below `cmd` (excluding `cmd` itself) as the snapshot's model, for the
/// contract-coverage comparison.
#[must_use]
pub fn extract(cmd: &Command) -> Vec<CommandSpec> {
    let mut out = Vec::new();
    for sub in cmd.get_subcommands() {
        collect(sub, &mut vec![sub.get_name().to_owned()], &mut out);
    }
    out
}

/// Like [`extract`], for a command that is itself the group `path` (so `["task"]` for `task`).
#[must_use]
pub fn extract_group(cmd: &Command) -> Vec<CommandSpec> {
    let mut out = Vec::new();
    collect(cmd, &mut vec![cmd.get_name().to_owned()], &mut out);
    out
}

fn collect(cmd: &Command, path: &mut Vec<String>, out: &mut Vec<CommandSpec>) {
    if cmd.has_subcommands() {
        for sub in cmd.get_subcommands() {
            path.push(sub.get_name().to_owned());
            collect(sub, path, out);
            path.pop();
        }
        return;
    }
    out.push(CommandSpec {
        path: path.clone(),
        params: params_of(cmd),
    });
}

/// The parameters of a clap command in the snapshot's model.
#[must_use]
pub fn params_of(cmd: &Command) -> Vec<ParamSpec> {
    let skipped = |id: &str| id == "help" || id == "version" || id.ends_with(SECONDARY);
    cmd.get_arguments()
        .filter(|arg| !skipped(arg.get_id().as_str()))
        .map(|arg| {
            let id = arg.get_id().as_str();
            let action = arg.get_action();
            let is_flag = matches!(action, ArgAction::SetTrue | ArgAction::SetFalse);
            if arg.is_positional() {
                let variadic = arg.get_num_args().is_some_and(|r| r.max_values() > 1);
                return ParamSpec {
                    kind: Kind::Argument,
                    name: Some(id.to_owned()),
                    opts: Vec::new(),
                    secondary_opts: Vec::new(),
                    is_flag: false,
                    multiple: false,
                    required: arg.is_required_set(),
                    nargs: Some(if variadic { -1 } else { 1 }),
                    envvar: None,
                    choices: None,
                };
            }
            let mut opts = Vec::new();
            if let Some(long) = arg.get_long() {
                opts.push(format!("--{long}"));
            }
            if let Some(short) = arg.get_short() {
                opts.push(format!("-{short}"));
            }
            let twin = format!("{id}{SECONDARY}");
            let secondary_opts = cmd
                .get_arguments()
                .find(|other| other.get_id().as_str() == twin)
                .and_then(Arg::get_long)
                .map(|long| vec![format!("--{long}")])
                .unwrap_or_default();
            let choices: Vec<String> = arg
                .get_possible_values()
                .iter()
                .map(|value| value.get_name().to_owned())
                .collect();
            ParamSpec {
                kind: Kind::Option,
                name: None,
                opts,
                secondary_opts,
                is_flag,
                multiple: matches!(action, ArgAction::Append),
                required: arg.is_required_set(),
                nargs: None,
                envvar: arg.get_env().map(|var| var.to_string_lossy().into_owned()),
                choices: if choices.is_empty() {
                    None
                } else {
                    Some(choices)
                },
            }
        })
        .collect()
}
