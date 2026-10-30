//! The arguments of `experiment create, fit, predict` (`cli/commands/experiments.py`): the
//! readers of the matched options, which build the requests. Functions of the matches with no
//! I/O; the handlers in [`crate::cli`] run them.

use clap::ArgMatches;
use serde_json::Value as Json;
use wg_iface::cli::args::{ReadFile, number, one};
use wg_iface::cli::failure::python_number_text;
use wg_iface::cli::{CliFailure, Usage};

use super::predict::{FitSettings, PredictOptions};

/// Click's usage line of `experiment fit`.
const FIT: Usage = Usage {
    usage: "workgraph experiment fit [OPTIONS]",
    command: "workgraph experiment fit",
};

/// Click's usage line of `experiment predict`.
const PREDICT: Usage = Usage {
    usage: "workgraph experiment predict [OPTIONS]",
    command: "workgraph experiment predict",
};

/// The arguments of `experiment create`.
#[derive(Debug, Clone, PartialEq)]
pub struct CreateArgs {
    pub slug: String,
    pub title: String,
    pub question: Option<String>,
    pub protocol: Json,
}

/// `parse_json_arg(protocol, "protocol")`: JSON text, or `@file` holding it.
fn protocol_document(text: &str, read_file: ReadFile<'_>) -> Result<Json, CliFailure> {
    let source = match text.strip_prefix('@') {
        Some(path) => read_file(path).map_err(|why| CliFailure::fail(why, 1))?,
        None => text.to_owned(),
    };
    wg_validate::pyjson::parse_message(&source)
        .map_err(|why| CliFailure::fail(format!("protocol is not valid JSON: {why}"), 2))
}

/// The options of `experiment create`.
///
/// # Errors
///
/// Returns the failure of an unreadable file or of JSON that does not parse.
pub fn create_args(m: &ArgMatches, read_file: ReadFile<'_>) -> Result<CreateArgs, CliFailure> {
    let protocol = protocol_document(&one(m, "--protocol").unwrap_or_default(), read_file)?;
    Ok(CreateArgs {
        slug: one(m, "slug").unwrap_or_default(),
        title: one(m, "--title").unwrap_or_default(),
        question: one(m, "--question"),
        protocol,
    })
}

/// A `float` option, else `default`: Click's check reports a value Python's `float()` cannot
/// read as a usage error.
fn float_option(m: &ArgMatches, id: &str, at: Usage) -> Result<Option<f64>, CliFailure> {
    one(m, id)
        .map(|text| {
            python_number_text(&text)
                .and_then(|digits| digits.parse::<f64>().ok())
                .ok_or_else(|| at.invalid_value(id, &text, "float"))
        })
        .transpose()
}

/// The model and settings of `experiment fit`.
///
/// # Errors
///
/// Returns the usage failure of an option that is not a number.
pub fn fit_args(m: &ArgMatches) -> Result<(String, FitSettings), CliFailure> {
    let repo_penalty = float_option(m, "--repo-penalty", FIT)?.unwrap_or(1.0);
    let feature_penalty = float_option(m, "--feature-penalty", FIT)?.unwrap_or(0.01);
    let samples = number(m, "--samples", 200, FIT)?;
    let seed = number(m, "--seed", 0, FIT)?;
    Ok((
        one(m, "--model").unwrap_or_default(),
        FitSettings {
            repo_penalty,
            feature_penalty,
            samples,
            seed: i128::from(seed),
        },
    ))
}

/// The model, the options and the fit id of `experiment predict`.
///
/// # Errors
///
/// Returns the usage failure of an option that is not a number.
pub fn predict_args(
    m: &ArgMatches,
) -> Result<(String, PredictOptions, Option<String>), CliFailure> {
    let lines = float_option(m, "--lines", PREDICT)?;
    let files = float_option(m, "--files", PREDICT)?;
    Ok((
        one(m, "--model").unwrap_or_default(),
        PredictOptions {
            task: one(m, "--task"),
            repo: one(m, "--repo"),
            lines,
            files,
            packages: one(m, "--packages"),
        },
        one(m, "--fit"),
    ))
}
