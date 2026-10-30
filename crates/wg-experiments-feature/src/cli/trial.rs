//! Shell: the `experiment trial` subgroup (IF-20): `add`, `show`, `classify`, `start` and `grade`. The clap tree
//! is the snapshot's own (`wg_iface::cli::tree`), built by the `experiment` group that owns it; that
//! group's run function hands every `experiment trial ...` invocation to [`run`].
//!
//! `start` and `grade` are the trial runner ([`crate::service::runner`]); their views live in
//! [`crate::spec::runner`]. The other views and the band parsing are pure and live in
//! [`crate::spec::trial`].

use clap::ArgMatches;
use wg_iface::cli::args::option_text;
use wg_iface::cli::{CliFailure, Usage, not_implemented_message};
use wg_runtime::{CliState, human_context};

use crate::service::{runner, trials};
use crate::spec::replay::float_text;
use crate::spec::runner::{grade_view, start_view};
use crate::spec::trial::{AddInput, band_text, trial_view};

/// The usage of `experiment trial grade`, as Click prints it in a usage error.
const GRADE: Usage = Usage {
    usage: "workgraph experiment trial grade [OPTIONS] TRIAL",
    command: "workgraph experiment trial grade",
};

/// Runs the matched leaf of `experiment trial ...`. `path` is the full path, so `path[2]` is the
/// verb.
pub fn run(state: &mut CliState, path: &[String], m: &ArgMatches) -> Result<(), CliFailure> {
    let workspace = state.workspace_ref(option_text(m, "--workspace").as_deref());
    let ws = workspace.as_deref();
    let context = human_context(None)?;
    match path.get(2).map(String::as_str) {
        Some("add") => {
            let experiment = option_text(m, "experiment").unwrap_or_default();
            let task = option_text(m, "task").unwrap_or_default();
            let arm = option_text(m, "--arm").unwrap_or_default();
            let phase = option_text(m, "--phase").unwrap_or_default();
            let band = band_text(option_text(m, "--band").as_deref())?;
            let input = AddInput {
                arm: &arm,
                phase: &phase,
                band,
            };
            let out = trials::trial_add(state.app()?, &context, ws, &experiment, &task, input)?;
            state.emit(&out, Some(&|c| trial_view(c, &out)))
        }
        Some("show") => {
            let trial = option_text(m, "trial").unwrap_or_default();
            let out = trials::trial_show(state.app()?, &context, ws, &trial)?;
            state.emit(&out, Some(&|c| trial_view(c, &out)))
        }
        Some("classify") => {
            let trial = option_text(m, "trial").unwrap_or_default();
            let cause = option_text(m, "--cause").unwrap_or_default();
            let source = option_text(m, "--source").unwrap_or_default();
            let notes = option_text(m, "--notes");
            let out = trials::trial_classify(
                state.app()?,
                &context,
                ws,
                &trial,
                &cause,
                &source,
                notes.as_deref(),
            )?;
            state.emit(&out, Some(&|c| trial_view(c, &out)))
        }
        Some("start") => {
            let trial = option_text(m, "trial").unwrap_or_default();
            let out = runner::trial_start_run(state.app()?, &context, ws, &trial)?;
            state.emit(&out, Some(&|c| start_view(c, &out)))
        }
        Some("grade") => {
            let trial = option_text(m, "trial").unwrap_or_default();
            let timeout = float_text(
                option_text(m, "--timeout").as_deref(),
                "--timeout",
                3600.0,
                GRADE,
            )?;
            let out = runner::trial_grade_run(state.app()?, &context, ws, &trial, timeout)?;
            state.emit(&out, Some(&|c| grade_view(c, &out)))
        }
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}
