//! LIB-17 (settings part): the settings document, its `paths` object and the patch document. The
//! DTOs moved with the registry feature; the health and meta documents stay in `wg-wire`.

#![allow(clippy::expect_used)]

use wg_json::{PyStr, Value};
use wg_registry_feature::spec::dto::settings;

/// A dict key as text (keys are stored as code points).
fn key_text(key: &PyStr) -> String {
    key.code_points()
        .iter()
        .map(|cp| char::from_u32(*cp).expect("code point"))
        .collect()
}

fn names(value: &Value) -> Vec<String> {
    match value {
        Value::Model { fields, .. } => fields.iter().map(|(k, _)| k.clone()).collect(),
        Value::Dict(fields) => fields.iter().map(|(k, _)| key_text(k)).collect(),
        other => panic!("neither a model nor a dict: {other:?}"),
    }
}

/// The `paths` object of a settings document.
fn get_paths(document: &Value) -> Value {
    match document {
        Value::Dict(fields) => fields
            .iter()
            .find(|(key, _)| key_text(key) == "paths")
            .map(|(_, value)| value.clone())
            .expect("paths"),
        other => panic!("not a dict: {other:?}"),
    }
}

#[test]
fn settings_documents() {
    let config = Value::Dict(Vec::new());
    let get = settings::settings(config.clone(), "cfg.toml", "data");
    assert_eq!(names(&get), settings::FIELDS);
    assert_eq!(names(&get_paths(&get)), settings::PATH_FIELDS);
    assert_eq!(names(&settings::patched(config)), settings::PATCH_FIELDS);
}
