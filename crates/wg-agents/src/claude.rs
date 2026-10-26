//! Running the Claude Code provider (P02).

use wg_agentspec::{
    AgentError, AgentEvent, AgentRunRequest, AgentRunResult, ClaudeCodeProvider, StreamState,
    availability_detail, build_argv, finish_claude, timeout_ms, unavailable_message,
};
use wg_proc::{Cancel, CaptureLimits, ProcessSpec, base_environment, run_capture};

use crate::AgentRunner;
use crate::stream::{resolve, run_streamed};

/// The milliseconds the `--version` probe may take.
const PROBE_TIMEOUT_MS: u64 = 20_000;

impl AgentRunner for ClaudeCodeProvider {
    /// Whether the executable resolves and answers `--version`; the detail is the version line or
    /// the reason.
    fn check_available(&self) -> (bool, String) {
        let exe = match resolve(&self.config.executable) {
            Ok(path) => path.display().to_string(),
            Err(error) => {
                return (
                    false,
                    format!(
                        "{} (install Claude Code or set agents.claude_code.executable)",
                        unavailable_message(&error)
                    ),
                );
            }
        };
        let mut spec = ProcessSpec::new([exe.clone(), "--version".to_owned()]);
        spec.timeout_ms = Some(PROBE_TIMEOUT_MS);
        match run_capture(&spec, CaptureLimits::default(), None) {
            Err(error) => (false, format!("{exe} --version failed: {error}")),
            Ok(out) if out.timed_out => (
                false,
                format!("{exe} --version failed: timed out after 20 seconds"),
            ),
            Ok(out) => availability_detail(
                out.exit_code,
                &String::from_utf8_lossy(&out.stdout),
                &String::from_utf8_lossy(&out.stderr),
            ),
        }
    }

    fn run<E>(
        &self,
        request: &AgentRunRequest,
        on_event: E,
        cancel: Option<Cancel<'_>>,
    ) -> Result<AgentRunResult, AgentError>
    where
        E: FnMut(AgentEvent) + Send + 'static,
    {
        let (ok, detail) = self.check_available();
        if !ok {
            return Err(AgentError::Unavailable(format!(
                "Claude Code is unavailable: {detail}"
            )));
        }
        let session_id = request
            .resume_session_id
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut spec = ProcessSpec::new(build_argv(&self.config, request, &session_id));
        spec.cwd = Some(request.workdir.clone());
        // Claude Code needs the user's own environment (credentials, config dirs).
        spec.env = Some(base_environment(request.env.clone(), true));
        spec.timeout_ms = Some(timeout_ms(
            request.timeout_seconds,
            self.config.timeout_seconds,
        ));
        spec.stdin_data = Some(request.prompt.clone().into_bytes());
        let (process, state) = run_streamed(
            &spec,
            request.log_path.clone(),
            StreamState::new(&session_id),
            StreamState::handle_line,
            on_event,
            cancel,
        )?;
        Ok(finish_claude(&state, &process))
    }
}
