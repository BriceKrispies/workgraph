//! IF-35: the experiment tools answer through the feature's tool table over a real home, and the
//! trial tools that have no service yet stay unanswered.
//!
//! The home is opened as the server opens it, one workspace is written through the store, then the
//! tools are called by name with the arguments the catalog would validate.

use std::collections::BTreeSet;
use std::error::Error;
use std::path::PathBuf;

use serde_json::{Map, Value, json};
use wg_experiments_feature::mcp;
use wg_mcpspec::agent_permissions;
use wg_mcpspec::options::Agent;
use wg_model::entity::Workspace;
use wg_model::{Write, derive_id};
use wg_runtime::App;
use wg_types::{ErrorCode, Id, Permission, Timestamp, WgError};

type TestResult = Result<(), Box<dyn Error>>;

/// Unwraps a fixture value that is valid by construction.
fn ok<T>(result: Result<T, WgError>) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("invalid fixture: {err}"),
    }
}

fn entropy(seed: u8) -> [u8; 32] {
    core::array::from_fn(|i| seed.wrapping_add(i as u8))
}

fn id(n: u32) -> Id {
    derive_id(&entropy(35), n)
}

fn at(second: u8) -> Timestamp {
    ok(Timestamp::from_components(2026, 10, 26, 9, 0, second, 0))
}

fn workspace() -> Workspace {
    Workspace {
        id: id(0),
        slug: "default".into(),
        name: "Default".into(),
        description: String::new(),
        key_prefix: "WG".into(),
        next_task_number: 1,
        settings: Map::new(),
        archived_at: None,
        created_at: at(0),
        updated_at: at(0),
    }
}

/// A fresh home with one workspace, the default one.
fn seeded_app(name: &str) -> Result<(App, PathBuf), Box<dyn Error>> {
    let home =
        std::env::temp_dir().join(format!("wg-experiments-if35-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let mut app = App::open(Some(home.as_path()))?;
    app.runtime
        .store
        .write(|session| session.apply_write(&Write::insert(workspace())))?;
    Ok((app, home))
}

fn agent() -> Result<Agent, WgError> {
    let permissions: BTreeSet<Permission> = agent_permissions::<&str>(&[])?;
    Ok(Agent {
        name: "recorder".into(),
        permissions,
    })
}

/// Calls `name` with `arguments`, which must be a tool this feature answers.
fn answer(app: &mut App, name: &str, arguments: &Value) -> Result<Value, WgError> {
    let agent = ok(agent());
    match mcp::call(app, &agent, name, arguments) {
        Some(result) => result,
        None => panic!("{name} is not answered by the experiments feature"),
    }
}

// covers: M17
#[test]
fn experiment_list_answers_a_dict_of_experiments() -> TestResult {
    let (mut app, home) = seeded_app("list")?;
    let listed = answer(&mut app, "experiment_list", &json!({}))?;
    assert_eq!(listed, json!({"experiments": []}));
    let _ = std::fs::remove_dir_all(&home);
    Ok(())
}

// covers: M22
#[test]
fn experiment_predict_names_every_missing_size_argument() -> TestResult {
    let (mut app, home) = seeded_app("predict")?;
    let failure = match answer(&mut app, "experiment_predict", &json!({"model": "m"})) {
        Ok(value) => panic!("expected a validation failure, got {value}"),
        Err(err) => err,
    };
    assert_eq!(failure.code, ErrorCode::ValidationFailed);
    assert_eq!(
        failure.message,
        "experiment_predict needs task or all of the features; missing repository, src_lines, src_files, packages"
    );
    let _ = std::fs::remove_dir_all(&home);
    Ok(())
}

#[test]
fn trial_tools_without_a_service_are_left_to_the_backend() -> TestResult {
    let (mut app, home) = seeded_app("trials")?;
    let agent = ok(agent());
    for name in ["trial_classify", "trial_start", "trial_grade", "task_list"] {
        assert!(
            mcp::call(&mut app, &agent, name, &json!({})).is_none(),
            "{name} should not be answered here"
        );
    }
    let _ = std::fs::remove_dir_all(&home);
    Ok(())
}
