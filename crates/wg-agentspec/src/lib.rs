//! wg-agentspec: the pure parts of the agent adapters.
//!
//! The Rust counterpart of the decision logic in `workgraph/integrations/agents/{base,command}.py`
//! and `workgraph/integrations/claude_code/adapter.py`:
//!
//! * the request, event, result and capability types ([`types`]);
//! * the Claude Code adapter (P02): argument vector, incremental `stream-json` interpretation and
//!   the final result ([`claude`]);
//! * the `workgraph.agent.v1` command adapter (P04): placeholder validation and substitution, the
//!   request payload, line interpretation and the final result ([`command`]);
//! * the description of an adapter entry, as the registry reports it ([`describe`]);
//! * small Python-compatible helpers ([`util`]).
//!
//! Nothing here touches a file, a process or the clock; `wg-agents` does the I/O.

pub mod claude;
pub mod command;
pub mod describe;
pub mod error;
pub mod types;
pub mod util;

pub use describe::{AdapterDescription, describe_adapter};

pub use claude::{
    ClaudeCodeProvider, ClaudeConfig, StreamState, availability_detail, build_argv,
    claude_capabilities, claude_config_snapshot, finish_claude, timeout_ms, unavailable_message,
};
pub use command::{
    CommandAgentProvider, CommandConfig, CommandState, PLACEHOLDERS, PROTOCOL, UnknownPlaceholder,
    command_capabilities, command_config_snapshot, finish_command, placeholder_values, render_args,
    request_payload, validate_template,
};
pub use error::AgentError;
pub use types::{
    AgentCapabilities, AgentEvent, AgentRunRequest, AgentRunResult, EventKind, RunStatus,
};
