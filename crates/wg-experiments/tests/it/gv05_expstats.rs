//! GV-05: the pure parts of `src/workgraph/experiments/stats.py` and `predictor.py` reproduce
//! `golden/expstats/{stats_wilson,stats_logistic,stats_bootstrap,stats_calibration,predictor}.json`
//! exactly. Floats are compared with `==`, except the statistics computed through `exp`, `log` or
//! `log1p` (the logistic fit, its predictions and its size-at-probability, and the predictor's
//! probability), which use a relative tolerance of `1e-12` (Q9, see `common`). The port keeps the
//! Python operation order (R5), so those values normally match bit for bit.

use std::error::Error;
use std::path::PathBuf;

use serde_json::Value;
use wg_experiments::predictor::{features_of, probability};
use wg_experiments::stats::{
    BootstrapConfig, DEFAULT_MAX_ITER, DEFAULT_TOL, LogisticFit, Penalty, Z_95, bootstrap_clusters,
    bootstrap_interval, brier, fit_logistic, predict, quantile, reliability, size_at_probability,
    wilson,
};

use crate::common;
use common::{assert_close_relative, assert_close_relative_slice};

type TestResult = Result<(), Box<dyn Error>>;
/// A bootstrap result: `Ok(None)` or the interval, or the Python error message.
type Outcome = Result<Option<(f64, f64, usize)>, String>;
/// `(lo, hi, n, mean_predicted, observed)` of one calibration bin.
type Bin = (f64, f64, usize, Option<f64>, Option<f64>);

fn load(name: &str) -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/expstats")
        .join(name);
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn cases(doc: &Value) -> Result<&Vec<Value>, Box<dyn Error>> {
    let cases = doc["cases"].as_array().ok_or("cases is not an array")?;
    assert!(!cases.is_empty());
    Ok(cases)
}

/// A JSON float, including the strings the generator uses for non-finite values.
fn float(value: &Value) -> Result<f64, Box<dyn Error>> {
    match value {
        Value::Number(n) => n.as_f64().ok_or_else(|| format!("not a float: {n}").into()),
        Value::String(s) => match s.as_str() {
            "NaN" => Ok(f64::NAN),
            "Infinity" => Ok(f64::INFINITY),
            "-Infinity" => Ok(f64::NEG_INFINITY),
            other => Err(format!("not a float: {other}").into()),
        },
        other => Err(format!("not a float: {other}").into()),
    }
}

fn floats(value: &Value) -> Result<Vec<f64>, Box<dyn Error>> {
    value
        .as_array()
        .ok_or("expected an array of floats")?
        .iter()
        .map(float)
        .collect()
}

fn rows(value: &Value) -> Result<Vec<Vec<f64>>, Box<dyn Error>> {
    value
        .as_array()
        .ok_or("expected an array of rows")?
        .iter()
        .map(floats)
        .collect()
}

fn ints(value: &Value) -> Result<Vec<i64>, Box<dyn Error>> {
    value
        .as_array()
        .ok_or("expected an array of ints")?
        .iter()
        .map(|v| v.as_i64().ok_or_else(|| format!("not an int: {v}").into()))
        .collect()
}

fn penalty_of(value: &Value) -> Result<Penalty, Box<dyn Error>> {
    match value {
        Value::Array(_) => Ok(Penalty::PerCoefficient(floats(value)?)),
        _ => Ok(Penalty::Uniform(float(value)?)),
    }
}

/// `{"ok": v}` or `{"error": "ValueError", "message": m}`.
enum Expected<'a> {
    Value(&'a Value),
    Error(&'a str),
}

fn expected(result: &Value) -> Result<Expected<'_>, Box<dyn Error>> {
    if let Some(ok) = result.get("ok") {
        return Ok(Expected::Value(ok));
    }
    assert_eq!(result["error"], "ValueError", "only ValueError is expected");
    Ok(Expected::Error(
        result["message"]
            .as_str()
            .ok_or("message is not a string")?,
    ))
}

fn int_field(value: &Value, key: &str) -> Result<i64, Box<dyn Error>> {
    value[key]
        .as_i64()
        .ok_or_else(|| format!("{key} is not an int").into())
}

fn usize_field(value: &Value, key: &str) -> Result<usize, Box<dyn Error>> {
    Ok(usize::try_from(int_field(value, key)?)?)
}

fn fit_matches(fit: &LogisticFit, expected: &Value, name: &str) -> TestResult {
    // The coefficients and the likelihood come through `exp`/`log1p` in the Newton iterations.
    assert_close_relative_slice(
        &fit.coef,
        &floats(&expected["coef"])?,
        &format!("{name} coef"),
    );
    assert_eq!(fit.n, usize_field(expected, "n")?, "{name} n");
    assert_eq!(
        fit.converged,
        expected["converged"].as_bool().ok_or("converged")?,
        "{name} converged"
    );
    assert_eq!(
        fit.iterations,
        usize_field(expected, "iterations")?,
        "{name} iterations"
    );
    assert_close_relative(
        fit.log_likelihood,
        float(&expected["log_likelihood"])?,
        &format!("{name} log_likelihood"),
    );
    assert_eq!(fit.penalty, floats(&expected["penalty"])?, "{name} penalty");
    Ok(())
}

#[test]
fn wilson_matches_python() -> TestResult {
    let doc = load("stats_wilson.json")?;
    for case in cases(&doc)? {
        let name = case["name"].as_str().ok_or("name")?;
        let z = match &case["z"] {
            Value::Null => Z_95,
            value => float(value)?,
        };
        let got = wilson(int_field(case, "successes")?, int_field(case, "n")?, z);
        match expected(&case["result"])? {
            Expected::Value(value) => {
                let want = floats(value)?;
                let (lo, hi) = got.map_err(|e| format!("{name}: {e}"))?;
                assert_eq!((lo, hi), (want[0], want[1]), "{name}");
            }
            Expected::Error(message) => {
                let err = got.err().ok_or_else(|| format!("{name}: no error"))?;
                assert_eq!(err.message(), message, "{name}");
            }
        }
    }
    Ok(())
}

#[test]
fn logistic_fits_match_python() -> TestResult {
    let doc = load("stats_logistic.json")?;
    for case in cases(&doc)? {
        let name = case["name"].as_str().ok_or("name")?;
        let x = rows(&case["x"])?;
        let y = ints(&case["y"])?;
        let penalty = penalty_of(&case["penalty"])?;
        let max_iter = int_field(case, "max_iter")?;
        let fitted = fit_logistic(&x, &y, &penalty, max_iter, DEFAULT_TOL);
        match expected(&case["fit"])? {
            Expected::Value(value) => {
                let fit = fitted.map_err(|e| format!("{name}: {e}"))?;
                fit_matches(&fit, value, name)?;
                for point in case["predict"].as_array().ok_or("predict")? {
                    let got = predict(&fit, &floats(&point["x"])?)?;
                    assert_close_relative(
                        got,
                        float(&point["p"])?,
                        &format!("{name} predict {}", point["x"]),
                    );
                }
                if let Some(map) = case["size_at_probability"].as_object() {
                    for (key, want) in map {
                        let p: f64 = key.parse()?;
                        let got = size_at_probability(&fit, p)?;
                        match want {
                            Value::Null => assert_eq!(got, None, "{name} size {key}"),
                            _ => {
                                let got = got.ok_or_else(|| format!("{name} size {key}: None"))?;
                                assert_close_relative(
                                    got,
                                    float(want)?,
                                    &format!("{name} size {key}"),
                                );
                            }
                        }
                    }
                }
            }
            Expected::Error(message) => {
                let err = fitted.err().ok_or_else(|| format!("{name}: no error"))?;
                assert_eq!(err.message(), message, "{name}");
            }
        }
    }
    Ok(())
}

#[test]
fn logistic_errors_match_python() -> TestResult {
    let doc = load("stats_logistic.json")?;
    let errors = doc["errors"].as_array().ok_or("errors")?;
    assert!(!errors.is_empty());
    for case in errors {
        let name = case["name"].as_str().ok_or("name")?;
        if let Some(p_values) = case.get("p_values") {
            let fit = fit_logistic(
                &[vec![0.0], vec![1.0]],
                &[0, 1],
                &Penalty::Uniform(1.0),
                DEFAULT_MAX_ITER,
                DEFAULT_TOL,
            )?;
            let results = case["results"].as_array().ok_or("results")?;
            for (p, result) in floats(p_values)?.into_iter().zip(results) {
                let Expected::Error(message) = expected(result)? else {
                    return Err(format!("{name}: expected an error for p={p}").into());
                };
                let err = size_at_probability(&fit, p)
                    .err()
                    .ok_or_else(|| format!("{name}: no error for p={p}"))?;
                assert_eq!(err.message(), message, "{name} p={p}");
            }
            continue;
        }
        let x = rows(&case["x"])?;
        let y = ints(&case["y"])?;
        // The generator stores `max_iter` only for the cases that set it; `max_iter_zero` is
        // the one error case that passes `max_iter=0` without recording it.
        let max_iter = match (case.get("max_iter"), name) {
            (Some(value), _) => value.as_i64().ok_or("max_iter")?,
            (None, "max_iter_zero") => 0,
            (None, _) => DEFAULT_MAX_ITER,
        };
        let fitted = fit_logistic(
            &x,
            &y,
            &penalty_of(&case["penalty"])?,
            max_iter,
            DEFAULT_TOL,
        );
        match expected(&case["result"])? {
            Expected::Value(_) => return Err(format!("{name}: expected a success").into()),
            Expected::Error(message) => {
                let err = fitted.err().ok_or_else(|| format!("{name}: no error"))?;
                assert_eq!(err.message(), message, "{name}");
            }
        }
    }
    Ok(())
}

fn statistic_mean(sample: &[&f64]) -> Option<f64> {
    let total = wg_experiments::stats::py_sum(sample.iter().map(|v| **v));
    Some(total / sample.len() as f64)
}

fn statistic_skip_mod3(sample: &[&f64]) -> Option<f64> {
    let total = wg_experiments::stats::py_sum(sample.iter().map(|v| **v));
    if total % 3.0 == 0.0 {
        None
    } else {
        Some(total)
    }
}

fn statistic_always_none(_sample: &[&f64]) -> Option<f64> {
    None
}

fn distinct_first_field(sample: &[&(String, i64)]) -> Option<f64> {
    let distinct: std::collections::BTreeSet<&str> =
        sample.iter().map(|row| row.0.as_str()).collect();
    Some(distinct.len() as f64)
}

fn bootstrap_result(case: &Value) -> Result<Outcome, Box<dyn Error>> {
    let config = BootstrapConfig {
        n: int_field(case, "n")?,
        seed: i128::from(int_field(case, "seed").unwrap_or(0)),
        level: float(&case["level"])?,
    };
    let statistic = case["statistic"].as_str().ok_or("statistic")?;
    let outcome = if case["cluster_first_field"].as_bool().unwrap_or(false) {
        let rows: Vec<(String, i64)> = case["rows"]
            .as_array()
            .ok_or("rows")?
            .iter()
            .map(|row| {
                Ok((
                    row[0].as_str().ok_or("cluster key")?.to_string(),
                    row[1].as_i64().ok_or("cluster value")?,
                ))
            })
            .collect::<Result<_, Box<dyn Error>>>()?;
        bootstrap_clusters(&rows, |row| row.0.clone(), distinct_first_field, &config)
    } else {
        let rows = floats(&case["rows"])?;
        match statistic {
            "mean" => bootstrap_interval(&rows, statistic_mean, &config),
            "skip_mod3" => bootstrap_interval(&rows, statistic_skip_mod3, &config),
            "always_none" => bootstrap_interval(&rows, statistic_always_none, &config),
            other => return Err(format!("unknown statistic {other}").into()),
        }
    };
    Ok(outcome.map_err(|e| e.message().to_string()))
}

#[test]
fn bootstrap_matches_python() -> TestResult {
    let doc = load("stats_bootstrap.json")?;
    for case in cases(&doc)? {
        let name = case["name"].as_str().ok_or("name")?;
        let got = bootstrap_result(case)?;
        match expected(&case["result"])? {
            Expected::Value(Value::Null) => {
                assert_eq!(got, Ok(None), "{name}");
            }
            Expected::Value(value) => {
                let want = floats(value)?;
                let got = got.map_err(|e| format!("{name}: {e}"))?;
                let Some((lo, hi, produced)) = got else {
                    return Err(format!("{name}: no interval").into());
                };
                assert_eq!((lo, hi), (want[0], want[1]), "{name}");
                assert_eq!(produced as f64, want[2], "{name} produced");
            }
            Expected::Error(message) => {
                assert_eq!(got.err().as_deref(), Some(message), "{name}");
            }
        }
    }
    Ok(())
}

fn reliability_bins(result: &Value) -> Result<Vec<Bin>, Box<dyn Error>> {
    result
        .as_array()
        .ok_or("bins")?
        .iter()
        .map(|bin| {
            let optional = |key: &str| -> Result<Option<f64>, Box<dyn Error>> {
                match &bin[key] {
                    Value::Null => Ok(None),
                    value => Ok(Some(float(value)?)),
                }
            };
            Ok((
                float(&bin["lo"])?,
                float(&bin["hi"])?,
                usize_field(bin, "n")?,
                optional("mean_predicted")?,
                optional("observed")?,
            ))
        })
        .collect()
}

fn check_brier(name: &str, case: &Value, result: &Value) -> TestResult {
    let got = brier(&floats(&case["probs"])?, &ints(&case["outcomes"])?);
    match expected(result)? {
        Expected::Value(value) => {
            assert_eq!(
                got.map_err(|e| format!("{name}: {e}"))?,
                float(value)?,
                "{name}"
            );
        }
        Expected::Error(message) => {
            let err = got.err().ok_or_else(|| format!("{name}: no error"))?;
            assert_eq!(err.message(), message, "{name}");
        }
    }
    Ok(())
}

fn check_reliability(name: &str, case: &Value, result: &Value) -> TestResult {
    let bins = case["bins"].as_i64().ok_or("bins")?;
    let got = reliability(&floats(&case["probs"])?, &ints(&case["outcomes"])?, bins);
    match expected(result)? {
        Expected::Value(value) => {
            let got = got.map_err(|e| format!("{name}: {e}"))?;
            let want = reliability_bins(value)?;
            let got: Vec<Bin> = got
                .into_iter()
                .map(|bin| (bin.lo, bin.hi, bin.n, bin.mean_predicted, bin.observed))
                .collect();
            assert_eq!(got, want, "{name}");
        }
        Expected::Error(message) => {
            let err = got.err().ok_or_else(|| format!("{name}: no error"))?;
            assert_eq!(err.message(), message, "{name}");
        }
    }
    Ok(())
}

#[test]
fn calibration_matches_python() -> TestResult {
    let doc = load("stats_calibration.json")?;
    for case in cases(&doc)? {
        let name = case["name"].as_str().ok_or("name")?;
        if case.get("reliability").is_some() {
            check_brier(name, case, &case["brier"])?;
            check_reliability(name, case, &case["reliability"])?;
        } else if case.get("bins").is_some() {
            check_reliability(name, case, &case["result"])?;
        } else {
            check_brier(name, case, &case["result"])?;
        }
    }
    Ok(())
}

#[test]
fn predictor_helpers_match_python() -> TestResult {
    let doc = load("predictor.json")?;

    for case in doc["features_of"].as_array().ok_or("features_of")? {
        let packages = usize::try_from(int_field(case, "packages")?)?;
        let got = features_of(
            float(&case["src_lines"])?,
            float(&case["src_files"])?,
            packages,
        );
        let want = floats(&case["result"]["ok"])?;
        assert_eq!(got.to_vec(), want, "features_of {}", case["src_lines"]);
    }

    for case in doc["probability"].as_array().ok_or("probability")? {
        let got = probability(&floats(&case["coef"])?, &floats(&case["x"])?);
        match expected(&case["result"])? {
            Expected::Value(value) => {
                assert_close_relative(got?, float(value)?, "probability");
            }
            Expected::Error(message) => {
                assert_eq!(
                    got.err().map(|e| e.message().to_string()).as_deref(),
                    Some(message)
                );
            }
        }
    }

    for case in doc["percentile"].as_array().ok_or("percentile")? {
        let values = floats(&case["values"])?;
        let q = float(&case["q"])?;
        let got = quantile(&values, q);
        match expected(&case["result"])? {
            Expected::Value(value) => assert_eq!(got, Some(float(value)?), "percentile q={q}"),
            Expected::Error(_) => assert_eq!(got, None, "percentile q={q}"),
        }
    }
    Ok(())
}
