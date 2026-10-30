//! Archive format checks (SH-20): the analytics encodings against DuckDB's own output, the
//! Parquet bytes read back, and the JSON Lines record shape.

use std::error::Error;
use std::path::PathBuf;

use arrow_array::{Array, RecordBatchReader};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use wg_archivespec as spec;
use wg_storecodec::Raw;
use wg_types::Timestamp;

type TestResult = Result<(), Box<dyn Error>>;

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../golden/archive")
        .join(name)
}

/// The dataset `golden/archive/duckdb-probe.*` was written from by DuckDB 1.3.2.
fn probe() -> spec::Dataset {
    let text = |value: &str| Raw::Text(value.to_string());
    spec::Dataset {
        columns: ["i", "f", "s", "e", "u"]
            .iter()
            .map(|name| name.to_string())
            .collect(),
        rows: vec![
            vec![
                Raw::Int(1),
                Raw::Real(1.5),
                text("a,b"),
                text(""),
                text("café"),
            ],
            vec![
                Raw::Int(2),
                Raw::Real(2.0),
                text("say \"hi\""),
                text("x"),
                text("tab\there"),
            ],
            vec![
                Raw::Null,
                Raw::Real(1e-5),
                text("multi\nline"),
                text("y"),
                text("back\\slash"),
            ],
            vec![
                Raw::Int(4),
                Raw::Real(1e20),
                Raw::Null,
                text("z"),
                text("x"),
            ],
        ],
        boolean_columns: Vec::new(),
    }
}

#[test]
fn csv_matches_duckdb_bytes() -> TestResult {
    let expected = std::fs::read_to_string(golden("duckdb-probe.csv"))?;
    assert_eq!(spec::csv_text(&probe()), expected);
    Ok(())
}

#[test]
fn json_matches_duckdb_bytes() -> TestResult {
    let expected = std::fs::read_to_string(golden("duckdb-probe.json"))?;
    assert_eq!(spec::json_text(&probe())?, expected);
    Ok(())
}

#[test]
fn empty_dataset_matches_duckdb() -> TestResult {
    let empty = spec::Dataset {
        columns: vec!["a".to_string()],
        rows: Vec::new(),
        boolean_columns: Vec::new(),
    };
    assert_eq!(spec::csv_text(&empty), "a\n");
    assert_eq!(spec::json_text(&empty)?, "[\n\t\n]\n");
    Ok(())
}

#[test]
fn parquet_reads_back_with_the_inferred_types_and_values() -> TestResult {
    let bytes = spec::parquet_bytes(&probe())?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from(bytes))?.build()?;
    let schema = reader.schema();
    let types: Vec<String> = schema
        .fields()
        .iter()
        .map(|f| f.data_type().to_string())
        .collect();
    assert_eq!(types, ["Int64", "Float64", "Utf8", "Utf8", "Utf8"]);
    let batches: Vec<_> = reader.collect::<Result<_, _>>()?;
    let batch = &batches[0];
    let ints = batch
        .column(0)
        .as_any()
        .downcast_ref::<arrow_array::Int64Array>()
        .ok_or("column i")?;
    assert_eq!(ints.value(0), 1);
    assert!(ints.is_null(2));
    let floats = batch
        .column(1)
        .as_any()
        .downcast_ref::<arrow_array::Float64Array>()
        .ok_or("column f")?;
    assert_eq!(floats.value(2), 1e-5);
    assert_eq!(floats.value(3), 1e20);
    let texts = batch
        .column(2)
        .as_any()
        .downcast_ref::<arrow_array::StringArray>()
        .ok_or("column s")?;
    assert_eq!(texts.value(2), "multi\nline");
    assert!(texts.is_null(3));
    Ok(())
}

#[test]
fn parquet_of_no_rows_keeps_the_columns() -> TestResult {
    let empty = spec::Dataset {
        columns: vec!["a".to_string()],
        rows: Vec::new(),
        boolean_columns: Vec::new(),
    };
    let bytes = spec::parquet_bytes(&empty)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(bytes::Bytes::from(bytes))?.build()?;
    assert_eq!(reader.schema().field(0).name(), "a");
    Ok(())
}

#[test]
fn backup_stamp_and_names() -> TestResult {
    let at = Timestamp::parse_iso("2026-10-26T05:07:35.123456+00:00")?;
    let stamp = spec::stamp(at);
    assert_eq!(stamp, "20261026T050735Z");
    assert_eq!(
        spec::backup_file_name(&stamp),
        "workgraph-backup-20261026T050735Z.zip"
    );
    assert_eq!(
        spec::pre_restore_file_name(&stamp),
        "pre-restore-20261026T050735Z.db"
    );
    Ok(())
}

#[test]
fn manifest_is_indent_two_python_json() -> TestResult {
    let text = spec::manifest_text("20261026T050735Z", "0.1.0", Some("0011"), false)?;
    let expected = "{\n  \"format\": \"workgraph-backup-v1\",\n  \"created_at\": \"20261026T050735Z\",\n  \"workgraph_version\": \"0.1.0\",\n  \"schema_revision\": \"0011\",\n  \"includes_artifacts\": false\n}";
    assert_eq!(text, expected);
    Ok(())
}

#[test]
fn entry_names_must_stay_inside_the_archive() {
    assert!(spec::is_safe_entry("workgraph.db"));
    assert!(spec::is_safe_entry("config/workspaces/default.toml"));
    assert!(spec::is_safe_entry("artifacts/runs/1.log"));
    assert!(!spec::is_safe_entry(""));
    assert!(!spec::is_safe_entry("/etc/passwd"));
    assert!(!spec::is_safe_entry("\\evil"));
    assert!(!spec::is_safe_entry("C:evil"));
    assert!(!spec::is_safe_entry("a/../../evil"));
    assert!(!spec::is_safe_entry("..\\evil"));
}

#[test]
fn export_record_is_python_dumps_of_the_row() -> TestResult {
    let table = spec::table("tasks").ok_or("tasks is in the layout")?;
    assert!(
        table
            .columns
            .iter()
            .any(|(name, _)| *name == "acceptance_criteria")
    );
    let mut row: Vec<Raw> = table.columns.iter().map(|_| Raw::Null).collect();
    let index = |name: &str| {
        table
            .columns
            .iter()
            .position(|(c, _)| *c == name)
            .ok_or("column")
    };
    row[index("id")?] = Raw::Text("abc".to_string());
    row[index("acceptance_criteria")?] = Raw::Text("[\"b\",\"a\"]".to_string());
    row[index("auto_execute")?] = Raw::Int(1);
    row[index("created_at")?] = Raw::Text("2026-10-26T05:07:35.000000+00:00".to_string());
    let line = spec::export_line(table, &row)?;
    assert!(line.starts_with("{\"table\": \"tasks\", \"row\": {\"id\": \"abc\", "));
    assert!(line.contains("\"acceptance_criteria\": [\"b\", \"a\"]"));
    assert!(line.contains("\"auto_execute\": true"));
    assert!(line.contains("\"created_at\": \"2026-10-26T05:07:35+00:00\""));
    Ok(())
}
