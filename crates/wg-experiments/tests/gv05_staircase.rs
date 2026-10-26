//! GV-05: `wg_experiments::staircase` reproduces `golden/expstats/staircase.json` exactly: the
//! protocol decoding and config validation (`config_cases`), the hand-written cases (`cases`), and
//! the simulated runs with the expected step after every prefix (`simulated`).

use std::error::Error;
use std::path::PathBuf;

use serde_json::Value;
use wg_experiments::staircase::{
    Lookup, Phase, ProtocolFields, Raw, StaircaseConfig, Step, next_step,
};

type TestResult = Result<(), Box<dyn Error>>;

fn load() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/expstats")
        .join("staircase.json");
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn cases<'a>(doc: &'a Value, key: &str) -> Result<&'a Vec<Value>, Box<dyn Error>> {
    let cases = doc[key]
        .as_array()
        .ok_or_else(|| format!("{key} is not an array"))?;
    assert!(!cases.is_empty(), "{key} is empty");
    Ok(cases)
}

fn int(value: &Value) -> Result<i64, Box<dyn Error>> {
    value
        .as_i64()
        .ok_or_else(|| format!("not an int: {value}").into())
}

fn history(value: &Value) -> Result<Vec<(i64, bool)>, Box<dyn Error>> {
    value
        .as_array()
        .ok_or("history is not an array")?
        .iter()
        .map(|pair| {
            Ok((
                int(&pair[0])?,
                pair[1].as_bool().ok_or("outcome is not a bool")?,
            ))
        })
        .collect()
}

/// A JSON value as a decoded protocol value: bool, int, or its repr.
fn raw(value: &Value) -> Raw {
    match value {
        Value::Bool(flag) => Raw::Bool(*flag),
        Value::Number(n) => n
            .as_i64()
            .map_or_else(|| Raw::Other(n.to_string()), Raw::Int),
        other => Raw::Other(other.to_string()),
    }
}

/// `protocol[section][key]` as a lookup; `Absent` when either level is missing.
fn lookup(section: Option<&Value>, key: &str) -> Lookup {
    section
        .and_then(|s| s.get(key))
        .map_or(Lookup::Absent, |v| Lookup::Found(raw(v)))
}

fn protocol_fields(protocol: &Value) -> ProtocolFields {
    let bands = protocol.get("bands");
    let staircase = protocol.get("staircase");
    let refine = protocol.get("refine");
    ProtocolFields {
        has_bands: bands.is_some(),
        has_staircase: staircase.is_some(),
        min: lookup(bands, "min"),
        max: lookup(bands, "max"),
        per_visit: lookup(staircase, "per_visit"),
        up: lookup(staircase, "up"),
        down: lookup(staircase, "down"),
        max_reversals: lookup(staircase, "max_reversals"),
        max_trials: lookup(staircase, "max_trials"),
        refine_trials: refine.and_then(|r| r.get("trials")).map(raw),
        refine_width: refine.and_then(|r| r.get("width")).map(raw),
    }
}

/// A `StaircaseConfig` literal from the JSON `config` object (no validation).
fn config_literal(config: &Value) -> Result<StaircaseConfig, Box<dyn Error>> {
    Ok(StaircaseConfig {
        min_band: int(&config["min_band"])?,
        max_band: int(&config["max_band"])?,
        per_visit: int(&config["per_visit"])?,
        up: int(&config["up"])?,
        down: int(&config["down"])?,
        max_reversals: int(&config["max_reversals"])?,
        max_trials: int(&config["max_trials"])?,
        refine_trials: int(&config["refine_trials"])?,
        refine_width: int(&config["refine_width"])?,
    })
}

/// The expected `StaircaseConfig` fields of an `ok` result.
fn assert_config(actual: &StaircaseConfig, expected: &Value, name: &str) -> TestResult {
    assert_eq!(
        actual.min_band,
        int(&expected["min_band"])?,
        "{name} min_band"
    );
    assert_eq!(
        actual.max_band,
        int(&expected["max_band"])?,
        "{name} max_band"
    );
    assert_eq!(
        actual.per_visit,
        int(&expected["per_visit"])?,
        "{name} per_visit"
    );
    assert_eq!(actual.up, int(&expected["up"])?, "{name} up");
    assert_eq!(actual.down, int(&expected["down"])?, "{name} down");
    assert_eq!(
        actual.max_reversals,
        int(&expected["max_reversals"])?,
        "{name} max_reversals"
    );
    assert_eq!(
        actual.max_trials,
        int(&expected["max_trials"])?,
        "{name} max_trials"
    );
    assert_eq!(
        actual.refine_trials,
        int(&expected["refine_trials"])?,
        "{name} refine_trials"
    );
    assert_eq!(
        actual.refine_width,
        int(&expected["refine_width"])?,
        "{name} refine_width"
    );
    Ok(())
}

/// The expected `Step` fields of an `ok` result, compared with exact floats.
fn assert_step(actual: &Step, expected: &Value, name: &str) -> TestResult {
    let phase = expected["phase"].as_str().ok_or("phase")?;
    assert_eq!(actual.phase.as_str(), phase, "{name} phase");
    let band = match &expected["band"] {
        Value::Null => None,
        value => Some(int(value)?),
    };
    assert_eq!(actual.band, band, "{name} band");
    assert_eq!(
        actual.reversals as i64,
        int(&expected["reversals"])?,
        "{name} reversals"
    );
    let threshold = match &expected["threshold"] {
        Value::Null => None,
        value => Some(value.as_f64().ok_or("threshold")?),
    };
    assert_eq!(actual.threshold, threshold, "{name} threshold");
    let visits: Vec<(i64, i64, i64)> = expected["visits"]
        .as_array()
        .ok_or("visits")?
        .iter()
        .map(|v| Ok((int(&v[0])?, int(&v[1])?, int(&v[2])?)))
        .collect::<Result<_, Box<dyn Error>>>()?;
    assert_eq!(actual.visits, visits, "{name} visits");
    Ok(())
}

/// Runs `next_step` and compares with `{"ok": ...}` or `{"error": ..., "message": ...}`.
fn check_step(
    config: &StaircaseConfig,
    staircase: &[(i64, bool)],
    refine: &[(i64, bool)],
    result: &Value,
    name: &str,
) -> TestResult {
    let got = next_step(config, staircase, refine);
    if let Some(ok) = result.get("ok") {
        let step = got.map_err(|e| format!("{name}: unexpected error {e}"))?;
        assert_step(&step, ok, name)
    } else {
        assert_eq!(result["error"], "ValueError", "{name}");
        let err = got.err().ok_or_else(|| format!("{name}: no error"))?;
        assert_eq!(
            err.message(),
            result["message"].as_str().unwrap_or_default(),
            "{name}"
        );
        Ok(())
    }
}

#[test]
fn config_cases_match_python() -> TestResult {
    let doc = load()?;
    for case in cases(&doc, "config_cases")? {
        let name = case["name"].as_str().ok_or("name")?;
        if let Some(protocol) = case.get("protocol") {
            let got = StaircaseConfig::from_protocol(&protocol_fields(protocol));
            check_config(got, &case["result"], name)?;
        } else {
            let got = config_literal(&case["config"])?.validate();
            check_config(got, &case["result"], name)?;
        }
    }
    Ok(())
}

fn check_config(
    got: Result<StaircaseConfig, wg_experiments::stats::ValueError>,
    result: &Value,
    name: &str,
) -> TestResult {
    if let Some(ok) = result.get("ok") {
        assert_config(&got.map_err(|e| format!("{name}: {e}"))?, ok, name)
    } else {
        let err = got.err().ok_or_else(|| format!("{name}: no error"))?;
        assert_eq!(
            err.message(),
            result["message"].as_str().unwrap_or_default(),
            "{name}"
        );
        Ok(())
    }
}

#[test]
fn cases_match_python() -> TestResult {
    let doc = load()?;
    for case in cases(&doc, "cases")? {
        let name = case["name"].as_str().ok_or("name")?;
        let config = config_literal(&case["config"])?;
        let staircase = history(&case["staircase"])?;
        let refine = history(&case["refine"])?;
        check_step(&config, &staircase, &refine, &case["result"], name)?;
    }
    Ok(())
}

#[test]
fn simulated_runs_match_python_after_every_prefix() -> TestResult {
    let doc = load()?;
    for run in cases(&doc, "simulated")? {
        let name = run["name"].as_str().ok_or("name")?;
        let config = config_literal(&run["config"])?;
        let staircase = history(&run["staircase"])?;
        let refine = history(&run["refine"])?;
        for prefix in run["prefixes"].as_array().ok_or("prefixes")? {
            let n = usize::try_from(int(&prefix["staircase"])?)?;
            let m = usize::try_from(int(&prefix["refine"])?)?;
            let step = next_step(&config, &staircase[..n], &refine[..m])?;
            assert_step(&step, &prefix["step"], &format!("{name} prefix {n}/{m}"))?;
        }
    }
    Ok(())
}

#[test]
fn phase_names_match_python() {
    assert_eq!(Phase::Staircase.as_str(), "staircase");
    assert_eq!(Phase::Refine.as_str(), "refine");
    assert_eq!(Phase::Done.as_str(), "done");
}
