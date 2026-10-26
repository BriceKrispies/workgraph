//! Shell: the `experiment trial` subgroup (IF-20): `add`, `show` and `classify`. The clap tree
//! is the snapshot's own (`wg_clispec::tree`), built by the `experiment` group that owns it; that
//! group's run function hands every `experiment trial ...` invocation to [`run`].
//!
//! `start` and `grade` (the trial runner) are not ported yet; they print the same
//! not-implemented message as any other unported command. The views and the band parsing are
//! pure and live in [`crate::spec::trial`].

use clap::ArgMatches;
use wg_clispec::args::option_text;
use wg_clispec::{CliFailure, not_implemented_message};
use wg_runtime::{CliState, human_context};

use crate::service::trials;
use crate::spec::trial::{AddInput, band_text, trial_view};

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
        _ => Err(CliFailure::fail(not_implemented_message(path), 1)),
    }
}
