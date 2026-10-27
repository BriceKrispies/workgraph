//! The applicable-checks reply of `GET /ws/{ws}/tasks/{ref}/checks` (H058, IF-45): each check is its
//! configuration's name and scope type followed by the check's own fields, as the Python model dumps
//! them.

use serde_json::{Map, Value as Json, json};
use wg_types::parse_id;
use wg_verify::{ApplicableCheck, CheckSpec};
use wg_verify_feature::spec::routes::check_document;

const CONFIG: &str = "018f0000-0000-7000-8000-000000000011";
const SCOPE: &str = "018f0000-0000-7000-8000-000000000012";

fn applicable(name: &str) -> ApplicableCheck {
    let raw: Map<String, Json> = json!({"name": name, "command": ["cargo", "fmt", "--check"]})
        .as_object()
        .cloned()
        .unwrap_or_default();
    let spec = CheckSpec::from_json(&raw).unwrap_or_else(|err| panic!("a valid check: {err:?}"));
    ApplicableCheck {
        config_id: parse_id(CONFIG).unwrap_or_else(|err| panic!("a uuid: {err:?}")),
        config_name: "unit".to_owned(),
        scope_type: "workspace".to_owned(),
        scope_id: parse_id(SCOPE).unwrap_or_else(|err| panic!("a uuid: {err:?}")),
        spec,
    }
}

// covers: H058
#[test]
fn a_check_document_leads_with_its_config_and_scope_then_the_check_fields() {
    let value = serde_json::to_value(check_document(&applicable("fmt"))).expect("a JSON value");
    let keys: Vec<&str> = value
        .as_object()
        .map(|object| object.keys().map(String::as_str).take(3).collect())
        .unwrap_or_default();
    assert_eq!(keys, ["config", "scope_type", "name"]);
    assert_eq!(value["config"], "unit");
    assert_eq!(value["scope_type"], "workspace");
    assert_eq!(value["name"], "fmt");
    assert_eq!(value["command"], json!(["cargo", "fmt", "--check"]));
}
