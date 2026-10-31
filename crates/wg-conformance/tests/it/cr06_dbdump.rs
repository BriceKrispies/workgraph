//! CR-06: the Rust database dump equals `tools/contracts/dbdump.py dump` byte for byte on every
//! fixture database. The expected files in `golden/db/dump/` were written by that Python tool.

use std::path::PathBuf;

use wg_conformance::dbdump::{canonical_create_table, canonical_dump};

const REVISIONS: [&str; 11] = [
    "0001", "0002", "0003", "0004", "0005", "0006", "0007", "0008", "0009", "0010", "0011",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
}

#[test]
fn fixture_dumps_equal_python_dbdump() {
    let golden = repo_root().join("golden").join("db");
    for revision in REVISIONS {
        let database = golden.join(format!("rev-{revision}.db"));
        let expected_path = golden.join("dump").join(format!("rev-{revision}.json"));
        let expected = std::fs::read(&expected_path)
            .unwrap_or_else(|e| panic!("read {}: {e}", expected_path.display()));
        let actual = canonical_dump(&database)
            .unwrap_or_else(|e| panic!("dump {}: {e}", database.display()));
        assert!(
            actual == expected,
            "revision {revision}: dump differs from {}",
            expected_path.display()
        );
    }
}

#[test]
fn create_table_constraints_sort_after_columns() {
    let sql = "CREATE TABLE t (\n\tb INTEGER NOT NULL,\n\tPRIMARY KEY (b),\n\ta TEXT,\n\t\
               CONSTRAINT z UNIQUE (a),\n\tCHECK (a <> 'x,y')\n)";
    assert_eq!(
        canonical_create_table(sql),
        "CREATE TABLE t (\n\tb INTEGER NOT NULL,\n\ta TEXT,\n\tCHECK (a <> 'x,y'),\n\t\
         CONSTRAINT z UNIQUE (a),\n\tPRIMARY KEY (b)\n)"
    );
    let shuffled = "CREATE TABLE t (b INTEGER, PRIMARY KEY (b), a TEXT, UNIQUE (a))";
    assert_eq!(
        canonical_create_table(shuffled),
        "CREATE TABLE t (\n\tb INTEGER,\n\ta TEXT,\n\tPRIMARY KEY (b),\n\tUNIQUE (a)\n)"
    );
}
