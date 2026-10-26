//! `CLIState` of `cli/common.py`: the global options, the lazily opened application and the
//! output helpers.

use std::io::Write;
use std::path::PathBuf;

use wg_clispec::CliFailure;
use wg_json::{Value, cli_json};
use wg_render::Console;
use wg_runtime::{App, Level, LogSettings, configure_logging};

use crate::output::{console, write_to};

/// What one `workgraph` invocation knows: `--json`, `--home`, `--workspace`, the open
/// application and where output goes.
pub struct CliState {
    pub json: bool,
    pub home: Option<PathBuf>,
    pub workspace: Option<String>,
    app: Option<App>,
    stdout: Box<dyn Write>,
    stderr: Box<dyn Write>,
    /// The console for standard output, read from the terminal once.
    pub console: Console,
}

impl CliState {
    /// A state writing to the given streams.
    #[must_use]
    pub fn new(json: bool, stdout: Box<dyn Write>, stderr: Box<dyn Write>) -> Self {
        Self {
            json,
            home: None,
            workspace: None,
            app: None,
            stdout,
            stderr,
            // The terminal does not change under a running command: read it once.
            console: console(false),
        }
    }

    /// `STATE.app()`: opens the application on first use.
    pub fn app(&mut self) -> Result<&mut App, CliFailure> {
        if self.app.is_none() {
            self.app = Some(App::open(self.home.as_deref())?);
        }
        match self.app.as_mut() {
            Some(app) => Ok(app),
            None => Err(CliFailure::fail("the application did not open", 1)),
        }
    }

    /// `ws(override)`: the explicit workspace, else the global `--workspace`, else
    /// `WORKGRAPH_WORKSPACE`.
    #[must_use]
    pub fn workspace_ref(&self, explicit: Option<&str>) -> Option<String> {
        explicit
            .map(str::to_owned)
            .or_else(|| self.workspace.clone())
            .or_else(|| std::env::var("WORKGRAPH_WORKSPACE").ok())
            .filter(|text| !text.is_empty())
    }

    /// `configure_logging("INFO", app.paths.log_dir)`, for the long-running commands that log.
    pub fn configure_logging(&mut self) -> Result<(), CliFailure> {
        let log_dir = PathBuf::from(self.app()?.paths.log_dir().as_str());
        configure_logging(&LogSettings {
            level: Level::Info,
            log_dir: Some(log_dir),
            stderr: true,
        })?;
        Ok(())
    }

    /// Writes to the output stream.
    pub fn print(&mut self, text: &str) {
        write_to(&mut *self.stdout, text);
    }

    /// Writes to the error stream.
    pub fn eprint(&mut self, text: &str) {
        write_to(&mut *self.stderr, text);
    }

    /// `emit(value, human)`: the value as JSON with `--json` (or when there is no human
    /// renderer), otherwise whatever `human` renders (a `wg-render` result or plain text).
    pub fn emit(
        &mut self,
        value: &Value,
        human: Option<&dyn Fn(&Console) -> String>,
    ) -> Result<(), CliFailure> {
        let text = match human {
            Some(render) if !self.json => render(&self.console),
            _ => cli_json(value).map_err(|err| CliFailure::fail(err.to_string(), 1))?,
        };
        self.print(&text);
        Ok(())
    }
}
