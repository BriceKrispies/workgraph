//! GV-16: every vector under `golden/codeparse/` must produce the recorded symbols, hunks and
//! change analysis, and the grammar pins must match the versions the Python analyser was built with.
#![allow(clippy::expect_used)]

use serde_json::{Value, json};
use wg_codeparse::{
    ANALYZER_VERSION, Language, Symbol, analyze_file, available_languages, extract_symbols,
    parse_hunks,
};

fn load(name: &str) -> Value {
    let path = format!(
        "{}/../../golden/codeparse/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).expect("golden file is readable");
    serde_json::from_str(&text).expect("golden file is JSON")
}

fn str_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("`{key}` is a string"))
}

fn opt_bytes(value: &Value, key: &str) -> Option<Vec<u8>> {
    value[key].as_str().map(|text| text.as_bytes().to_vec())
}

fn lines(value: &Value) -> Vec<usize> {
    value
        .as_array()
        .expect("line list")
        .iter()
        .map(|line| usize::try_from(line.as_u64().expect("line number")).expect("fits"))
        .collect()
}

fn symbols_value(symbols: &[Symbol]) -> Value {
    Value::Array(
        symbols
            .iter()
            .map(|symbol| {
                json!({
                    "kind": symbol.kind.as_str(),
                    "name": symbol.name,
                    "start_line": symbol.start_line,
                    "end_line": symbol.end_line,
                })
            })
            .collect(),
    )
}

/// Checks one side's recorded symbols. A `null` record means the file did not parse, or the
/// language is unsupported; a missing side (`None` source) records an empty list.
fn check_symbols(
    id: &str,
    side: &str,
    source: Option<&[u8]>,
    language: Option<Language>,
    golden: &Value,
) {
    let Some(language) = language else {
        assert!(
            golden.is_null(),
            "{id} {side}: unsupported language records null"
        );
        return;
    };
    match source {
        None => assert_eq!(golden, &json!([]), "{id} {side}: absent source records []"),
        Some(bytes) => {
            let actual = extract_symbols(bytes, language)
                .expect("grammar loads")
                .map_or(Value::Null, |symbols| symbols_value(&symbols));
            assert_eq!(&actual, golden, "{id} {side}: symbols");
        }
    }
}

fn sort_key(change: &Value) -> (String, String, String) {
    (
        str_field(change, "kind").to_owned(),
        str_field(change, "name").to_owned(),
        str_field(change, "change").to_owned(),
    )
}

#[test]
fn extract_vectors_match_recorded_symbols() {
    let golden = load("extract.json");
    for case in golden["cases"].as_array().expect("cases") {
        let id = str_field(case, "id");
        let language =
            Language::from_name(str_field(case, "language")).expect("supported language");
        let symbols = extract_symbols(str_field(case, "source").as_bytes(), language)
            .expect("grammar loads")
            .expect("source parses");
        assert_eq!(symbols_value(&symbols), case["symbols"], "{id}");
    }
}

#[test]
fn change_vectors_match_recorded_analysis() {
    let files = [
        "python.json",
        "javascript.json",
        "typescript.json",
        "tsx.json",
        "go.json",
        "rust.json",
        "c_sharp.json",
        "fallback.json",
    ];
    let mut checked = 0;
    for file in files {
        let golden = load(file);
        assert_eq!(golden["analyzer_version"], ANALYZER_VERSION, "{file}");
        for case in golden["cases"].as_array().expect("cases") {
            let id = str_field(case, "id");
            let diff = str_field(case, "diff");
            let (deleted, added) = parse_hunks(diff);
            assert_eq!(
                deleted,
                lines(&case["deleted_lines"]),
                "{id}: deleted lines of diff"
            );
            assert_eq!(
                added,
                lines(&case["added_lines"]),
                "{id}: added lines of diff"
            );

            let language_name = case["language"].as_str();
            let language = language_name.and_then(Language::from_name);
            let old = opt_bytes(case, "old");
            let new = opt_bytes(case, "new");
            let deleted_lines = lines(&case["deleted_lines"]);
            let added_lines = lines(&case["added_lines"]);
            let actual = analyze_file(
                str_field(case, "path"),
                language_name,
                old.as_deref(),
                new.as_deref(),
                &deleted_lines,
                &added_lines,
                extract_symbols,
            );

            let expected = &case["expected"];
            assert_eq!(
                actual.mode.as_str(),
                str_field(expected, "mode"),
                "{id}: mode"
            );
            assert_eq!(
                actual.reason.as_deref(),
                expected["reason"].as_str(),
                "{id}: reason"
            );
            assert_eq!(
                actual.functions(),
                expected["functions"].as_u64().unwrap_or(0) as usize,
                "{id}: functions"
            );
            assert_eq!(
                actual.types(),
                expected["types"].as_u64().unwrap_or(0) as usize,
                "{id}: types"
            );
            assert_eq!(
                actual.unattributed_lines,
                expected["unattributed_lines"].as_u64().unwrap_or(0) as usize,
                "{id}: unattributed lines"
            );

            let mut changes: Vec<Value> = actual
                .changes
                .iter()
                .map(|change| {
                    json!({
                        "kind": change.kind.as_str(),
                        "name": change.name,
                        "change": change.change.as_str(),
                        "start_line": change.start_line,
                        "end_line": change.end_line,
                        "lines_changed": change.lines_changed,
                    })
                })
                .collect();
            changes.sort_by_key(sort_key);
            let mut recorded: Vec<Value> = expected["changes"].as_array().expect("changes").clone();
            recorded.sort_by_key(sort_key);
            assert_eq!(changes, recorded, "{id}: changes");

            check_symbols(id, "old", old.as_deref(), language, &case["symbols_old"]);
            check_symbols(id, "new", new.as_deref(), language, &case["symbols_new"]);
            checked += 1;
        }
    }
    assert!(checked > 0, "golden cases were read");
}

#[test]
fn grammar_catalogue_and_pins_match_golden() {
    let golden = load("grammars.json");
    assert_eq!(golden["analyzer_version"], ANALYZER_VERSION);

    let mut recorded: Vec<&str> = golden["available_languages"]
        .as_array()
        .expect("languages")
        .iter()
        .map(|name| name.as_str().expect("name"))
        .collect();
    recorded.sort_unstable();
    let mut loaded = available_languages();
    loaded.sort_unstable();
    assert_eq!(loaded, recorded);

    let cargo_toml = include_str!("../Cargo.toml");
    for (crate_name, version) in golden["grammars"].as_object().expect("grammars") {
        let version = version.as_str().expect("version");
        let pin = format!("{crate_name} = \"={version}\"");
        assert!(cargo_toml.contains(&pin), "Cargo.toml pins `{pin}`");
    }
}
