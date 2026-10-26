//! The `classify` and `search` commands (IF-14): the clap trees against the snapshot's parameters,
//! the lexical hit shaping and the `SearchResult` shape the command prints.

use wg_intel_feature::spec::args::{classify_command, search_command};
use wg_intel_feature::spec::search_out::{
    Candidate, TaskHead, lexical_hits, outcome, round4, unavailable_note,
};
use wg_json::{PyStr, Value};
use wg_types::Id;

fn text(value: &str) -> Value {
    Value::Str(PyStr::from(value))
}

fn id(n: u128) -> Id {
    Id::from_u128(n)
}

fn head(key: &str, category: Option<&str>) -> TaskHead {
    TaskHead {
        key: key.to_owned(),
        title: format!("Task {key}"),
        status: "ready".to_owned(),
        category: category.map(str::to_owned),
    }
}

fn candidate(id: Id, rank: f64, in_scope: bool, key: &str) -> Candidate {
    Candidate {
        task_id: id,
        rank,
        snippet: format!("[{key}] snippet"),
        in_scope,
        task: Some(head(key, Some("feature"))),
    }
}

fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Model { fields, .. } => fields
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, found)| found),
        _ => None,
    }
}

fn subcommands(command: &clap::Command) -> Vec<String> {
    command
        .get_subcommands()
        .map(|sub| sub.get_name().to_owned())
        .collect()
}

// covers: C133
#[test]
fn search_tree_has_the_snapshot_options() {
    let command = search_command();
    let names: Vec<&str> = command
        .get_arguments()
        .filter_map(|arg| arg.get_long())
        .collect();
    for option in [
        "mode",
        "project",
        "repo",
        "category",
        "status",
        "agent",
        "model",
        "path",
        "limit",
        "similar-to",
        "reindex",
        "workspace",
    ] {
        assert!(names.contains(&option), "missing --{option}");
    }
    let mut query = command.get_positionals();
    assert_eq!(query.next().map(|arg| arg.get_id().as_str()), Some("query"));
}

// covers: C038, C039, C040, C041, C042, C043, C044, C045, C046
#[test]
fn classify_tree_has_the_nine_commands() {
    let mut names = subcommands(&classify_command());
    names.sort();
    assert_eq!(
        names,
        [
            "confirm",
            "disagreements",
            "evaluate",
            "evaluations",
            "gold",
            "promote",
            "reclassify",
            "request",
            "show",
        ]
    );
}

// covers: C133
#[test]
fn lexical_hits_keep_scope_and_number_the_ranks() {
    let (a, b, c, d) = (id(1), id(2), id(3), id(4));
    let mut gone = candidate(d, -1.0, true, "WG-4");
    gone.task = None;
    let candidates = [
        candidate(a, -2.123_456, true, "WG-1"),
        candidate(b, -9.0, false, "WG-2"),
        gone,
        candidate(c, -0.5, true, "WG-3"),
    ];
    let hits = lexical_hits(&candidates, 10);
    let keys: Vec<&str> = hits.iter().map(|hit| hit.head.key.as_str()).collect();
    assert_eq!(keys, ["WG-1", "WG-3"]);
    assert_eq!(hits[0].lexical_rank, 1);
    assert_eq!(hits[1].lexical_rank, 2);
    assert_eq!(hits[0].score, 2.1235);
    assert_eq!(hits[0].snippet, "[WG-1] snippet");
    assert_eq!(lexical_hits(&candidates, 1).len(), 1);
}

#[test]
fn round4_matches_python_round() {
    assert_eq!(round4(-2.0), -2.0);
    assert_eq!(round4(1.0 / 3.0), 0.3333);
}

// covers: C133
#[test]
fn unavailable_note_names_the_model_and_the_reindex_command() {
    assert_eq!(
        unavailable_note("ollama", "nomic-embed-text"),
        "semantic search unavailable: no embeddings stored for ollama/nomic-embed-text; run `workgraph search reindex`; lexical results only"
    );
}

// covers: C133
#[test]
fn hybrid_without_embeddings_is_lexical_with_the_note() {
    let candidates = [candidate(id(7), -3.0, true, "WG-7")];
    let note = unavailable_note("ollama", "nomic-embed-text");
    let out = outcome("login", "hybrid", &candidates, 5, Some(&note));
    assert_eq!(field(&out, "mode"), Some(&text("lexical")));
    assert_eq!(field(&out, "semantic_available"), Some(&Value::Bool(false)));
    assert_eq!(field(&out, "embedding_model"), Some(&Value::Null));
    assert_eq!(field(&out, "note"), Some(&text(&note)));
    let Some(Value::List(hits)) = field(&out, "hits") else {
        panic!("hits is a list");
    };
    assert_eq!(hits.len(), 1);
    assert_eq!(field(&hits[0], "score"), Some(&Value::Float(3.0)));
}

// covers: C133
#[test]
fn semantic_without_embeddings_returns_no_hits() {
    let out = outcome(
        "login",
        "semantic",
        &[],
        5,
        Some(&unavailable_note("ollama", "nomic-embed-text")),
    );
    assert_eq!(field(&out, "mode"), Some(&text("semantic")));
    assert_eq!(field(&out, "hits"), Some(&Value::List(Vec::new())));
}

// covers: C133
#[test]
fn lexical_search_caps_hits_at_the_limit() {
    let candidates: Vec<Candidate> = (1_u128..=5)
        .map(|n| candidate(id(n), -(n as f64), true, &format!("WG-{n}")))
        .collect();
    let out = outcome("task", "lexical", &candidates, 2, None);
    let Some(Value::List(hits)) = field(&out, "hits") else {
        panic!("hits is a list");
    };
    assert_eq!(hits.len(), 2);
    assert_eq!(field(&out, "note"), Some(&Value::Null));
}
