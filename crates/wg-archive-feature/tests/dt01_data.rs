//! The readers, documents and views of `backup`, `restore`, `export workspace` and `import`.

use std::path::PathBuf;

use serde_json::json;
use wg_archive_feature::spec;
use wg_render::Console;

// covers: C037
#[test]
fn backup_reads_output_and_artifacts() {
    let m = spec::backup_command()
        .try_get_matches_from(["backup", "-o", "out.zip", "--include-artifacts"])
        .expect("the snapshot's backup command parses");
    assert_eq!(spec::backup_output(&m), Some(PathBuf::from("out.zip")));
    assert!(spec::include_artifacts(&m));
}

// covers: C124
#[test]
fn restore_reads_archive_and_flags() {
    let m = spec::restore_command()
        .try_get_matches_from(["restore", "a.zip", "--with-config", "--force"])
        .expect("the snapshot's restore command parses");
    assert_eq!(spec::path_arg(&m, "archive"), PathBuf::from("a.zip"));
    assert!(spec::with_config(&m));
    assert!(spec::force(&m));
}

// covers: C124
#[test]
fn restore_view_leaves_out_empty_fields() {
    let console = Console::new(80);
    let manifest = json!({"format": "workgraph-backup-v1", "schema_revision": null});
    let document = spec::restore_document("a.zip", &manifest, None, Some("0010"), Some("0011"));
    let text = spec::restore_view(&console, &document);
    assert!(text.contains("restored"));
    assert!(text.contains("  restored_from: a.zip"));
    assert!(text.contains("  schema_before: 0010"));
    assert!(text.contains("  schema_after: 0011"));
    assert!(!text.contains("safety_copy"));
    assert!(text.contains("{'format': 'workgraph-backup-v1', 'schema_revision': None}"));
}

// covers: C085
#[test]
fn export_view_lists_tables_sorted_by_name() {
    let console = Console::new(80);
    let tables = vec![("tasks".to_owned(), 3), ("events".to_owned(), 6)];
    let text = spec::export_view(&console, "out.jsonl", &tables);
    assert!(text.starts_with("exported to out.jsonl\n"));
    let events_at = text.find("events").expect("events row");
    let tasks_at = text.find("tasks").expect("tasks row");
    assert!(events_at < tasks_at);
}

// covers: C098
#[test]
fn import_document_keeps_read_order() {
    let tables = vec![("workspaces".to_owned(), 1), ("tasks".to_owned(), 3)];
    let document = spec::import_document("ws-1", &tables);
    let text = wg_json::cli_json(&document).expect("the document serialises");
    let workspaces_at = text.find("\"workspaces\"").expect("workspaces key");
    let tasks_at = text.find("\"tasks\"").expect("tasks key");
    assert!(workspaces_at < tasks_at);
}
