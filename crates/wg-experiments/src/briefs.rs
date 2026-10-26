//! Brief templates for agent trials: port of `src/workgraph/experiments/briefs.py` (CORE-26,
//! GV-17).
//!
//! A template is immutable once it has been used to render a brief: changing its text means adding
//! a new template id, so that every recorded trial can be traced to the exact wording it was given.

/// Everything a brief template may print. Commands are argv lists, rendered with [`shlex_join`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BriefContext {
    /// Task key, for example `WG-1`.
    pub task_key: String,
    /// Trial id, for example `T-1`.
    pub trial_id: String,
    /// Repository name.
    pub repository: String,
    /// Path of the trial's worktree.
    pub worktree: String,
    /// Branch the trial commits on.
    pub branch: String,
    /// One-line subject of the task.
    pub subject: String,
    /// Free-text body of the task; trailing newlines are dropped when rendered.
    pub body: String,
    /// Test files that the oracle runs.
    pub test_paths: Vec<String>,
    /// Oracle commands, each an argv list.
    pub oracle: Vec<Vec<String>>,
    /// Gate commands, each an argv list.
    pub gates: Vec<Vec<String>>,
    /// Leaf-only: `(criterion, check)` pairs. `replay-v1` does not print them.
    pub acceptance: Vec<(String, String)>,
    /// Leaf-only: files the trial may change. `replay-v1` does not print them.
    pub files: Vec<String>,
    /// Leaf-only: out-of-scope items. `replay-v1` does not print them.
    pub out_of_scope: Vec<String>,
}

/// Why a brief could not be rendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BriefError {
    /// The template id is not one of [`TEMPLATE_IDS`].
    UnknownTemplate {
        /// The id that was asked for.
        template: String,
    },
}

impl BriefError {
    /// The message the Python `ValidationFailed` carries.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::UnknownTemplate { template } => format!(
                "unknown brief template {}; known templates: {}",
                py_repr(template),
                TEMPLATE_IDS.join(", ")
            ),
        }
    }
}

/// The known template ids, sorted, as the Python `TEMPLATES` keys are listed in error messages.
pub const TEMPLATE_IDS: [&str; 2] = ["leaf-v1", "replay-v1"];

/// Renders `context` with the named template. Unknown ids are rejected, never guessed.
///
/// # Errors
///
/// Returns [`BriefError::UnknownTemplate`] when `template_id` is not in [`TEMPLATE_IDS`].
pub fn render(template_id: &str, context: &BriefContext) -> Result<String, BriefError> {
    match template_id {
        "replay-v1" => Ok(replay_v1(context)),
        "leaf-v1" => Ok(leaf_v1(context)),
        other => Err(BriefError::UnknownTemplate {
            template: other.to_owned(),
        }),
    }
}

/// `shlex.join`: each argument quoted with [`shlex_quote`], joined by single spaces.
#[must_use]
pub fn shlex_join(argv: &[String]) -> String {
    argv.iter()
        .map(|part| shlex_quote(part))
        .collect::<Vec<String>>()
        .join(" ")
}

/// `shlex.quote`: an argument made only of ASCII word characters and `@%+=:,./-` is returned as
/// it is; the empty string becomes `''`; anything else is wrapped in single quotes.
#[must_use]
pub fn shlex_quote(part: &str) -> String {
    if part.is_empty() {
        return "''".to_owned();
    }
    let safe = part
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if safe {
        part.to_owned()
    } else {
        format!("'{}'", part.replace('\'', "'\"'\"'"))
    }
}

/// Python's `repr` of a string, for the error message: single quotes unless the text holds a
/// single quote and no double quote.
fn py_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let escaped: String = text
        .chars()
        .flat_map(|c| match c {
            '\\' => vec!['\\', '\\'],
            '\n' => vec!['\\', 'n'],
            '\r' => vec!['\\', 'r'],
            '\t' => vec!['\\', 't'],
            c if c == quote => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    format!("{quote}{escaped}{quote}")
}

/// Commands as indented lines, four spaces each; empty when there are no commands.
fn command_lines(commands: &[Vec<String>]) -> String {
    commands
        .iter()
        .map(|command| format!("    {}", shlex_join(command)))
        .collect::<Vec<String>>()
        .join("\n")
}

/// `- item` lines, or `- (none)` when there are none.
fn bullets(items: &[String]) -> String {
    if items.is_empty() {
        "- (none)".to_owned()
    } else {
        items
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<String>>()
            .join("\n")
    }
}

/// The body followed by a blank line, or nothing for an empty body.
fn body_block(body: &str) -> String {
    let body = body.trim_end_matches('\n');
    if body.is_empty() {
        String::new()
    } else {
        format!("{body}\n\n")
    }
}

fn gates_or_none(gates: &[Vec<String>]) -> String {
    let lines = command_lines(gates);
    if lines.is_empty() {
        "    (none)".to_owned()
    } else {
        lines
    }
}

fn tests_lines(tests: &[String]) -> String {
    tests
        .iter()
        .map(|path| format!("- {path}"))
        .collect::<Vec<String>>()
        .join("\n")
}

fn replay_v1(c: &BriefContext) -> String {
    let BriefContext {
        task_key,
        trial_id,
        repository,
        worktree,
        branch,
        subject,
        ..
    } = c;
    format!(
        "Task-ID: {task_key}\n\
         Trial-ID: {trial_id}\n\
         \n\
         You are working alone in the git worktree {worktree} (repository {repository}), \
         on branch {branch}. Work only there. Do not push or merge.\n\
         \n\
         ## Task\n\
         {subject}\n\
         \n\
         {body}## Tests that must pass\n\
         These test files were added or changed for this task and fail now:\n\
         {tests}\n\
         \n\
         Make them pass by changing the code, not the tests. Do not modify, delete or skip these test files.\n\
         \n\
         Run them with:\n\
         {oracle}\n\
         \n\
         ## Checks that must also pass\n\
         {gates}\n\
         \n\
         ## Rules\n\
         Follow the repository's own agent instructions (AGENTS.md or CLAUDE.md) if it has them. Commit your work on {branch} \
         when you are done.\n\
         \n\
         ## Report\n\
         Reply with what you changed, the output of the test command, and anything you could not finish.\n",
        body = body_block(&c.body),
        tests = tests_lines(&c.test_paths),
        oracle = command_lines(&c.oracle),
        gates = gates_or_none(&c.gates),
    )
}

fn leaf_v1(c: &BriefContext) -> String {
    let BriefContext {
        task_key,
        trial_id,
        repository,
        worktree,
        branch,
        subject,
        ..
    } = c;
    let done: Vec<String> = c
        .acceptance
        .iter()
        .map(|(criterion, check)| format!("{criterion} (checked by: {check})"))
        .collect();
    format!(
        "Task-ID: {task_key}\n\
         Trial-ID: {trial_id}\n\
         \n\
         You are working alone in the git worktree {worktree} (repository {repository}), \
         on branch {branch}. Work only there. Do not push or merge.\n\
         \n\
         ## Task\n\
         {subject}\n\
         \n\
         {body}## Done means\n\
         {done}\n\
         \n\
         ## Files you may change\n\
         {files}\n\
         \n\
         ## Out of scope\n\
         {out_of_scope}\n\
         \n\
         ## Tests that must pass\n\
         These tests were written for this task and fail now. Make them pass by changing the code, not the tests. \
         Do not modify, delete or skip them.\n\
         {oracle}\n\
         \n\
         ## Checks that must also pass\n\
         {gates}\n\
         \n\
         ## Rules\n\
         Follow the repository's own agent instructions (AGENTS.md or CLAUDE.md) if it has them. Change only the files \
         listed above. Commit your work on {branch} when you are done.\n\
         \n\
         ## Report\n\
         Reply with what you changed, the output of the test command, and anything you could not finish.\n",
        body = body_block(&c.body),
        done = bullets(&done),
        files = bullets(&c.files),
        out_of_scope = bullets(&c.out_of_scope),
        oracle = command_lines(&c.oracle),
        gates = gates_or_none(&c.gates),
    )
}
