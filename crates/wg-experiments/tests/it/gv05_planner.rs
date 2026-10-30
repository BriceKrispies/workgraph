//! GV-05: `wg_experiments::planner` reproduces the pure helpers of `golden/expstats/planner.json`
//! exactly: summaries, counts, band rows, the size transform, the clustered size-fit intervals,
//! the size fit, the seeded pick and the nearest unused task. Every float is compared with `==`.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;

use serde_json::Value;
use wg_experiments::planner::{
    BandRow, FitRow, SizeFitOutcome, SizeTrial, Task, band_rows, lines, lines_interval,
    nearest_unused, pick, size_fit, sorted_counts, summary,
};

type TestResult = Result<(), Box<dyn Error>>;

fn load() -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/expstats")
        .join("planner.json");
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

/// A JSON float, including the strings the generator uses for non-finite values.
fn float(value: &Value) -> Result<f64, Box<dyn Error>> {
    value
        .as_f64()
        .ok_or_else(|| format!("not a float: {value}").into())
}

fn floats(value: &Value) -> Result<Vec<f64>, Box<dyn Error>> {
    value
        .as_array()
        .ok_or("expected an array")?
        .iter()
        .map(float)
        .collect()
}

fn int(value: &Value) -> Result<i64, Box<dyn Error>> {
    value
        .as_i64()
        .ok_or_else(|| format!("not an int: {value}").into())
}

fn opt_float(value: &Value) -> Result<Option<f64>, Box<dyn Error>> {
    match value {
        Value::Null => Ok(None),
        other => Ok(Some(float(other)?)),
    }
}

/// `{"ok": v}` yields `v`; an error result fails the test.
fn ok(result: &Value) -> Result<&Value, Box<dyn Error>> {
    result
        .get("ok")
        .ok_or_else(|| format!("expected ok, got {result}").into())
}

fn cases<'a>(doc: &'a Value, key: &str) -> Result<&'a Vec<Value>, Box<dyn Error>> {
    let cases = doc[key]
        .as_array()
        .ok_or_else(|| format!("{key} is not an array"))?;
    assert!(!cases.is_empty(), "{key} is empty");
    Ok(cases)
}

#[test]
fn summary_matches_python() -> TestResult {
    let doc = load()?;
    for case in cases(&doc, "summary")? {
        let values = floats(&case["values"])?;
        let got = summary(&values);
        let want = ok(&case["result"])?;
        assert_eq!(
            got.median,
            opt_float(&want["median"])?,
            "median of {values:?}"
        );
        assert_eq!(got.mean, opt_float(&want["mean"])?, "mean of {values:?}");
    }
    Ok(())
}

#[test]
fn sorted_counts_match_python() -> TestResult {
    let doc = load()?;
    for case in cases(&doc, "sorted_counts")? {
        let values: Vec<String> = case["values"]
            .as_array()
            .ok_or("values")?
            .iter()
            .map(|v| v.as_str().unwrap_or_default().to_string())
            .collect();
        let refs: Vec<&str> = values.iter().map(String::as_str).collect();
        let want = ok(&case["result"])?
            .as_object()
            .ok_or("sorted_counts result is not an object")?;
        let expected: Vec<(String, usize)> = want
            .iter()
            .map(|(k, v)| Ok((k.clone(), usize::try_from(int(v)?)?)))
            .collect::<Result<_, Box<dyn Error>>>()?;
        assert_eq!(sorted_counts(&refs), expected, "{values:?}");
    }
    Ok(())
}

#[test]
fn band_rows_match_python() -> TestResult {
    let doc = load()?;
    for case in cases(&doc, "band_rows")? {
        let graded: Vec<(Option<i64>, bool)> = case["trials"]
            .as_array()
            .ok_or("trials")?
            .iter()
            .map(|t| {
                let band = match &t["band"] {
                    Value::Null => None,
                    v => Some(int(v)?),
                };
                Ok((band, t["outcome"] == "pass"))
            })
            .collect::<Result<_, Box<dyn Error>>>()?;
        let got = band_rows(&graded)?;
        let want = ok(&case["result"])?.as_array().ok_or("band rows")?;
        assert_eq!(got.len(), want.len(), "row count");
        for (row, expected) in got.iter().zip(want) {
            let BandRow {
                band,
                n,
                passes,
                rate,
                wilson,
            } = *row;
            assert_eq!(band, int(&expected["band"])?, "band");
            assert_eq!(n as i64, int(&expected["n"])?, "n");
            assert_eq!(passes as i64, int(&expected["passes"])?, "passes");
            assert_eq!(rate, float(&expected["rate"])?, "rate");
            let bounds = floats(&expected["wilson"])?;
            assert_eq!(wilson, (bounds[0], bounds[1]), "wilson");
        }
    }
    Ok(())
}

#[test]
fn lines_matches_python() -> TestResult {
    let doc = load()?;
    for case in cases(&doc, "lines")? {
        let x = opt_float(&case["x"])?;
        let want = opt_float(ok(&case["result"])?)?;
        assert_eq!(lines(x), want, "lines({x:?})");
    }
    Ok(())
}

#[test]
fn lines_interval_matches_python() -> TestResult {
    let doc = load()?;
    for case in cases(&doc, "lines_interval")? {
        let rows: Vec<FitRow> = case["rows"]
            .as_array()
            .ok_or("rows")?
            .iter()
            .map(|r| {
                Ok(FitRow {
                    task_id: r["task_id"].as_str().unwrap_or_default().to_string(),
                    x: float(&r["x"])?,
                    y: int(&r["y"])?,
                })
            })
            .collect::<Result<_, Box<dyn Error>>>()?;
        let seed = i128::from(int(&case["seed"])?);
        let got = lines_interval(&rows, float(&case["p"])?, seed)?;
        let want = ok(&case["result"])?;
        let expected = match want {
            Value::Null => None,
            pair => {
                let bounds = floats(pair)?;
                Some((bounds[0], bounds[1]))
            }
        };
        assert_eq!(
            got,
            expected,
            "interval p={} seed={seed}",
            float(&case["p"])?
        );
    }
    Ok(())
}

fn size_trials(value: &Value) -> Result<Vec<SizeTrial>, Box<dyn Error>> {
    value
        .as_array()
        .ok_or("trials")?
        .iter()
        .map(|t| {
            Ok(SizeTrial {
                task_id: t["task_id"].as_str().unwrap_or_default().to_string(),
                passed: t["outcome"] == "pass",
                src_lines: opt_float(&t["src"])?,
            })
        })
        .collect()
}

#[test]
fn size_fit_matches_python() -> TestResult {
    let doc = load()?;
    for case in cases(&doc, "size_fit")? {
        let trials = size_trials(&case["trials"])?;
        let seed = i128::from(int(&case["seed"])?);
        let got = size_fit(&trials, seed)?;
        let want = ok(&case["result"])?.as_array().ok_or("size fit pair")?;
        match (got, &want[0], &want[1]) {
            (SizeFitOutcome::Rejected(reason), Value::Null, reason_json) => {
                assert_eq!(Some(reason.as_str()), reason_json.as_str(), "reason");
            }
            (SizeFitOutcome::Fitted(fit), fit_json, Value::Null) => {
                assert_eq!(fit.intercept, float(&fit_json["intercept"])?, "intercept");
                assert_eq!(fit.slope, float(&fit_json["slope"])?, "slope");
                assert_eq!(fit.converged, fit_json["converged"] == true, "converged");
                assert_eq!(fit.n as i64, int(&fit_json["n"])?, "n");
                assert_eq!(fit.x50, opt_float(&fit_json["x50"])?, "x50");
                assert_eq!(fit.x80, opt_float(&fit_json["x80"])?, "x80");
                assert_eq!(fit.lines50, opt_float(&fit_json["lines50"])?, "lines50");
                assert_eq!(fit.lines80, opt_float(&fit_json["lines80"])?, "lines80");
                assert_eq!(fit.ci50, pair(&fit_json["ci50"])?, "ci50");
                assert_eq!(fit.ci80, pair(&fit_json["ci80"])?, "ci80");
            }
            (got, _, _) => return Err(format!("size fit mismatch: {got:?} vs {want:?}").into()),
        }
    }
    Ok(())
}

/// A JSON `[low, high]` pair, or `None`.
fn pair(value: &Value) -> Result<Option<(f64, f64)>, Box<dyn Error>> {
    match value {
        Value::Null => Ok(None),
        other => {
            let bounds = floats(other)?;
            Ok(Some((bounds[0], bounds[1])))
        }
    }
}

fn tasks_of(value: &Value) -> Result<Vec<Task>, Box<dyn Error>> {
    value
        .as_array()
        .ok_or("tasks")?
        .iter()
        .map(|t| {
            Ok(Task {
                key: t["key"].as_str().unwrap_or_default().to_string(),
                band: int(&t["band"])?,
            })
        })
        .collect()
}

#[test]
fn pick_matches_python() -> TestResult {
    let doc = load()?;
    for case in cases(&doc, "pick")? {
        let seed = i128::from(int(&case["seed"])?);
        let next_seq = i128::from(int(&case["next_seq"])?);
        let tasks = tasks_of(&case["tasks"])?;
        let got = pick(seed, next_seq, &tasks).map(|t| t.key.as_str());
        assert_eq!(
            got,
            ok(&case["result"])?.as_str(),
            "pick seed={seed} next_seq={next_seq}"
        );
    }
    Ok(())
}

#[test]
fn nearest_unused_matches_python() -> TestResult {
    let doc = load()?;
    for case in cases(&doc, "nearest_unused")? {
        let pool = tasks_of(&case["pool"])?;
        let used: BTreeSet<String> = case["used"]
            .as_array()
            .ok_or("used")?
            .iter()
            .map(|k| k.as_str().unwrap_or_default().to_string())
            .collect();
        let band = int(&case["band"])?;
        let got = nearest_unused(&pool, &used, band).map(|(chosen, tasks)| {
            (
                chosen,
                tasks.iter().map(|t| t.key.clone()).collect::<Vec<_>>(),
            )
        });
        let want = ok(&case["result"])?;
        let expected = match want {
            Value::Null => None,
            pair_json => {
                let chosen = int(&pair_json[0])?;
                let keys = pair_json[1]
                    .as_array()
                    .ok_or("keys")?
                    .iter()
                    .map(|k| k.as_str().unwrap_or_default().to_string())
                    .collect();
                Some((chosen, keys))
            }
        };
        assert_eq!(got, expected, "nearest_unused band={band} used={used:?}");
    }
    Ok(())
}
