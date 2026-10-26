//! Running the command agent provider (P04, `workgraph.agent.v1`).

use std::fs;
use std::path::Path;

use wg_agentspec::util::py_dumps;
use wg_agentspec::{
    AgentError, AgentEvent, AgentRunRequest, AgentRunResult, CommandAgentProvider, CommandState,
    finish_command, placeholder_values, render_args, request_payload, timeout_ms,
    unavailable_message,
};
use wg_proc::{Cancel, ProcessSpec, base_environment};

use crate::AgentRunner;
use crate::stream::{resolve, run_streamed};

fn write_file(path: &Path, content: &str) -> Result<(), AgentError> {
    fs::write(path, content).map_err(|source| AgentError::Io {
        path: path.display().to_string(),
        source,
    })
}

impl AgentRunner for CommandAgentProvider {
    /// Whether the executable resolves; the detail is its path or the reason.
    fn check_available(&self) -> (bool, String) {
        match resolve(&self.config.executable) {
            Ok(path) => (true, path.display().to_string()),
            Err(error) => (false, unavailable_message(&error)),
        }
    }

    /// The prompt and the request are written beside the manifest (the attempt directory), never
    /// inside a repository.
    fn run<E>(
        &self,
        request: &AgentRunRequest,
        on_event: E,
        cancel: Option<Cancel<'_>>,
    ) -> Result<AgentRunResult, AgentError>
    where
        E: FnMut(AgentEvent) + Send + 'static,
    {
        let control_dir = request.manifest_path.parent().unwrap_or(Path::new("."));
        let prompt_file = control_dir.join("workgraph-prompt.md");
        write_file(&prompt_file, &request.prompt)?;
        let payload = request_payload(request);
        write_file(
            &control_dir.join("workgraph-request.json"),
            &py_dumps(&payload, Some(2)),
        )?;
        let values = placeholder_values(request, &prompt_file.display().to_string());
        let mut argv = vec![self.config.executable.clone()];
        argv.extend(render_args(&self.config.args, &values));
        let mut spec = ProcessSpec::new(argv);
        spec.cwd = Some(request.workdir.clone());
        let extra = self.config.env.iter().chain(request.env.iter()).cloned();
        spec.env = Some(base_environment(extra, false));
        spec.timeout_ms = Some(timeout_ms(
            request.timeout_seconds,
            self.config.timeout_seconds,
        ));
        spec.stdin_data = Some(py_dumps(&payload, None).into_bytes());
        let (process, state) = run_streamed(
            &spec,
            request.log_path.clone(),
            CommandState::default(),
            CommandState::handle_line,
            on_event,
            cancel,
        )?;
        Ok(finish_command(&state, &process, &self.config))
    }
}
