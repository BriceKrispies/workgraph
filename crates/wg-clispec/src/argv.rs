//! Argument rules that run before clap sees the command line.

/// The command line after the pre-clap rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Argv {
    /// Whether `--json` appeared.
    pub json: bool,
    /// The arguments without any `--json` (program name excluded).
    pub args: Vec<String>,
}

/// `main()` of `cli/main.py`: `--json` is accepted anywhere on the command line, including after
/// another option's value, and is removed before the command parser runs. Only the exact text
/// `--json` counts, and every occurrence is removed.
#[must_use]
pub fn split_json<I, S>(args: I) -> Argv
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut json = false;
    let mut kept = Vec::new();
    for arg in args {
        let arg = arg.into();
        if arg == "--json" {
            json = true;
        } else {
            kept.push(arg);
        }
    }
    Argv { json, args: kept }
}

/// Whether the command line is the Claude Code hook fast path (`claude hook ...`), which is
/// dispatched before the parser is built.
#[must_use]
pub fn is_hook_invocation(args: &[String]) -> bool {
    args.len() >= 2 && args[0] == "claude" && args[1] == "hook"
}
