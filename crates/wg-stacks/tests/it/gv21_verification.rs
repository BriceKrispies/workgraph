//! GV-21: the verification vectors of `golden/stackplan/` (prefix construction for bisection, the
//! bisection search, and the regression judgement with the verify and bisect entry decisions) and
//! GV-02: the regression vectors of `golden/signatures/regression.json`.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;

use serde_json::{Value, json};
use wg_stacks::bisect::{BisectEntry, Bisection, Entry, bisect_entry, prefix_shas};
use wg_stacks::regression::{
    BaseOutcomes, CheckOutcome, RunCheck, RunResult, errored_checks, failed_checks, regressions,
};
use wg_stacks::verify::{VerifyDecision, verify_decision};
use wg_types::VerificationStatus;
use wg_verify::Baseline;

type TestResult = Result<(), Box<dyn Error>>;

fn golden(path: &str) -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden")
        .join(path);
    Ok(serde_json::from_str(&std::fs::read_to_string(path)?)?)
}

fn cases(vectors: &Value) -> Result<&Vec<Value>, Box<dyn Error>> {
    let cases = vectors["cases"].as_array().ok_or("cases is not an array")?;
    assert!(!cases.is_empty());
    Ok(cases)
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn str_map(value: &Value) -> BTreeMap<String, String> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn status(value: &Value) -> Result<VerificationStatus, Box<dyn Error>> {
    Ok(value.as_str().ok_or("status")?.parse()?)
}

fn entry(task: &str, merges: &Value) -> Entry {
    Entry {
        item_id: format!("item-{task}"),
        task_id: format!("task-{task}"),
        task_key: task.to_string(),
        title: String::new(),
        merges: merges.as_object().cloned().unwrap_or_default(),
    }
}

/// The pairs of a regression result as a JSON object, keys sorted as the vectors store them.
fn as_object(found: &[(String, Vec<String>)]) -> Value {
    let sorted: BTreeMap<&str, &Vec<String>> = found.iter().map(|(n, s)| (n.as_str(), s)).collect();
    json!(sorted)
}

#[test]
fn prefix_shas_vectors() -> TestResult {
    let vectors = golden("stackplan/prefix_shas.json")?;
    for case in cases(&vectors)? {
        let id = case["id"].as_str().ok_or("id")?;
        let bases = str_map(&case["bases"]);
        let sequence: Vec<Entry> = case["sequence"]
            .as_array()
            .ok_or("sequence")?
            .iter()
            .map(|e| entry(e["task"].as_str().unwrap_or_default(), &e["merges"]))
            .collect();
        let expected = case["prefix_shas"].as_array().ok_or("prefix_shas")?;
        assert_eq!(expected.len(), sequence.len() + 1, "{id}");
        for (position, want) in expected.iter().enumerate() {
            let got = prefix_shas(&bases, &sequence, position)?;
            assert_eq!(json!(got), *want, "{id} position {position}");
        }
    }
    Ok(())
}

#[test]
fn bisection_search_vectors() -> TestResult {
    let vectors = golden("stackplan/bisection_search.json")?;
    for case in cases(&vectors)? {
        let id = case["id"].as_str().ok_or("id")?;
        let keys = strings(&case["sequence"]);
        let passes: Vec<bool> = case["passes"]
            .as_array()
            .ok_or("passes")?
            .iter()
            .map(|v| v.as_bool().unwrap_or(false))
            .collect();
        let errored: Vec<bool> = case["errored"]
            .as_array()
            .ok_or("errored")?
            .iter()
            .map(|v| v.as_bool().unwrap_or(false))
            .collect();
        // The vector does not store each entry's merges, only the shas each probe checked out; the
        // prefix construction is pinned by `prefix_shas_vectors`, so the search is replayed here
        // over the probe positions and outcomes.
        let mut search = Bisection::new(keys.len(), "tip".to_string());
        let mut probes = Vec::new();
        let mut outcome = None;
        while let Some(middle) = search.next_probe() {
            let recorded = &case["probes"][probes.len()];
            assert_eq!(recorded["position"], json!(middle), "{id} probe position");
            if errored[middle] {
                probes.push(json!({"position": middle, "errored": true}));
                outcome = Some(json!({"outcome": "errored", "stage": "bisect"}));
                break;
            }
            let passed = passes[middle];
            probes.push(json!({"position": middle, "passed": passed}));
            search = search.record(middle, passed, format!("run-{middle}"));
        }
        let outcome = outcome.unwrap_or_else(|| {
            let position = search.position();
            json!({"outcome": "ejected", "position": position, "culprit": keys[position - 1]})
        });
        assert_eq!(outcome, case["result"], "{id} result");
        let recorded: Vec<Value> = case["probes"]
            .as_array()
            .ok_or("probes")?
            .iter()
            .map(|p| {
                let mut p = p.clone();
                if let Some(map) = p.as_object_mut() {
                    map.remove("shas");
                }
                p
            })
            .collect();
        assert_eq!(probes, recorded, "{id} probes");
    }
    Ok(())
}

struct Judgement {
    run: RunResult,
    base: BaseOutcomes,
    baselines: BTreeMap<String, Baseline>,
}

fn judgement(case: &Value) -> Result<Judgement, Box<dyn Error>> {
    let mut checks = Vec::new();
    for check in case["checks"].as_array().ok_or("checks")? {
        checks.push(RunCheck {
            name: check["name"].as_str().ok_or("name")?.to_string(),
            required: check["required"].as_bool().ok_or("required")?,
            status: status(&check["status"])?,
            repository_id: check["repository_id"].as_str().map(str::to_string),
            failure_signatures: strings(&check["failure_signatures"]),
            output_excerpt: String::new(),
        });
    }
    let mut baselines = BTreeMap::new();
    for (name, mode) in case["spec_baselines"].as_object().ok_or("spec_baselines")? {
        let mode = Baseline::parse(mode.as_str().ok_or("baseline")?).ok_or("baseline")?;
        baselines.insert(name.clone(), mode);
    }
    // The generator built the base in the order of the run's checks.
    let mut pairs = Vec::new();
    for check in &checks {
        if let Some(data) = case["base"].get(&check.name) {
            pairs.push((
                check.name.clone(),
                CheckOutcome {
                    status: status(&data["status"])?,
                    signatures: strings(&data["signatures"]),
                    run_id: Some("base-1".into()),
                    excerpt: String::new(),
                },
            ));
        }
    }
    let required_passed = checks
        .iter()
        .filter(|c| c.required)
        .all(|c| c.status == VerificationStatus::Passed);
    Ok(Judgement {
        run: RunResult {
            id: "run-1".into(),
            required_passed,
            checks,
        },
        base: BaseOutcomes::from_pairs(pairs),
        baselines,
    })
}

fn verify_json(decision: &VerifyDecision) -> Value {
    match decision {
        VerifyDecision::Errored(errored) => json!({
            "outcome": "errored",
            "stage": errored.stage.as_str(),
            "checks": errored.checks(),
        }),
        VerifyDecision::Passed {
            reason,
            tolerated: None,
        } => json!({"outcome": "passed", "reason": reason}),
        VerifyDecision::Passed {
            reason,
            tolerated: Some(found),
        } => json!({"outcome": "passed", "reason": reason, "tolerated": as_object(found)}),
        VerifyDecision::Bisect(found) => {
            let mut sorted = found.clone();
            sorted.sort();
            json!({"outcome": "bisect", "failing": sorted})
        }
    }
}

#[test]
fn regression_judgement_vectors() -> TestResult {
    let vectors = golden("stackplan/regression_judgement.json")?;
    for case in cases(&vectors)? {
        let id = case["id"].as_str().ok_or("id")?;
        let j = judgement(case)?;
        let checks = &j.run.checks;
        assert_eq!(
            as_object(&regressions(checks, Some(&j.base), &j.baselines)),
            case["regressions"],
            "{id} regressions"
        );
        let failed: BTreeMap<String, Option<String>> = failed_checks(checks).into_iter().collect();
        assert_eq!(json!(failed), case["failed"], "{id} failed");
        let mut errored: Vec<&str> = errored_checks(checks)
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        errored.sort_unstable();
        assert_eq!(json!(errored), case["errored"], "{id} errored");
        assert_eq!(
            verify_json(&verify_decision(&j.run, &j.base, &j.baselines)),
            case["verify"],
            "{id} verify"
        );
        let sequence_len = case["sequence_len"].as_u64().ok_or("sequence_len")? as usize;
        let entry = match bisect_entry(sequence_len, &j.run.id, checks, &j.base, &j.baselines) {
            BisectEntry::Search => json!({"outcome": "search"}),
            BisectEntry::FailedOnBase {
                reason,
                failed_checks,
                ..
            } => {
                json!({"outcome": "failed_on_base", "reason": reason, "failed_checks": failed_checks})
            }
        };
        assert_eq!(entry, case["bisect_entry"], "{id} bisect entry");
    }
    Ok(())
}

#[test]
fn gv02_regression_vectors() -> TestResult {
    let vectors = golden("signatures/regression.json")?;
    for case in cases(&vectors)? {
        let id = case["id"].as_str().ok_or("id")?;
        let mut checks = Vec::new();
        for check in case["checks"].as_array().ok_or("checks")? {
            checks.push(RunCheck {
                name: check["name"].as_str().ok_or("name")?.to_string(),
                required: check["required"].as_bool().unwrap_or(true),
                status: match check.get("status") {
                    Some(value) => status(value)?,
                    None => VerificationStatus::Failed,
                },
                repository_id: None,
                failure_signatures: strings(&check["signatures"]),
                output_excerpt: String::new(),
            });
        }
        let mut baselines = BTreeMap::new();
        for spec in case["specs"].as_array().ok_or("specs")? {
            let mode =
                Baseline::parse(spec["baseline"].as_str().ok_or("baseline")?).ok_or("baseline")?;
            baselines.insert(spec["name"].as_str().ok_or("name")?.to_string(), mode);
        }
        let base = if case["base_is_none"].as_bool().ok_or("base_is_none")? {
            None
        } else {
            let mut pairs = Vec::new();
            for (name, data) in case["base"].as_object().ok_or("base")? {
                pairs.push((
                    name.clone(),
                    CheckOutcome {
                        status: status(&data["status"])?,
                        signatures: strings(&data["signatures"]),
                        run_id: data["run_id"].as_str().map(str::to_string),
                        excerpt: String::new(),
                    },
                ));
            }
            Some(BaseOutcomes::from_pairs(pairs))
        };
        let found = regressions(&checks, base.as_ref(), &baselines);
        assert_eq!(as_object(&found), case["expected"], "{id}");
    }
    Ok(())
}
