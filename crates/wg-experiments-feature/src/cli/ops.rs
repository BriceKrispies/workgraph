//! `workgraph experiment create, freeze, close, next, report, fit, predict` (IF-19): the handlers
//! of the experiment lifecycle, the planner and the predictor. Each reads its options with
//! [`crate::spec::ops_args`], calls one service entry point and prints the result with
//! [`crate::spec::ops_view`].

use clap::ArgMatches;
use wg_iface::cli::CliFailure;
use wg_iface::cli::args::one;
use wg_runtime::{CliState, human_context};

use crate::service;
use crate::spec::predict::predict_request;
use crate::spec::{ops_args, ops_view};

/// Reads a file for a `@path` argument.
fn read_file(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|err| format!("cannot read {path}: {err}"))
}

/// Runs the command at `path` when it is one of these; `None` for any other command.
pub(super) fn run(
    state: &mut CliState,
    path: &[String],
    m: &ArgMatches,
    ws: Option<&str>,
) -> Option<Result<(), CliFailure>> {
    let reference = one(m, "experiment").unwrap_or_default();
    match path.get(1).map(String::as_str)? {
        "create" => Some(create(state, m, ws)),
        "freeze" => Some(lifecycle(state, ws, &reference, Step::Freeze)),
        "close" => Some(lifecycle(state, ws, &reference, Step::Close)),
        "next" => Some(next(state, ws, &reference)),
        "report" => Some(report(state, ws, &reference)),
        "fit" => Some(fit(state, m, ws)),
        "predict" => Some(predict(state, m, ws)),
        _ => None,
    }
}

/// The lifecycle steps that take an experiment reference.
enum Step {
    Freeze,
    Close,
}

fn create(state: &mut CliState, m: &ArgMatches, ws: Option<&str>) -> Result<(), CliFailure> {
    let args = ops_args::create_args(m, &read_file)?;
    let context = human_context(None)?;
    let out = service::experiment_create(
        state.app()?,
        &context,
        ws,
        &args.slug,
        &args.title,
        args.question.as_deref(),
        &args.protocol,
    )?;
    state.emit(&out, Some(&|c| ops_view::create_view(c, &out)))
}

fn lifecycle(
    state: &mut CliState,
    ws: Option<&str>,
    reference: &str,
    step: Step,
) -> Result<(), CliFailure> {
    let context = human_context(None)?;
    match step {
        Step::Freeze => {
            let out = service::experiment_freeze(state.app()?, &context, ws, reference)?;
            state.emit(&out, Some(&|c| ops_view::freeze_view(c, &out)))
        }
        Step::Close => {
            let out = service::experiment_close(state.app()?, &context, ws, reference)?;
            state.emit(&out, Some(&|c| ops_view::close_view(c, &out)))
        }
    }
}

fn next(state: &mut CliState, ws: Option<&str>, reference: &str) -> Result<(), CliFailure> {
    let context = human_context(None)?;
    let out = service::experiment_next(state.app()?, &context, ws, reference)?;
    state.emit(&out, Some(&|c| ops_view::next_view(c, &out)))
}

fn report(state: &mut CliState, ws: Option<&str>, reference: &str) -> Result<(), CliFailure> {
    let context = human_context(None)?;
    let out = service::experiment_report(state.app()?, &context, ws, reference)?;
    state.emit(&out, Some(&|c| ops_view::report_view(c, &out)))
}

fn fit(state: &mut CliState, m: &ArgMatches, ws: Option<&str>) -> Result<(), CliFailure> {
    let (model, settings) = ops_args::fit_args(m)?;
    let context = human_context(None)?;
    let out = service::experiment_fit(state.app()?, &context, ws, &model, &settings)?;
    state.emit(&out, Some(&|c| ops_view::fit_view(c, &out)))
}

fn predict(state: &mut CliState, m: &ArgMatches, ws: Option<&str>) -> Result<(), CliFailure> {
    let (model, options, fit_id) = ops_args::predict_args(m)?;
    let context = human_context(None)?;
    state.app()?;
    let request = predict_request(&options)?;
    let out = service::experiment_predict(
        state.app()?,
        &context,
        ws,
        &model,
        &request,
        fit_id.as_deref(),
    )?;
    state.emit(&out, Some(&|c| ops_view::prediction_view(c, &out)))
}
