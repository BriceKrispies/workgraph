//! Experiment protocol validation (`core/policies/experiments.py`): every problem with a
//! protocol, so one attempt reports them all, and the arms a valid protocol names.
//!
//! A protocol pins everything a trial depends on before the first trial runs: the model per arm
//! (by full id), the brief, the size metric, the band range, the staircase and the budget. Bare
//! model aliases are rejected because the same alias has resolved to different models over time.

use serde_json::{Map, Value as Json, json};
use wg_model::py_repr_str;
use wg_types::{ErrorCode, WgError};

/// Aliases resolve to whatever model is current (`MODEL_ALIASES`).
const MODEL_ALIASES: [&str; 5] = ["haiku", "sonnet", "opus", "fable", "inherit"];
/// `REQUIRED_KEYS`.
const REQUIRED_KEYS: [&str; 6] = [
    "arms",
    "brief_template",
    "primary_size",
    "bands",
    "staircase",
    "budget",
];
/// `OPTIONAL_KEYS`.
const OPTIONAL_KEYS: [&str; 10] = [
    "refine",
    "repeat",
    "control",
    "gates",
    "seed",
    "oracle_template",
    "env",
    "pool",
    "setup",
    "clone",
];
/// `CLONE_MODES`, in the order the message lists them.
const CLONE_MODES: [&str; 2] = ["shared", "isolated"];
/// `STAIRCASE_KEYS`.
const STAIRCASE_KEYS: [&str; 5] = ["per_visit", "up", "down", "max_reversals", "max_trials"];

/// `_is_int`: a JSON integer (a Python `int`, never a `bool` or a float).
fn is_int(value: &Json) -> bool {
    matches!(value, Json::Number(n) if n.is_i64() || n.is_u64())
}

/// `_at_least_one`.
fn at_least_one(value: Option<&Json>) -> bool {
    match value {
        Some(Json::Number(n)) => {
            n.as_i64().is_some_and(|i| i >= 1) || n.as_u64().is_some_and(|u| u >= 1)
        }
        _ => false,
    }
}

/// An integer as a wide integer, for comparisons that must not overflow.
fn wide(value: &Json) -> Option<i128> {
    match value {
        Json::Number(n) => n
            .as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from)),
        _ => None,
    }
}

/// `str.isdigit` for one character: the ASCII digits and the common non-ASCII digit forms.
fn py_isdigit(c: char) -> bool {
    c.is_ascii_digit()
        || matches!(
            c,
            '\u{b2}'
                | '\u{b3}'
                | '\u{b9}'
                | '\u{660}'..='\u{669}'
                | '\u{6f0}'..='\u{6f9}'
                | '\u{966}'..='\u{96f}'
                | '\u{2070}'
                | '\u{2074}'..='\u{2079}'
                | '\u{2080}'..='\u{2089}'
                | '\u{ff10}'..='\u{ff19}'
        )
}

/// `model_problem`: why a model id is not acceptable for a trial, or `None` for a pinned full id.
#[must_use]
pub fn model_problem(model: &Json) -> Option<String> {
    let Some(text) = model.as_str().filter(|text| !text.trim().is_empty()) else {
        return Some("is not a non-empty string".to_owned());
    };
    if MODEL_ALIASES.contains(&text.trim().to_lowercase().as_str()) {
        return Some(format!(
            "is the alias {}; pin a full model id such as 'claude-haiku-5-5'",
            py_repr_str(text)
        ));
    }
    // A pinned id names a version; a family name such as "claude-haiku" is still an alias.
    if !text.chars().any(py_isdigit) {
        return Some(
            "is not a pinned model id (expected a full id such as 'claude-haiku-5-5')".to_owned(),
        );
    }
    None
}

/// `_positive_ints`: the problems of a section whose listed keys are integers of at least one.
fn positive_ints(section: &Json, name: &str, keys: &[&str]) -> Vec<String> {
    let Json::Object(map) = section else {
        return vec![format!("{name} must be an object")];
    };
    let bad = keys
        .iter()
        .filter(|key| !at_least_one(map.get(**key)))
        .map(|key| format!("{name}.{key} must be an integer >= 1"));
    let mut unknown: Vec<&str> = map
        .keys()
        .map(String::as_str)
        .filter(|key| !keys.contains(key))
        .collect();
    unknown.sort_unstable();
    let extra =
        (!unknown.is_empty()).then(|| format!("{name} has unknown keys: {}", unknown.join(", ")));
    bad.chain(extra).collect()
}

/// The problems of the `arms` entry.
fn arm_problems(arms: &Json) -> Vec<String> {
    let Some(list) = arms.as_array().filter(|list| !list.is_empty()) else {
        return vec!["arms must be a non-empty list of {name, model} objects".to_owned()];
    };
    let (problems, _) = list.iter().enumerate().fold(
        (Vec::<String>::new(), Vec::<&str>::new()),
        |(problems, names), (index, arm)| {
            let label = format!("arms[{index}]");
            let named = arm.as_object().filter(|map| {
                map.len() == 2 && map.contains_key("name") && map.contains_key("model")
            });
            let Some(map) = named else {
                let problems = problems
                    .into_iter()
                    .chain([format!(
                        "{label} must be an object with exactly name and model"
                    )])
                    .collect();
                return (problems, names);
            };
            let (name_problem, names) = match map.get("name").and_then(Json::as_str) {
                Some(name) if !name.trim().is_empty() => {
                    if names.contains(&name) {
                        (
                            Some(format!(
                                "{label}.name {} is used by another arm",
                                py_repr_str(name)
                            )),
                            names,
                        )
                    } else {
                        (None, names.into_iter().chain([name]).collect())
                    }
                }
                _ => (
                    Some(format!("{label}.name must be a non-empty string")),
                    names,
                ),
            };
            let model_issue = map
                .get("model")
                .and_then(model_problem)
                .map(|problem| format!("{label}.model {problem}"));
            (
                problems
                    .into_iter()
                    .chain(name_problem)
                    .chain(model_issue)
                    .collect(),
                names,
            )
        },
    );
    problems
}

/// The problems of the `bands` entry.
fn band_problems(bands: &Json) -> Vec<String> {
    let exact = bands
        .as_object()
        .filter(|map| map.len() == 2 && map.contains_key("min") && map.contains_key("max"));
    let Some(map) = exact else {
        return vec!["bands must be an object with exactly min and max".to_owned()];
    };
    match (map.get("min").and_then(wide), map.get("max").and_then(wide)) {
        (Some(low), Some(high)) if low > high => {
            vec!["bands.min must not exceed bands.max".to_owned()]
        }
        (Some(_), Some(_)) => Vec::new(),
        _ => vec!["bands.min and bands.max must be integers".to_owned()],
    }
}

/// A non-empty list of non-empty strings.
fn string_list(value: &Json) -> bool {
    value.as_array().is_some_and(|parts| {
        !parts.is_empty()
            && parts
                .iter()
                .all(|part| part.as_str().is_some_and(|text| !text.is_empty()))
    })
}

/// A list of non-empty command lists (`gates`, `setup`).
fn command_lists(value: &Json) -> bool {
    value
        .as_array()
        .is_some_and(|commands| commands.iter().all(string_list))
}

/// `protocol_problems`: every problem with a protocol, in the order Python reports them.
#[must_use]
pub fn protocol_problems(protocol: &Json) -> Vec<String> {
    let Json::Object(map) = protocol else {
        return vec!["protocol must be a JSON object".to_owned()];
    };
    let mut missing: Vec<&str> = REQUIRED_KEYS
        .iter()
        .copied()
        .filter(|key| !map.contains_key(*key))
        .collect();
    missing.sort_unstable();
    let mut unknown: Vec<&str> = map
        .keys()
        .map(String::as_str)
        .filter(|key| !REQUIRED_KEYS.contains(key) && !OPTIONAL_KEYS.contains(key))
        .collect();
    unknown.sort_unstable();
    let key_problems = [
        (!missing.is_empty()).then(|| format!("missing keys: {}", missing.join(", "))),
        (!unknown.is_empty()).then(|| format!("unknown keys: {}", unknown.join(", "))),
    ]
    .into_iter()
    .flatten();

    let arms = map.get("arms").map(arm_problems).unwrap_or_default();
    let texts = ["brief_template", "primary_size"]
        .into_iter()
        .filter(|key| {
            map.get(*key)
                .is_some_and(|value| value.as_str().is_none_or(|text| text.trim().is_empty()))
        })
        .map(|key| format!("{key} must be a non-empty string"));
    let bands = map.get("bands").map(band_problems).unwrap_or_default();
    let staircase = map
        .get("staircase")
        .map(|section| positive_ints(section, "staircase", &STAIRCASE_KEYS))
        .unwrap_or_default();
    let budget = map
        .get("budget")
        .map(|section| positive_ints(section, "budget", &["trials"]))
        .unwrap_or_default();
    let objects = ["refine", "repeat", "control"]
        .into_iter()
        .filter(|key| map.get(*key).is_some_and(|value| !value.is_object()))
        .map(|key| format!("{key} must be an object"));
    let control_model = map
        .get("control")
        .and_then(Json::as_object)
        .and_then(|control| control.get("model"))
        .and_then(model_problem)
        .map(|problem| format!("control.model {problem}"));
    let gates = map
        .get("gates")
        .filter(|value| !command_lists(value))
        .map(|_| "gates must be a list of non-empty command lists of strings".to_owned());
    let setup = map
        .get("setup")
        .filter(|value| !command_lists(value))
        .map(|_| "setup must be a list of non-empty command lists of strings".to_owned());
    let template = map
        .get("oracle_template")
        .filter(|value| !string_list(value))
        .map(|_| "oracle_template must be a non-empty list of strings".to_owned());
    let env = map
        .get("env")
        .filter(|value| {
            !value
                .as_object()
                .is_some_and(|env| env.values().all(Json::is_string))
        })
        .map(|_| "env must be an object of string values".to_owned());
    let clone = map
        .get("clone")
        .filter(|value| {
            !value
                .as_str()
                .is_some_and(|mode| CLONE_MODES.contains(&mode))
        })
        .map(|_| format!("clone must be one of: {}", CLONE_MODES.join(", ")));
    let pool = map
        .get("pool")
        .filter(|value| !value.is_object())
        .map(|_| "pool must be an object".to_owned());
    let seed = map
        .get("seed")
        .filter(|value| !is_int(value))
        .map(|_| "seed must be an integer".to_owned());

    key_problems
        .chain(arms)
        .chain(texts)
        .chain(bands)
        .chain(staircase)
        .chain(budget)
        .chain(objects)
        .chain(control_model)
        .chain(gates)
        .chain(setup)
        .chain(template)
        .chain(env)
        .chain(clone)
        .chain(pool)
        .chain(seed)
        .collect()
}

/// `validate_protocol`: the protocol as an object when valid; otherwise a `validation_failed`
/// error naming every problem.
///
/// # Errors
///
/// Returns the `validation_failed` error with the problems as its `problems` detail.
pub fn validate_protocol(protocol: &Json) -> Result<Map<String, Json>, WgError> {
    let problems = protocol_problems(protocol);
    match (problems.is_empty(), protocol) {
        (true, Json::Object(map)) => Ok(map.clone()),
        _ => Err(WgError::new(
            ErrorCode::ValidationFailed,
            format!("invalid experiment protocol: {}", problems.join("; ")),
        )
        .with_detail("problems", json!(problems))),
    }
}

/// `ExperimentService._arms`: the `(name, model)` of each arm of a valid protocol, in order.
#[must_use]
pub fn arms(protocol: &Map<String, Json>) -> Vec<(String, String)> {
    protocol
        .get("arms")
        .and_then(Json::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|arm| {
                    Some((
                        arm.get("name")?.as_str()?.to_owned(),
                        arm.get("model")?.as_str()?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}
