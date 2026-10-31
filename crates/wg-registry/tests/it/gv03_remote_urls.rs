//! GV-03 (URLs): every case of `golden/gitparse/remote_urls.json` must normalise to the recorded
//! value. Also the `test_normalize_remote_url` case of `tests/unit/test_foundations.py`.

use serde_json::Value;
use wg_registry::remote::normalize_remote_url;

fn load() -> Value {
    let path = format!(
        "{}/../../golden/gitparse/remote_urls.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

#[test]
fn remote_url_vectors_match() {
    let golden = load();
    let cases = golden["cases"].as_array().expect("cases array");
    assert!(!cases.is_empty(), "golden has no cases");
    for case in cases {
        let url = case["input"]["url"].as_str().expect("input url");
        let want = case["output"]["normalized"].as_str().expect("normalized");
        assert_eq!(normalize_remote_url(url), want, "{}: {url:?}", case["name"]);
    }
}

#[test]
fn equivalent_remote_forms_normalise_alike() {
    let variants = [
        "git@github.com:Org/Repo.git",
        "https://github.com/Org/Repo",
        "ssh://git@GitHub.com/Org/Repo.git",
        "https://user:pw@github.com/Org/Repo.git/",
    ];
    for variant in variants {
        assert_eq!(normalize_remote_url(variant), "github.com/Org/Repo");
    }
}
