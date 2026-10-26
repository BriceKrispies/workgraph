//! WG-3738: `resources/read` against the recorded reply.
//!
//! The recorded reply's `contents` item does carry a `uri` (the recorder sorts keys, so it reads
//! `mimeType`, `text`, `uri`; the first reading of the transcript missed the last key). The
//! `rmcp` type the server answers with requires `uri`, and `read_result` writes it, so there is
//! no divergence to review: the reply equals the recorded one.

#![allow(clippy::expect_used)]

use std::path::PathBuf;

use serde_json::Value;
use wg_mcpspec::read_result;

#[test]
fn read_result_equals_the_recorded_reply() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../contracts/mcp/stdio.jsonl");
    let text = std::fs::read_to_string(path).expect("transcript");
    let row = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("json line"))
        .find(|row| row["label"] == "resources/read:catalog")
        .expect("the catalog read is recorded");
    let uri = row["request"]["params"]["uri"].as_str().expect("uri");
    let result = &row["response"]["result"];

    let item = &result["contents"][0];
    assert_eq!(item["uri"], uri, "the recorded item names the resource");
    let body = item["text"].as_str().expect("text");
    // Value equality ignores key order, which the recorder normalised.
    assert_eq!(&read_result(uri, body), result);
}
