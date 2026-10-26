//! The readers, query and views of `events list` and `events verify`.

use serde_json::json;
use wg_events_feature::spec;
use wg_render::Console;

// covers: C066
#[test]
fn list_defaults_to_newest_first_with_a_cap() {
    let m = spec::command()
        .try_get_matches_from(["events", "list", "--limit", "9000"])
        .expect("the snapshot's events command parses");
    let (_, list) = m.subcommand().expect("the list subcommand");
    let args = spec::list_args(list).expect("the options read");
    assert_eq!(args.limit, 9000);
    let query = spec::list_query(&args, None).expect("a query without an entity");
    assert_eq!(query.limit, spec::HISTORY_LIMIT_CAP);
    assert!(query.descending);
    assert_eq!(query.after_seq, None);
}

// covers: C066
#[test]
fn list_with_an_entity_that_is_not_an_id_is_empty() {
    let m = spec::command()
        .try_get_matches_from(["events", "list", "--entity", "not-an-id"])
        .expect("the snapshot's events command parses");
    let (_, list) = m.subcommand().expect("the list subcommand");
    let args = spec::list_args(list).expect("the options read");
    assert_eq!(spec::list_query(&args, None), None);
}

// covers: C066
#[test]
fn list_after_a_sequence_reads_oldest_first() {
    let m = spec::command()
        .try_get_matches_from(["events", "list", "--after", "7"])
        .expect("the snapshot's events command parses");
    let (_, list) = m.subcommand().expect("the list subcommand");
    let args = spec::list_args(list).expect("the options read");
    let query = spec::list_query(&args, None).expect("a query");
    assert_eq!(query.after_seq, Some(7));
    assert!(!query.descending);
}

// covers: C067
#[test]
fn verify_view_shows_mismatches_as_python_repr() {
    // Wide enough that the line does not wrap (Console wraps at its width, as rich does).
    let console = Console::new(200);
    let report = json!({
        "tasks": 2,
        "events": 5,
        "consistent": false,
        "mismatches": {"WG-1": {"status": {"table": "done", "events": "open"}}}
    });
    let text = spec::verify_view(&console, &spec::verify_document(&report));
    assert!(text.starts_with(
        "MISMATCH: 2 tasks, 5 events {'WG-1': {'status': {'table': 'done', 'events': 'open'}}}"
    ));
}

// covers: C067
#[test]
fn verify_view_reports_a_consistent_log() {
    let console = Console::new(80);
    let report = json!({"tasks": 3, "events": 9, "consistent": true, "mismatches": {}});
    let text = spec::verify_view(&console, &spec::verify_document(&report));
    assert!(text.starts_with("consistent: 3 tasks, 9 events"));
    assert!(!text.contains("{}"));
}
