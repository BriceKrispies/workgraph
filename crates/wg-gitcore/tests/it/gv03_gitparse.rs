//! GV-03 (gitparse): every case of `golden/gitparse/` other than the remote URLs (those are pinned
//! by `wg-registry`, `tests/gv03_remote_urls.rs`) must produce the recorded output.

use serde_json::{Value, json};
use wg_gitcore::{
    CommitMeta, FileChange, Trailer, cherry_picked_from, file_class, language_for, module_for,
    parse_diff_tree, parse_meta, parse_numstat_z, parse_patch_ids, parse_trailers,
    parse_worktree_porcelain,
};

/// The placeholder the worktree vectors use for the directory they were generated in.
const ROOT: &str = "/wgroot";

fn load(name: &str) -> Value {
    let path = format!(
        "{}/../../golden/gitparse/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

fn cases(golden: &Value) -> Vec<Value> {
    let list = golden["cases"].as_array().cloned().unwrap_or_default();
    assert!(!list.is_empty(), "golden has no cases");
    list
}

fn check(golden: &Value, mut run: impl FnMut(&Value) -> Value) {
    let mut failures = Vec::new();
    for case in cases(golden) {
        let name = case["name"].as_str().unwrap_or("?");
        let got = run(&case["input"]);
        if got != case["output"] {
            failures.push(format!("{name}: got {got} want {}", case["output"]));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatches:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn str_of(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn trailer_json(trailer: &Trailer) -> Value {
    json!({ "key": trailer.key, "value": trailer.value })
}

fn file_json(file: &FileChange) -> Value {
    json!({
        "path": file.path,
        "old_path": file.old_path,
        "status": file.status,
        "similarity": file.similarity,
        "insertions": file.insertions,
        "deletions": file.deletions,
        "binary": file.binary,
    })
}

fn meta_json(commit: &CommitMeta) -> Value {
    json!({
        "sha": commit.sha,
        "tree": commit.tree,
        "parents": commit.parents,
        "author_name": commit.author_name,
        "author_email": commit.author_email,
        "authored_at": commit.authored_at.map(|t| t.format_isoformat()),
        "committer_name": commit.committer_name,
        "committer_email": commit.committer_email,
        "committed_at": commit.committed_at.map(|t| t.format_isoformat()),
        "trailers": commit.trailers.iter().map(trailer_json).collect::<Vec<_>>(),
        "message": commit.message,
        "subject": commit.subject(),
        "cherry_picked_from": commit.cherry_picked_from(),
    })
}

#[test]
fn trailers_match_golden() {
    check(&load("trailers"), |input| {
        let trailers = parse_trailers(str_of(&input["raw"]));
        json!({ "trailers": trailers.iter().map(trailer_json).collect::<Vec<_>>() })
    });
}

#[test]
fn cherry_pick_references_match_golden() {
    check(
        &load("cherry_pick"),
        |input| json!({ "sha": cherry_picked_from(str_of(&input["message"])) }),
    );
}

#[test]
fn commit_meta_matches_golden() {
    check(&load("commit_meta"), |input| {
        // The vectors hold a log of several commits; a parse error would surface as a mismatch.
        let commits = parse_meta(str_of(&input["raw"])).unwrap_or_default();
        json!({ "commits": commits.iter().map(meta_json).collect::<Vec<_>>() })
    });
}

#[test]
fn commit_meta_rejects_bad_dates() {
    let raw = "\u{1e}aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{1f}tree\u{1f}\u{1f}A\u{1f}a@x\u{1f}not-a-date\u{1f}C\u{1f}c@x\u{1f}\u{1f}\u{1f}Subject";
    assert!(parse_meta(raw).is_err());
}

#[test]
fn diff_tree_matches_golden() {
    check(&load("diff_tree"), |input| {
        let commits = parse_diff_tree(str_of(&input["raw"]));
        let commits: serde_json::Map<String, Value> = commits
            .iter()
            .map(|(sha, diff)| {
                let files: Vec<Value> = diff.files.iter().map(file_json).collect();
                (sha.clone(), json!({ "sha": diff.sha, "files": files }))
            })
            .collect();
        json!({ "commits": commits })
    });
}

#[test]
fn numstat_z_matches_golden() {
    check(&load("numstat_z"), |input| {
        let files: Vec<Value> = parse_numstat_z(str_of(&input["raw"]))
            .iter()
            .map(file_json)
            .collect();
        json!({ "files": files })
    });
}

#[test]
fn patch_ids_match_golden() {
    check(&load("patch_id"), |input| {
        let ids: serde_json::Map<String, Value> = parse_patch_ids(str_of(&input["raw"]))
            .into_iter()
            .map(|(commit, id)| (commit, Value::String(id)))
            .collect();
        json!({ "ids": ids })
    });
}

#[test]
fn worktree_porcelain_matches_golden() {
    let golden = load("worktree_porcelain");
    let mut failures = Vec::new();
    for case in cases(&golden) {
        let name = case["name"].as_str().unwrap_or("?");
        let nul = case["input"]["nul"].as_bool().unwrap_or(false);
        let output = str_of(&case["input"]["output"]).replace("@ROOT@", ROOT);
        let entries: Vec<Value> = parse_worktree_porcelain(&output, nul)
            .into_iter()
            .map(|entry| {
                json!({
                    "path": entry.path,
                    "head": entry.head,
                    "branch": entry.branch,
                    "detached": entry.detached,
                    "bare": entry.bare,
                    "locked": entry.locked,
                    "prunable": entry.prunable,
                })
            })
            .collect();
        let want = case["output"].to_string().replace("@ROOT@", ROOT);
        let want: Value = serde_json::from_str(&want).unwrap_or(Value::Null);
        let got = json!({ "entries": entries });
        if got != want {
            failures.push(format!("{name}: got {got} want {want}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn file_classes_match_golden() {
    check(&load("file_classes"), |input| {
        let path = str_of(&input["path"]);
        let module: serde_json::Map<String, Value> = (1..=3)
            .map(|depth| (depth.to_string(), Value::String(module_for(path, depth))))
            .collect();
        json!({
            "class": file_class(path),
            "language": language_for(path),
            "module": module,
        })
    });
}
