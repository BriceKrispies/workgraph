//! wg-telemetry: Claude Code hook payload normalisation, secret redaction, dedupe keys, session correlation
//! and binding, subagent transcript parsing and the wall-time breakdown (`integrations/claude_code/*` and
//! `security/redaction.py` in the Python tree; GV-07).
//!
//! Everything here is a pure function of its inputs (plan 4.6, Core tier). The shell reads the transcript
//! and sidecar files, runs the database lookups that [`binding::BindingFacts`] describes, and clocks the
//! receipt time when a record has none.
//!
//! * [`hook`] is the payload normalisation, the dedupe key, the tool summary and the tool event shape.
//! * [`redact`] is secret redaction, bounded truncation and the shape-only summary.
//! * [`binding`] is session correlation: which attempt, task or cwd binding a session belongs to.
//! * [`transcript`] parses a subagent transcript and its tool calls.
//! * [`timing`] classifies tool calls and measures where a run's wall time went.
//! * [`json`] is the Python text of JSON values (`json.dumps(sort_keys=True)`, `str`, `repr`).

pub mod binding;
pub mod hook;
pub mod json;
pub mod redact;
pub mod timing;
pub mod transcript;
mod util;

pub use binding::{BindingFacts, BindingSource, Resolved, resolve_binding};
pub use hook::{CaptureConfig, HookFields, Normalised, PayloadError, dedupe_key, normalise};
pub use redact::{redact, redact_object, redact_text, summarize};
pub use timing::{breakdown, classify};
pub use transcript::{ToolCall, TranscriptStats, parse_transcript, split_lines, tool_calls};
