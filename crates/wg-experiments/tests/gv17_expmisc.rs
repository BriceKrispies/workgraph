//! GV-17: the pure parts of `src/workgraph/experiments/contract.py`, `briefs.py`, `escape.py` and
//! the replay selection in `replay.py` reproduce `golden/expmisc/{contract,briefs,escape,replay}.json`
//! exactly. Each case is compared as a whole JSON value; a failing case is named in the message.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;

use serde_json::{Map, Value, json};
use wg_experiments::briefs::{BriefContext, render};
use wg_experiments::contract::contract_problems;
use wg_experiments::escape::{ToolCall, find_escapes, normalize_path_text};
use wg_experiments::replay::{
    ReplayConfig, added_lines, band, changed_paths, expand_oracle_template, features,
    has_inline_tests, kind, manifest_dirs, match_glob, nearest, parse_name_status, parse_numstat,
    split_changes, template_inputs, tests_modified,
};

type TestResult = Result<(), Box<dyn Error>>;

/// The manifest-bearing tree of the generator (`TREE` in `tools/golden/gen_expmisc.py`).
const TREE: [&str; 13] = [
    "Cargo.toml",
    "crates/a/Cargo.toml",
    "crates/a/src/lib.rs",
    "crates/a/tests/t.rs",
    "crates/b/src/main.rs",
    "crates/b/src/x.rs",
    "pyproject.toml",
    "src/workgraph/mod.py",
    "tests/unit/test_mod.py",
    "web/package.json",
    "web/src/app.ts",
    "README.md",
    "uv.lock",
];

fn load(name: &str) -> Result<Vec<Value>, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/expmisc")
        .join(name);
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(path)?)?;
    let cases = doc["cases"].as_array().ok_or("cases is not an array")?;
    assert!(!cases.is_empty(), "{name} has no cases");
    Ok(cases.clone())
}

fn strings(value: &Value) -> Result<Vec<String>, Box<dyn Error>> {
    value
        .as_array()
        .ok_or("expected an array of strings")?
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("not a string: {item}").into())
        })
        .collect()
}

fn argvs(value: &Value) -> Result<Vec<Vec<String>>, Box<dyn Error>> {
    value
        .as_array()
        .ok_or("expected an array of argv lists")?
        .iter()
        .map(strings)
        .collect()
}

fn report(failures: &[String]) -> String {
    failures.join("\n")
}

fn pairs(value: &Value) -> Result<Vec<(String, String)>, Box<dyn Error>> {
    value
        .as_array()
        .ok_or("expected an array of pairs")?
        .iter()
        .map(|pair| {
            let items = strings(pair)?;
            match items.as_slice() {
                [a, b] => Ok((a.clone(), b.clone())),
                _ => Err(format!("not a pair: {pair}").into()),
            }
        })
        .collect()
}

type ContractInput = (Vec<String>, Vec<String>, Value);

fn contract_context(case: &Value) -> Result<ContractInput, Box<dyn Error>> {
    let input = &case["input"];
    Ok((
        strings(&input["tags"])?,
        strings(&input["acceptance_criteria"])?,
        input["metadata"].clone(),
    ))
}

#[test]
fn contract_vectors() -> TestResult {
    let mut failures = Vec::new();
    for case in load("contract.json")? {
        let id = case["id"].as_str().unwrap_or_default().to_owned();
        let (tags, criteria, metadata) = contract_context(&case)?;
        let problems = contract_problems(&tags, &criteria, &metadata);
        let actual = json!({
            "missing": problems.missing,
            "not_delegable": problems.not_delegable,
        });
        if actual != case["output"] {
            failures.push(format!("{id}: got {actual}, want {}", case["output"]));
        }
    }
    assert!(failures.is_empty(), "{}", report(&failures));
    Ok(())
}

fn brief_context(input: &Value) -> Result<BriefContext, Box<dyn Error>> {
    Ok(BriefContext {
        task_key: input["task_key"].as_str().unwrap_or_default().to_owned(),
        trial_id: input["trial_id"].as_str().unwrap_or_default().to_owned(),
        repository: input["repository"].as_str().unwrap_or_default().to_owned(),
        worktree: input["worktree"].as_str().unwrap_or_default().to_owned(),
        branch: input["branch"].as_str().unwrap_or_default().to_owned(),
        subject: input["subject"].as_str().unwrap_or_default().to_owned(),
        body: input["body"].as_str().unwrap_or_default().to_owned(),
        test_paths: strings(&input["test_paths"])?,
        oracle: argvs(&input["oracle"])?,
        gates: argvs(&input["gates"])?,
        acceptance: pairs(&input["acceptance"])?,
        files: strings(&input["files"])?,
        out_of_scope: strings(&input["out_of_scope"])?,
    })
}

#[test]
fn briefs_vectors() -> TestResult {
    let mut failures = Vec::new();
    for case in load("briefs.json")? {
        let id = case["id"].as_str().unwrap_or_default().to_owned();
        let input = &case["input"];
        if let Some(template) = input.get("template") {
            let template = template.as_str().unwrap_or_default();
            let context = brief_context(&input["context"])?;
            match render(template, &context) {
                Ok(text) => {
                    let want = &case["output"]["text"];
                    if *want != json!(text) {
                        failures.push(format!("{id}: text differs"));
                    }
                }
                Err(error) => {
                    let actual = json!({"error": "ValidationFailed", "message": error.message()});
                    if actual != case["output"] {
                        failures.push(format!("{id}: got {actual}, want {}", case["output"]));
                    }
                }
            }
        } else {
            let context = brief_context(input)?;
            for template in ["replay-v1", "leaf-v1"] {
                let want = case["output"][template].as_str().unwrap_or_default();
                match render(template, &context) {
                    Ok(text) if text == want => {}
                    _ => failures.push(format!("{id}: {template} differs")),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", report(&failures));
    Ok(())
}

fn escape_json(escapes: &[wg_experiments::escape::Escape]) -> Value {
    Value::Array(
        escapes
            .iter()
            .map(|e| {
                json!({
                    "tool": e.tool,
                    "tool_use_id": e.tool_use_id,
                    "root": e.root,
                    "excerpt": e.excerpt,
                })
            })
            .collect(),
    )
}

#[test]
fn escape_vectors() -> TestResult {
    let mut failures = Vec::new();
    for case in load("escape.json")? {
        let id = case["id"].as_str().unwrap_or_default().to_owned();
        let input = &case["input"];
        if let Some(text) = input.get("text").and_then(Value::as_str) {
            let normalized = normalize_path_text(text);
            let mut actual = Map::new();
            actual.insert("normalized".to_owned(), json!(normalized));
            if let Some(roots) = input.get("roots") {
                let calls = [ToolCall {
                    name: "Bash".to_owned(),
                    input: json!({"command": text}),
                    tool_use_id: "g1".to_owned(),
                }];
                let escapes = find_escapes(&calls, &strings(roots)?);
                actual.insert("escapes".to_owned(), escape_json(&escapes));
            }
            if Value::Object(actual.clone()) != case["output"] {
                failures.push(format!(
                    "{id}: got {}, want {}",
                    Value::Object(actual),
                    case["output"]
                ));
            }
        } else if let Some(calls_json) = input.get("calls") {
            let calls: Vec<ToolCall> = calls_json
                .as_array()
                .ok_or("calls is not an array")?
                .iter()
                .map(|call| ToolCall {
                    name: call["name"].as_str().unwrap_or_default().to_owned(),
                    input: call["input"].clone(),
                    tool_use_id: call["tool_use_id"].as_str().unwrap_or_default().to_owned(),
                })
                .collect();
            let escapes = find_escapes(&calls, &strings(&input["roots"])?);
            let actual = json!({"escapes": escape_json(&escapes)});
            if actual != case["output"] {
                failures.push(format!("{id}: got {actual}, want {}", case["output"]));
            }
        } else {
            failures.push(format!("{id}: unrecognised input"));
        }
    }
    assert!(failures.is_empty(), "{}", report(&failures));
    Ok(())
}

/// The config of a shape name in the golden file (`rust-only`, `python-only`, ...).
fn shape_config(shape: &str) -> ReplayConfig {
    let has_cargo = shape == "rust-only" || shape == "rust-and-python";
    let has_pyproject = shape == "python-only" || shape == "rust-and-python";
    ReplayConfig::for_manifests(has_cargo, has_pyproject)
}

fn str_list(items: &[String]) -> Value {
    json!(items)
}

fn tree_set(files: &[&str]) -> BTreeSet<String> {
    files.iter().map(|f| (*f).to_owned()).collect()
}

#[test]
fn replay_vectors() -> TestResult {
    let mut failures = Vec::new();
    let tree = tree_set(&TREE);
    for case in load("replay.json")? {
        let id = case["id"].as_str().unwrap_or_default().to_owned();
        let input = &case["input"];
        let actual: Option<Value> = if id == "match-glob-matrix" {
            let paths = strings(&input["paths"])?;
            let patterns = strings(&input["patterns"])?;
            let matches: Vec<Value> = paths
                .iter()
                .flat_map(|path| {
                    patterns.iter().map(move |pattern| {
                        json!({
                            "path": path,
                            "pattern": pattern,
                            "match": match_glob(path, pattern),
                        })
                    })
                })
                .collect();
            Some(json!({"matches": matches}))
        } else if let Some(shape) = id.strip_prefix("config-") {
            let config = shape_config(shape);
            Some(json!({
                "test_globs": str_list(&config.test_globs),
                "source_suffixes": str_list(&config.source_suffixes),
                "ignore_globs": str_list(&config.ignore_globs),
            }))
        } else if let Some(shape) = id.strip_prefix("kind-") {
            let config = shape_config(shape);
            let kinds: Map<String, Value> = strings(&input["paths"])?
                .iter()
                .map(|path| (path.clone(), json!(kind(path, &config).as_str())))
                .collect();
            Some(json!({"kinds": kinds}))
        } else if id == "band" {
            let bands: Vec<Value> = input["lines"]
                .as_array()
                .ok_or("lines is not an array")?
                .iter()
                .map(|lines| json!(band(lines.as_i64().unwrap_or_default())))
                .collect();
            Some(json!({"bands": bands}))
        } else if id.starts_with("expand-") {
            let argv = expand_oracle_template(
                &strings(&input["template"])?,
                &strings(&input["test_paths"])?,
                &strings(&input["packages"])?,
            );
            Some(json!({"argv": argv}))
        } else if id.starts_with("nearest-") {
            let dirs: BTreeSet<String> = strings(&input["dirs"])?.into_iter().collect();
            let path = input["path"].as_str().unwrap_or_default();
            Some(json!({"nearest": nearest(path, &dirs)}))
        } else if id == "manifest-dirs-tree" {
            let names = strings(&input["names"])?;
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let dirs: Vec<String> = manifest_dirs(&tree, &names).into_iter().collect();
            Some(json!({"dirs": dirs}))
        } else if id.starts_with("template-inputs-") {
            let (existing, packages) = template_inputs(&tree, &strings(&input["test_paths"])?);
            Some(json!({"existing": existing, "packages": packages}))
        } else if id.starts_with("commit-") {
            let shape = input["shape"].as_str().unwrap_or_default();
            let config = shape_config(shape);
            let numstat = input["numstat"].as_str().unwrap_or_default();
            let name_status = input["name_status"].as_str().unwrap_or_default();
            let patch = input["patch"].as_str().unwrap_or_default();
            let changes = parse_numstat(numstat);
            let split = split_changes(&changes, &config);
            let facts = features(&split, &tree);
            let added = added_lines(patch);
            let entries = parse_name_status(name_status);
            let kinds: Map<String, Value> = changes
                .iter()
                .map(|c| (c.path.clone(), json!(kind(&c.path, &config).as_str())))
                .collect();
            let change_json: Vec<Value> = changes
                .iter()
                .map(|c| {
                    json!({
                        "path": c.path,
                        "added": c.added,
                        "removed": c.removed,
                        "old_path": c.old_path,
                    })
                })
                .collect();
            Some(json!({
                "changes": change_json,
                "kinds": kinds,
                "src_lines": facts.src_lines,
                "test_lines": facts.test_lines,
                "src_files": facts.src_files,
                "test_files": facts.test_files,
                "band": facts.band,
                "packages": facts.packages,
                "has_inline_tests": has_inline_tests(&added, &split.sources),
                "tests_modified": tests_modified(&entries, &config),
                "changed_paths": changed_paths(&entries, &config),
            }))
        } else {
            None
        };
        match actual {
            Some(actual) if actual == case["output"] => {}
            Some(actual) => failures.push(format!("{id}: got {actual}, want {}", case["output"])),
            None => failures.push(format!("{id}: no case handler")),
        }
    }
    assert!(failures.is_empty(), "{}", report(&failures));
    Ok(())
}
