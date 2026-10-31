//! `CLIState` of `cli/common.py`: the global options, the lazily opened application and the
//! output streams of one `workgraph` invocation. The front end supplies the console (it reads the
//! terminal); the state holds it for the commands that render.

use std::io::Write;
use std::path::PathBuf;

use wg_iface::cli::CliFailure;
use wg_json::{Value, cli_json};
use wg_render::Console;

use crate::{App, Feature, Level, LogSettings, configure_logging};

/// What one `workgraph` invocation knows: `--json`, `--home`, `--workspace`, the open
/// application and where output goes. A plain record: the front end builds it with a literal
/// (`app` starts as `None`, and [`CliState::app`] opens it on first use).
pub struct CliState {
    pub json: bool,
    pub home: Option<PathBuf>,
    pub workspace: Option<String>,
    pub app: Option<App>,
    pub stdout: Box<dyn Write>,
    pub stderr: Box<dyn Write>,
    /// The console for standard output.
    pub console: Console,
    /// The console for standard error.
    pub err_console: Console,
    /// The features of the binary, for the commands that run jobs in process (`demo seed`).
    pub features: &'static [&'static Feature],
}

impl CliState {
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

/// Writes `text` to `stream` and flushes. A closed pipe (`workgraph ... | head`) is not an error.
fn write_to(stream: &mut dyn Write, text: &str) {
    if text.is_empty() {
        return;
    }
    let _ = stream.write_all(text.as_bytes());
    let _ = stream.flush();
}
