//! Golden vectors for the configuration model (GV-11).
//!
//! `golden/config/*.json` is produced by `tools/golden/gen_config.py` from the Python `ConfigStore`.
//! Each test here drives the Rust model the way the generator drives the Python one, and compares
//! every observed outcome with the recorded one. Keys are sorted in the golden files, so
//! comparison is by JSON value, not by text, except where a TOML text is pinned (the writer).

use std::error::Error;
use std::path::PathBuf;

use serde_json::{Map, Value, json};
use wg_config::{
    ConfigError, ConfigFiles, default_global, default_overrides, parse_document, parse_value,
    render, set_path, strip_none, validate_global, validate_overrides,
};

type TestResult = Result<(), Box<dyn Error>>;

const SLUG: &str = "team";

fn golden(name: &str) -> Result<Value, Box<dyn Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/config")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&text)?)
}

/// `config_error_json` of the generator: the error shape, without the Python input.
fn config_error_json(error: &ConfigError) -> Value {
    match error {
        ConfigError::InvalidToml(_) => {
            json!({"outcome": "error", "kind": "invalid_toml", "errors": []})
        }
        ConfigError::Validation(errors) => json!({
            "outcome": "error",
            "kind": "validation",
            "errors": errors
                .iter()
                .map(|error| json!({
                    "type": error.kind,
                    "loc": error.loc.iter().map(wg_validate::Loc::to_json).collect::<Vec<_>>(),
                    "msg": error.msg,
                }))
                .collect::<Vec<_>>(),
        }),
        ConfigError::Exception(name) => json!({"outcome": "exception", "type": name}),
    }
}

fn with_problems(mut value: Value, problems: usize) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.insert("problems".to_string(), json!(problems));
    }
    value
}

/// The value at a dotted key of a dump (`_get`).
fn value_at(document: &Map<String, Value>, dotted: &str) -> Value {
    let mut cursor = Value::Object(document.clone());
    for part in dotted.split('.') {
        cursor = cursor.get(part).cloned().unwrap_or(Value::Null);
    }
    cursor
}

/// The store the generator builds from a `files` map (`config.toml` and `workspaces/<slug>.toml`).
fn store_from(files: &Value) -> ConfigFiles {
    let mut store = ConfigFiles::default();
    if let Some(entries) = files.as_object() {
        for (path, text) in entries {
            let text = text.as_str().unwrap_or_default().to_string();
            if path == "config.toml" {
                store.global = Some(text);
            } else if let Some(slug) = path
                .strip_prefix("workspaces/")
                .and_then(|rest| rest.strip_suffix(".toml"))
            {
                store.workspaces.insert(slug.to_string(), text);
            }
        }
    }
    store
}

/// The `files` map of a store after an action (`_files_after`).
fn files_of(store: &ConfigFiles) -> Value {
    let mut files = Map::new();
    if let Some(text) = &store.global {
        files.insert("config.toml".to_string(), json!(text));
    }
    for (slug, text) in &store.workspaces {
        files.insert(format!("workspaces/{slug}.toml"), json!(text));
    }
    Value::Object(files)
}

/// The outcome of `_load_action`: validate every file, then the global configuration.
fn load_outcome(store: &ConfigFiles) -> Value {
    match store.validate_files() {
        Err(error) => config_error_json(&error),
        Ok(problems) => match store.load_global() {
            Ok(cfg) => {
                json!({"outcome": "ok", "effective": Value::Object(cfg), "problems": problems.len()})
            }
            Err(error) => with_problems(config_error_json(&error), problems.len()),
        },
    }
}

/// The outcome of `_key_result`: as `_load_action`, but reporting only the value at the key.
fn key_outcome(store: &ConfigFiles, key: &str) -> Value {
    match store.validate_files() {
        Err(error) => config_error_json(&error),
        Ok(problems) => match store.load_global() {
            Ok(cfg) => {
                json!({"outcome": "ok", "value_at_key": value_at(&cfg, key), "problems": problems.len()})
            }
            Err(error) => with_problems(config_error_json(&error), problems.len()),
        },
    }
}

/// The outcome of `_merge_action`: validate every file, then the configuration for the workspace.
fn merge_outcome(store: &ConfigFiles) -> Value {
    match store.validate_files() {
        Err(error) => config_error_json(&error),
        Ok(problems) => match store.effective(Some(SLUG)) {
            Ok(effective) => {
                json!({"outcome": "ok", "effective": Value::Object(effective), "problems": problems.len()})
            }
            Err(error) => with_problems(config_error_json(&error), problems.len()),
        },
    }
}

/// `golden` minus the keys that name the input rather than the observed outcome.
fn observed(golden_case: &Value, skip: &[&str]) -> Value {
    let mut object = golden_case.as_object().cloned().unwrap_or_default();
    for key in skip {
        object.remove(*key);
    }
    Value::Object(object)
}

/// Compares each observed outcome with its golden case, collecting mismatches with their names.
fn check(mismatches: &mut Vec<String>, name: &str, actual: &Value, expected: &Value) {
    let (actual, expected) = (numeric(actual), numeric(expected));
    if actual != expected {
        mismatches.push(format!(
            "{name}:\n  expected {expected}\n  actual   {actual}"
        ));
    }
}

/// A JSON value with its numbers in one canonical form, so that equality is numeric and does not
/// depend on how `serde_json` stores a number. With `arbitrary_precision` (enabled by `wg-json` when
/// the workspace is built together) a number keeps its source text, so `1e-07` and `1e-7` differ;
/// Python writes `1e-07`, and both must compare equal. Integers stay integers (`120` is not
/// `120.0`), floats compare by value, and `-0.0` is folded to `0.0`.
fn numeric(value: &Value) -> Value {
    match value {
        Value::Number(number) => {
            if let Some(int) = number.as_i64() {
                Value::from(int)
            } else if let Some(unsigned) = number.as_u64() {
                Value::from(unsigned)
            } else {
                let float = number.as_f64().unwrap_or(f64::NAN);
                Value::from(if float == 0.0 { 0.0 } else { float })
            }
        }
        Value::Array(items) => Value::Array(items.iter().map(numeric).collect()),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, value)| (key.clone(), numeric(value)))
                .collect(),
        ),
        other => other.clone(),
    }
}

fn report(mismatches: &[String], total: usize) {
    assert!(
        mismatches.is_empty(),
        "{} of {total} cases differ:\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The writer's input for a table key path: the default document with `key` replaced.
fn document_with(key: &str, input: &Value) -> Result<Map<String, Value>, Box<dyn Error>> {
    let base = strip_none(&default_global());
    set_path(&base, key, input.clone()).ok_or_else(|| format!("{key} is not nested").into())
}

#[test]
fn defaults_match_golden() -> TestResult {
    let golden = golden("defaults")?;
    let mut mismatches = Vec::new();
    check(
        &mut mismatches,
        "model defaults",
        &Value::Object(default_global()),
        &golden["model_default"],
    );
    assert!(mismatches.is_empty(), "{mismatches:?}");
    assert_eq!(
        render(&strip_none(&default_global())),
        golden["saved_toml"].as_str().unwrap_or_default(),
        "saved global default"
    );
    assert_eq!(
        render(&strip_none(&default_overrides())),
        golden["workspace_default_saved_toml"]
            .as_str()
            .unwrap_or_default(),
        "saved workspace default"
    );
    Ok(())
}

#[test]
fn scalar_keys_match_golden() -> TestResult {
    let keys = golden("keys")?;
    let rows = keys.as_array().ok_or("keys.json is not an array")?;
    let mut mismatches = Vec::new();
    let mut total = 0;
    for row in rows.iter().filter(|row| row.get("wildcard").is_none()) {
        let key = row["key"].as_str().unwrap_or_default();
        for case in row["cases"].as_array().into_iter().flatten() {
            total += 1;
            let name = format!("{key} {}", case["label"]);
            let document = document_with(key, &case["input"])?;
            let text = render(&document);
            check(
                &mut mismatches,
                &format!("{name} document"),
                &json!(text),
                &case["document_toml"],
            );
            let store = ConfigFiles {
                global: Some(
                    case["document_toml"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                ),
                ..ConfigFiles::default()
            };
            let actual = key_outcome(&store, key);
            check(
                &mut mismatches,
                &name,
                &actual,
                &observed(case, &["label", "input", "document_toml"]),
            );
        }
    }
    report(&mismatches, total);
    Ok(())
}

/// The error list with its order ignored. The golden inputs of table rows were stored with sorted
/// keys, so the order of errors across table entries cannot be recovered from them; the set of
/// errors is still compared exactly.
fn unordered_errors(mut outcome: Value) -> Value {
    if let Some(errors) = outcome.get_mut("errors").and_then(Value::as_array_mut) {
        // Keys are sorted explicitly: the golden objects and ours list their keys in other orders.
        let canonical =
            |error: &Value| format!("{}|{}|{}", error["loc"], error["type"], error["msg"]);
        errors.sort_by_key(canonical);
    }
    outcome
}

#[test]
fn table_keys_match_golden() -> TestResult {
    let keys = golden("keys")?;
    let rows = keys.as_array().ok_or("keys.json is not an array")?;
    let mut mismatches = Vec::new();
    let mut total = 0;
    for row in rows.iter().filter(|row| row.get("wildcard").is_some()) {
        let key = row["key"].as_str().unwrap_or_default();
        for case in row["cases"].as_array().into_iter().flatten() {
            total += 1;
            let name = format!("{key} {}", case["label"]);
            let document = document_with(key, &case["input"])?;
            let text = render(&document);
            // The golden text follows the insertion order of the Python input, which the sorted
            // golden input no longer carries, so the document is compared as a value.
            let ours = Value::Object(parse_document(&text)?);
            let theirs = Value::Object(parse_document(
                case["document_toml"].as_str().unwrap_or_default(),
            )?);
            check(&mut mismatches, &format!("{name} document"), &ours, &theirs);
            let store = ConfigFiles {
                global: Some(text),
                ..ConfigFiles::default()
            };
            check(
                &mut mismatches,
                &name,
                &unordered_errors(load_outcome(&store)),
                &unordered_errors(observed(case, &["label", "input", "document_toml"])),
            );
        }
    }
    report(&mismatches, total);
    Ok(())
}

#[test]
fn key_table_covers_contract_section_13_5() -> TestResult {
    // Section 13.5 has 41 rows: 38 leaf keys and 3 named-table families.
    let keys = golden("keys")?;
    let rows = keys.as_array().ok_or("keys.json is not an array")?;
    assert_eq!(
        rows.len(),
        41,
        "one golden row per row of contract section 13.5"
    );
    Ok(())
}

#[test]
fn load_documents_match_golden() -> TestResult {
    let loads = golden("load")?;
    let cases = loads.as_array().ok_or("load.json is not an array")?;
    let mut mismatches = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap_or_default();
        let store = ConfigFiles {
            global: Some(case["config_toml"].as_str().unwrap_or_default().to_string()),
            ..ConfigFiles::default()
        };
        check(
            &mut mismatches,
            name,
            &load_outcome(&store),
            &observed(case, &["name", "config_toml"]),
        );
    }
    report(&mismatches, cases.len());
    Ok(())
}

#[test]
fn migrate_matches_golden() -> TestResult {
    let migrates = golden("migrate")?;
    let cases = migrates.as_array().ok_or("migrate.json is not an array")?;
    let mut mismatches = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap_or_default();
        let store = store_from(&case["files_before"]);
        let actual = match store.migrate_files() {
            Ok((migrated, rewritten)) => json!({
                "outcome": "ok",
                "rewritten": rewritten,
                "files_after": files_of(&migrated),
            }),
            Err(error) => config_error_json(&error),
        };
        check(
            &mut mismatches,
            name,
            &actual,
            &observed(case, &["name", "files_before"]),
        );
    }
    report(&mismatches, cases.len());
    Ok(())
}

#[test]
fn merge_matches_golden() -> TestResult {
    let merges = golden("merge")?;
    let cases = merges.as_array().ok_or("merge.json is not an array")?;
    let mut mismatches = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap_or_default();
        let store = store_from(&case["files"]);
        check(
            &mut mismatches,
            name,
            &merge_outcome(&store),
            &observed(case, &["name", "files", "workspace"]),
        );
    }
    report(&mismatches, cases.len());
    Ok(())
}

#[test]
fn set_matches_golden() -> TestResult {
    let sets = golden("set")?;
    let cases = sets.as_array().ok_or("set.json is not an array")?;
    let mut mismatches = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap_or_default();
        let key = case["key"].as_str().unwrap_or_default();
        let parsed = parse_value(case["raw"].as_str().unwrap_or_default());
        let skip = ["name", "scope", "key", "raw"];
        let actual = if case["scope"] == "global" {
            global_set(key, parsed)
        } else {
            workspace_set(key, parsed)
        };
        check(&mut mismatches, name, &actual, &observed(case, &skip));
    }
    report(&mismatches, cases.len());
    Ok(())
}

/// `_set_global`: the CLI's `config set` against the defaults, then the written global file.
fn global_set(key: &str, parsed: Value) -> Value {
    let base = default_global();
    let Some(target) = set_path(&base, key, parsed.clone()) else {
        return json!({"exit": 2, "outcome": "not_nested", "parsed": parsed});
    };
    match validate_global(&Value::Object(target)) {
        Err(errors) => json!({
            "exit": 7,
            "outcome": "invalid",
            "parsed": parsed,
            "errors": config_error_json(&ConfigError::Validation(errors))["errors"],
        }),
        Ok(cfg) => json!({
            "exit": 0,
            "outcome": "written",
            "parsed": parsed,
            "value_at_key": value_at(&cfg, key),
            "toml": render(&strip_none(&cfg)),
        }),
    }
}

/// `_set_workspace`: the workspace overrides, saved, then the effective configuration reloaded.
fn workspace_set(key: &str, parsed: Value) -> Value {
    let base = strip_none(&default_overrides());
    let Some(target) = set_path(&base, key, parsed.clone()) else {
        return json!({"exit": 2, "outcome": "not_nested", "parsed": parsed});
    };
    let overrides = match validate_overrides(&Value::Object(target)) {
        Ok(overrides) => overrides,
        Err(errors) => {
            return json!({
                "exit": 7,
                "outcome": "invalid",
                "parsed": parsed,
                "errors": config_error_json(&ConfigError::Validation(errors))["errors"],
            });
        }
    };
    let text = render(&strip_none(&overrides));
    let store = ConfigFiles {
        workspaces: [(SLUG.to_string(), text.clone())].into_iter().collect(),
        ..ConfigFiles::default()
    };
    match store.effective(Some(SLUG)) {
        Err(error) => {
            let mut outcome = config_error_json(&error);
            outcome["exit"] = json!(7);
            outcome["parsed"] = parsed;
            outcome
        }
        Ok(effective) => json!({
            "exit": 0,
            "outcome": "written",
            "parsed": parsed,
            "value_at_key": value_at(&effective, key),
            "workspace_toml": text,
        }),
    }
}

#[test]
fn writer_matches_golden() -> TestResult {
    let writer = golden("writer")?;
    let cases = writer.as_array().ok_or("writer.json is not an array")?;
    let mut mismatches = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap_or_default();
        let text = case["toml"].as_str().unwrap_or_default();
        // The golden value is stored with sorted keys, so the TOML text is checked by reading it
        // back: parsing the pinned text must give the golden value, and writing that parse back
        // must give the pinned text byte for byte.
        let parsed = parse_document(text)?;
        check(
            &mut mismatches,
            &format!("{name} value"),
            &Value::Object(parsed.clone()),
            &case["value"],
        );
        check(
            &mut mismatches,
            &format!("{name} text"),
            &json!(render(&parsed)),
            &json!(text),
        );
    }
    report(&mismatches, cases.len());
    Ok(())
}

#[test]
fn validators_accept_defaults() -> TestResult {
    // The defaults validate against their own models, the way a fresh install reads them back.
    let global = validate_global(&Value::Object(default_global()));
    assert!(global.is_ok(), "{global:?}");
    let overrides = validate_overrides(&Value::Object(default_overrides()));
    assert!(overrides.is_ok(), "{overrides:?}");
    Ok(())
}
