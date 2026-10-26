//! wg-agents: Claude Code adapter and command agent adapter.
//!
//! The shell half of `workgraph/integrations/claude_code/adapter.py` and
//! `workgraph/integrations/agents/command.py`. The providers' data, the decisions (argument
//! vector, line interpretation, result shaping) and the error type live in the pure
//! `wg-agentspec`; this crate resolves the executable, writes the protocol files and runs the
//! real process through `wg-proc`. Import [`AgentRunner`] to call `check_available` and `run`.

mod claude;
mod command;
mod stream;

use wg_proc::Cancel;

pub use wg_agentspec::{
    AgentCapabilities, AgentError, AgentEvent, AgentRunRequest, AgentRunResult, ClaudeCodeProvider,
    ClaudeConfig, CommandAgentProvider, CommandConfig, EventKind, RunStatus,
};

/// What a provider does with a process: availability and one attempt.
pub trait AgentRunner {
    /// Whether the agent can be used; the detail is a version, a path or the reason.
    fn check_available(&self) -> (bool, String);

    /// Run one attempt, streaming events to `on_event`. `cancel` is polled and, once it returns
    /// `true`, the child's tree is terminated.
    ///
    /// # Errors
    /// [`AgentError::Unavailable`] when the agent is not usable, [`AgentError::Io`] when a
    /// protocol file cannot be written, [`AgentError::Proc`] when the process cannot be started.
    /// A run that fails, times out or is cancelled is a normal [`AgentRunResult`].
    fn run<E>(
        &self,
        request: &AgentRunRequest,
        on_event: E,
        cancel: Option<Cancel<'_>>,
    ) -> Result<AgentRunResult, AgentError>
    where
        E: FnMut(AgentEvent) + Send + 'static;
}
