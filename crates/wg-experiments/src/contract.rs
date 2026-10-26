//! Leaf contract check: port of `src/workgraph/experiments/contract.py` (CORE-26, GV-17).
//!
//! The decision runs in a fixed order. A task that needs a human decision, has no checkable
//! oracle, or is a report or register item is `not_delegable`. A task whose `metadata.contract` is
//! missing or malformed is `incomplete`. Otherwise the predictor scores the declared size: `leaf`
//! when predicted success meets the target, `split` when it does not. The predicted success is
//! always an inference from the named fit.
//!
//! The shell reads the task and runs the predictor (a database read and a fit); this module holds
//! the field checks and the decision.

use serde_json::Value;

/// Tags that make a task not delegable (`NOT_DELEGABLE_TAGS`).
pub const NOT_DELEGABLE_TAGS: [&str; 5] = [
    "needs-decision",
    "kind:report",
    "kind:register-row",
    "kind:split-status",
    "kind:manifest",
];

/// What the leaf check decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The task can go out as a leaf.
    Leaf,
    /// The predicted success is below the target: split the task.
    Split,
    /// The task cannot be delegated.
    NotDelegable,
    /// The contract is missing or malformed, or there is no predictor fit.
    Incomplete,
}

impl Verdict {
    /// The wire name of the verdict.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Leaf => "leaf",
            Self::Split => "split",
            Self::NotDelegable => "not_delegable",
            Self::Incomplete => "incomplete",
        }
    }
}

/// The predictor's answer for one task.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Prediction {
    /// Predicted probability of success, in `[0, 1]`.
    pub probability: f64,
    /// Whether the fit has seen the task's repository.
    pub repository_known: bool,
}

/// Whether a fit exists for the requested model.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fit {
    /// No predictor fit exists for the model (the Python `NotFoundError`).
    NoFit,
    /// The fit predicted for the task.
    Predicted(Prediction),
}

/// The fields missing or malformed, and the reasons the task cannot go out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problems {
    /// Contract fields absent or malformed, named as the Python source names them.
    pub missing: Vec<String>,
    /// Reasons the task is not delegable.
    pub not_delegable: Vec<String>,
}

/// The result of the leaf check (`ContractVerdict`).
#[derive(Clone, Debug, PartialEq)]
pub struct ContractVerdict {
    /// The decision.
    pub verdict: Verdict,
    /// Human-readable reasons, one or more.
    pub reasons: Vec<String>,
    /// Missing contract fields, when the verdict is `incomplete` on the contract.
    pub missing: Vec<String>,
    /// The prediction, when one was made.
    pub prediction: Option<Prediction>,
    /// The target the prediction was compared with.
    pub target: f64,
    /// The predictor model name.
    pub model: String,
}

/// Checks a task's tags, acceptance criteria and metadata for the contract fields.
///
/// `metadata` is the task's metadata object; the contract is read from its `contract` key, and a
/// missing or non-object contract counts as empty.
#[must_use]
pub fn contract_problems(tags: &[String], criteria: &[String], metadata: &Value) -> Problems {
    let contract = metadata.get("contract").filter(|value| value.is_object());
    let field = |key: &str| contract.and_then(|c| c.get(key));
    let predicted = field("predicted").filter(|value| value.is_object());
    let predicted_field = |key: &str| predicted.and_then(|p| p.get(key));

    let tag_reasons = tags
        .iter()
        .filter(|tag| NOT_DELEGABLE_TAGS.contains(&tag.as_str()))
        .map(|tag| format!("tag {tag} makes the task not delegable"));
    let oracle_missing = !has_oracle(field("oracle")) && criteria.is_empty();
    let not_delegable: Vec<String> = tag_reasons
        .chain(oracle_missing.then(|| {
            "no checkable oracle: the task has no contract oracle and no acceptance criteria"
                .to_owned()
        }))
        .collect();

    let checks = field("checks").and_then(Value::as_array);
    let checks_valid = checks.is_some_and(|items| {
        items.len() == criteria.len()
            && items
                .iter()
                .all(|item| item.as_str().is_some_and(|text| !text.trim().is_empty()))
    });
    let got = checks.map_or_else(|| "none".to_owned(), |items| items.len().to_string());
    let packages_valid = predicted_field("packages")
        .and_then(Value::as_array)
        .is_some_and(|items| items.iter().all(Value::is_string));

    let missing: Vec<String> = [
        (!non_empty_strings(field("files"))).then(|| "contract.files".to_owned()),
        (!argv_lists(field("oracle"), false)).then(|| "contract.oracle".to_owned()),
        (!argv_lists(field("gates"), true)).then(|| "contract.gates".to_owned()),
        (!non_empty_strings(field("out_of_scope"))).then(|| "contract.out_of_scope".to_owned()),
        (!checks_valid).then(|| {
            format!(
                "contract.checks ({} acceptance criteria, {got} checks, one non-empty each)",
                criteria.len()
            )
        }),
        (!is_positive(predicted_field("src_lines")))
            .then(|| "contract.predicted.src_lines".to_owned()),
        (!is_positive(predicted_field("src_files")))
            .then(|| "contract.predicted.src_files".to_owned()),
        (!packages_valid).then(|| "contract.predicted.packages".to_owned()),
    ]
    .into_iter()
    .flatten()
    .collect();

    Problems {
        missing,
        not_delegable,
    }
}

fn has_oracle(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
}

fn is_positive(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_f64)
        .is_some_and(|number| number > 0.0)
}

/// A non-empty list of non-blank strings.
fn non_empty_strings(value: Option<&Value>) -> bool {
    value.and_then(Value::as_array).is_some_and(|items| {
        !items.is_empty()
            && items
                .iter()
                .all(|item| item.as_str().is_some_and(|text| !text.trim().is_empty()))
    })
}

/// A list of argv lists, each non-empty and made of non-empty strings. An empty outer list is
/// accepted only when `allow_empty`.
fn argv_lists(value: Option<&Value>, allow_empty: bool) -> bool {
    value.and_then(Value::as_array).is_some_and(|argvs| {
        (allow_empty || !argvs.is_empty())
            && argvs.iter().all(|argv| {
                argv.as_array().is_some_and(|parts| {
                    !parts.is_empty()
                        && parts
                            .iter()
                            .all(|part| part.as_str().is_some_and(|text| !text.is_empty()))
                })
            })
    })
}

/// Python's `float` text for the out-of-range target in the error message.
fn py_float(number: f64) -> String {
    if number.is_nan() {
        "nan".to_owned()
    } else if number.is_infinite() {
        (if number > 0.0 { "inf" } else { "-inf" }).to_owned()
    } else if number.fract() == 0.0 && number.abs() < 1e16 {
        format!("{number:.1}")
    } else {
        format!("{number}")
    }
}

/// The leaf decision, given the contract problems and a lazily computed predictor fit.
///
/// The fit is consulted only when the contract is complete, as the Python source does.
///
/// # Errors
///
/// Returns the Python `ValidationFailed` message when `target` is outside `[0, 1]`.
pub fn decide(
    problems: Problems,
    fit: impl FnOnce() -> Fit,
    model: &str,
    target: f64,
) -> Result<ContractVerdict, String> {
    if !(0.0..=1.0).contains(&target) {
        return Err(format!(
            "target must be between 0 and 1, got {}",
            py_float(target)
        ));
    }
    let verdict = |verdict, reasons: Vec<String>, missing, prediction| ContractVerdict {
        verdict,
        reasons,
        missing,
        prediction,
        target,
        model: model.to_owned(),
    };
    if !problems.not_delegable.is_empty() {
        return Ok(verdict(
            Verdict::NotDelegable,
            problems.not_delegable,
            Vec::new(),
            None,
        ));
    }
    if !problems.missing.is_empty() {
        let reason =
            "contract is missing or malformed: fix the fields listed in missing".to_owned();
        return Ok(verdict(
            Verdict::Incomplete,
            vec![reason],
            problems.missing,
            None,
        ));
    }
    let prediction = match fit() {
        Fit::NoFit => {
            let reason = format!(
                "no predictor fit for {model}: run workgraph experiment fit --model {model}"
            );
            return Ok(verdict(Verdict::Incomplete, vec![reason], Vec::new(), None));
        }
        Fit::Predicted(prediction) => prediction,
    };
    if prediction.probability < target {
        let reason = format!(
            "predicted success {:.3} is below the target {target:.3}: split",
            prediction.probability
        );
        return Ok(verdict(
            Verdict::Split,
            vec![reason],
            Vec::new(),
            Some(prediction),
        ));
    }
    let meets = format!(
        "predicted success {:.3} meets the target {target:.3}",
        prediction.probability
    );
    let reasons = if prediction.repository_known {
        vec![meets]
    } else {
        vec![
            meets,
            "repository not seen by the fit: run a replay calibration first".to_owned(),
        ]
    };
    Ok(verdict(
        Verdict::Leaf,
        reasons,
        Vec::new(),
        Some(prediction),
    ))
}
