//! SH-11 statement checks: the FTS MATCH expression built from free text (Python's
//! `to_fts_query`), the bm25 candidate statement as `search/service.py` issues it, and the scope
//! query's placeholder numbering. No database is involved.

use wg_intel_feature::spec::search_sql::{
    SearchScope, fts_match, lexical_candidates, search_scope,
};
use wg_storecodec::Raw;
use wg_types::{Id, parse_id};

fn id(text: &str) -> Id {
    match parse_id(text) {
        Ok(id) => id,
        Err(err) => panic!("invalid fixture id {text}: {err}"),
    }
}

#[test]
fn match_expressions_quote_every_token() {
    assert_eq!(
        fts_match("billing export", true, false).as_deref(),
        Some("\"billing\" \"export\"*")
    );
    assert_eq!(
        fts_match("billing export", false, false).as_deref(),
        Some("\"billing\" \"export\"")
    );
    // Punctuation separates tokens; underscores and non-ASCII letters are word characters.
    assert_eq!(
        fts_match("a-b_c", false, false).as_deref(),
        Some("\"a\" \"b_c\"")
    );
    assert_eq!(
        fts_match("café naïve", false, false).as_deref(),
        Some("\"café\" \"naïve\"")
    );
    assert_eq!(fts_match("  !!! ", true, false), None);
    assert_eq!(fts_match("", true, false), None);
}

#[test]
fn match_any_dedupes_in_order_and_caps_at_forty_tokens() {
    // The prefix goes on the last token before the OR join, so `"alpha"*` is distinct from `"alpha"`.
    assert_eq!(
        fts_match("alpha beta alpha", true, true).as_deref(),
        Some("\"alpha\" OR \"beta\" OR \"alpha\"*")
    );
    assert_eq!(
        fts_match("alpha beta alpha", false, true).as_deref(),
        Some("\"alpha\" OR \"beta\"")
    );
    let many: Vec<String> = (0..50).map(|n| format!("t{n}")).collect();
    let expr = fts_match(&many.join(" "), false, true).expect("tokens present");
    assert_eq!(
        expr.matches(" OR ").count(),
        39,
        "40 tokens, joined by 39 ORs"
    );
    assert!(expr.starts_with("\"t0\" OR \"t1\""));
    assert!(!expr.contains("\"t40\""));
}

#[test]
fn lexical_candidates_issue_the_python_bm25_statement() {
    let statement = lexical_candidates(
        "\"billing\"*",
        id("00000000-0000-0000-0000-000000000001"),
        4,
    );
    assert_eq!(
        statement.sql,
        "SELECT task_id, bm25(task_fts, 0, 0, 5.0, 10.0, 3.0, 1.0, 2.0, 2.0) AS rank, \
         snippet(task_fts, -1, '[', ']', ' ... ', 12) AS snip \
         FROM task_fts WHERE task_fts MATCH ?1 AND workspace_id = ?2 \
         ORDER BY rank LIMIT ?3"
    );
    assert_eq!(statement.params[0], Raw::Text("\"billing\"*".to_owned()));
    assert_eq!(
        statement.params[1],
        Raw::Text("00000000-0000-0000-0000-000000000001".to_owned())
    );
    assert_eq!(statement.params[2], Raw::Int(20), "limit * 5 candidates");
}

#[test]
fn scope_placeholders_are_numbered_in_order() {
    let workspace = id("00000000-0000-0000-0000-000000000001");
    let plain = search_scope(workspace, &SearchScope::default());
    assert_eq!(
        plain.sql,
        "SELECT \"id\" FROM \"tasks\" WHERE \"workspace_id\" = ?1 AND \"archived_at\" IS NULL"
    );
    assert_eq!(plain.params.len(), 1);

    let full = SearchScope {
        include_archived: true,
        project_id: Some(workspace),
        repository_id: Some(workspace),
        category: Some("bug".into()),
        statuses: vec!["ready".into(), "accepted".into()],
        created_after: None,
        created_before: None,
        agent: Some("claude".into()),
        model: Some("opus".into()),
        changed_path: Some("billing".into()),
    };
    let statement = search_scope(workspace, &full);
    // workspace, project, repository, category, two statuses, agent, model, path.
    assert_eq!(statement.params.len(), 9);
    for n in 1..=statement.params.len() {
        assert!(statement.sql.contains(&format!("?{n}")), "missing ?{n}");
    }
    assert!(!statement.sql.contains("?10"));
    assert!(statement.sql.contains("LIKE '%' || ?9 || '%'"));
    assert!(
        !statement.sql.contains("archived_at"),
        "archived tasks are included"
    );
}
