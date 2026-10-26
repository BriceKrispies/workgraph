//! GV-14: every case of `golden/pydantic/` must give the recorded dump or the recorded error list.
//!
//! The request models come from the `contracts/openapi.json` component schemas and the MCP argument
//! models from the `input_schema` of `contracts/mcp.json`. The two request models that are not in
//! the OpenAPI document (`AttemptCreate`, `TaskFilter`) are built by hand, and the Python field
//! validators are attached by name.

#![allow(clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use wg_validate::{
    Check, ChoiceKind, Extra, Field, LoadOptions, Model, Schema, StrRules, model_from_json_schema,
    validate,
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(path: PathBuf) -> Value {
    let text = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn check_for(model: &str, field: &str) -> Option<Check> {
    match (model, field) {
        ("WorkspaceCreate", "slug") | ("ProjectCreate", "slug") | ("ProjectUpdate", "slug") => {
            Some(Check::Slug)
        }
        ("WorkspaceCreate", "key_prefix") => Some(Check::KeyPrefix),
        ("TaskCreate", "tags") | ("TaskUpdate", "tags" | "add_tags" | "remove_tags") => {
            Some(Check::Tags)
        }
        ("TaskCreate", "files") | ("TaskUpdate", "files" | "add_files") => Some(Check::Files),
        ("TaskFileIn", "anchor") => Some(Check::EmptyToNone),
        _ => None,
    }
}

fn with_checks_schema(schema: Schema) -> Schema {
    match schema {
        Schema::Model(model) => Schema::Model(Box::new(with_checks(*model))),
        Schema::Nullable(inner) => Schema::Nullable(Box::new(with_checks_schema(*inner))),
        Schema::List {
            item,
            min_length,
            max_length,
        } => Schema::List {
            item: Box::new(with_checks_schema(*item)),
            min_length,
            max_length,
        },
        other => other,
    }
}

fn with_checks(model: Model) -> Model {
    let name = model.name.clone();
    Model {
        fields: model
            .fields
            .into_iter()
            .map(|field| Field {
                check: check_for(&name, &field.name),
                schema: with_checks_schema(field.schema),
                ..field
            })
            .collect(),
        ..model
    }
}

fn text() -> Schema {
    Schema::Str(StrRules {
        strip: true,
        ..StrRules::default()
    })
}

fn maybe_text(name: &str) -> Field {
    Field::optional(name, Schema::Nullable(Box::new(text())), Value::Null)
}

fn enum_of(openapi: &Value, name: &str) -> Schema {
    let values = openapi["components"]["schemas"][name]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("{name} is not an enum"))
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    Schema::Choice {
        kind: ChoiceKind::Enum,
        values,
    }
}

fn maybe_list(name: &str, item: Schema) -> Field {
    Field::optional(
        name,
        Schema::Nullable(Box::new(Schema::List {
            item: Box::new(item),
            min_length: None,
            max_length: None,
        })),
        Value::Null,
    )
}

fn maybe_time(name: &str) -> Field {
    Field::optional(
        name,
        Schema::Nullable(Box::new(Schema::DateTime)),
        Value::Null,
    )
}

fn flag(name: &str) -> Field {
    Field::optional(name, Schema::Bool, json!(false))
}

fn attempt_create() -> Model {
    let mut provider = StrRules {
        strip: true,
        ..StrRules::default()
    };
    provider.max_length = Some(100);
    Model::new(
        "AttemptCreate",
        Extra::Forbid,
        vec![
            Field::required("task", text()),
            Field::optional("provider", Schema::Str(provider), json!("external")),
            maybe_text("adapter"),
            maybe_text("agent_id"),
            maybe_text("model"),
            maybe_text("session_id"),
            maybe_text("parent_attempt"),
            Field::optional(
                "status",
                Schema::Choice {
                    kind: ChoiceKind::Literal,
                    values: vec!["queued".to_string(), "running".to_string()],
                },
                json!("running"),
            ),
        ],
    )
}

fn task_filter(openapi: &Value) -> Model {
    let literal_sort = Schema::Choice {
        kind: ChoiceKind::Literal,
        values: [
            "created_at",
            "updated_at",
            "priority",
            "key",
            "title",
            "status",
            "due_at",
        ]
        .map(str::to_string)
        .to_vec(),
    };
    Model::new(
        "TaskFilter",
        Extra::Forbid,
        vec![
            maybe_list("statuses", enum_of(openapi, "TaskStatus")),
            maybe_list("priorities", enum_of(openapi, "TaskPriority")),
            maybe_text("project"),
            maybe_text("repository"),
            maybe_list("tags", text()),
            maybe_text("file"),
            maybe_text("category"),
            maybe_text("assignee"),
            maybe_text("parent"),
            flag("root_only"),
            maybe_text("text"),
            maybe_time("created_after"),
            maybe_time("created_before"),
            flag("include_archived"),
            flag("archived_only"),
            Field::optional("sort", literal_sort, json!("created_at")),
            Field::optional("descending", Schema::Bool, json!(true)),
            Field::optional(
                "limit",
                Schema::Int(wg_validate::IntRules {
                    ge: Some(1),
                    le: Some(500),
                    ..Default::default()
                }),
                json!(50),
            ),
            Field::optional(
                "offset",
                Schema::Int(wg_validate::IntRules {
                    ge: Some(0),
                    ..Default::default()
                }),
                json!(0),
            ),
        ],
    )
}

/// JSON equality with numbers compared by value, so the result does not depend on whether
/// `serde_json` runs with `arbitrary_precision` (the workspace build enables it through wg-json).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| same(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        _ => a == b,
    }
}

/// Runs every case of one golden file; returns the number of cases.
fn run_file(path: &Path, model: &Model) -> usize {
    let golden = read(path.to_path_buf());
    let cases = golden["cases"].as_array().expect("cases");
    for case in cases {
        let id = case["id"].as_str().unwrap_or("?");
        let result = validate(model, &case["input"]);
        let label = format!("{} / {id}", path.display());
        match (&case["output"]["ok"], &case["output"]["errors"]) {
            (ok, _) if !ok.is_null() => {
                let got = result.unwrap_or_else(|e| panic!("{label}: errors {e:?}"));
                assert!(same(&got, ok), "{label}: {got} != {ok}");
            }
            (_, Value::Array(expected)) => {
                let got = result.expect_err(&label);
                let got: Vec<Value> = got.iter().map(|e| e.to_json()).collect();
                assert!(
                    same(&Value::Array(got.clone()), &Value::Array(expected.clone())),
                    "{label}: {got:?} != {expected:?}"
                );
            }
            _ => panic!("{label}: neither ok nor errors"),
        }
    }
    cases.len()
}

fn files_in(dir: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(root().join("golden/pydantic").join(dir))
        .expect("golden dir")
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    found.sort();
    found
}

#[test]
fn request_models() {
    let openapi = read(root().join("contracts/openapi.json"));
    let options = LoadOptions {
        strip_whitespace: true,
    };
    let mut total = 0;
    for path in files_in("request") {
        let golden = read(path.clone());
        let name = golden["model"].as_str().expect("model name").to_string();
        let model = match name.as_str() {
            "AttemptCreate" => attempt_create(),
            "TaskFilter" => task_filter(&openapi),
            _ => {
                let schema = &openapi["components"]["schemas"][&name];
                model_from_json_schema(&openapi, schema, options)
                    .unwrap_or_else(|e| panic!("{name}: {e}"))
            }
        };
        total += run_file(&path, &with_checks(model));
    }
    assert!(total > 1000, "only {total} request cases ran");
}

#[test]
fn mcp_tools() {
    let mcp = read(root().join("contracts/mcp.json"));
    let tools = mcp["tools"].as_array().expect("tools");
    let mut total = 0;
    let mut files = 0;
    for path in files_in("mcp") {
        let golden = read(path.clone());
        let name = golden["tool"].as_str().expect("tool name");
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("tool {name} not in contracts/mcp.json"));
        let schema = &tool["input_schema"];
        let model = model_from_json_schema(schema, schema, LoadOptions::default())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        total += run_file(&path, &model);
        files += 1;
    }
    assert_eq!(files, tools.len(), "a tool has no golden file");
    assert!(total > 1000, "only {total} mcp cases ran");
}
