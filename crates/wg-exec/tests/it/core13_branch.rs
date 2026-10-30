//! CORE-13: attempt branch names match `_branch_name` in `integrations/git/worktrees.py`.
//! Expected values were produced by running the Python function.

use wg_exec::branch::attempt_branch_name;

#[test]
fn plain_task_key_keeps_its_hyphen() {
    assert_eq!(
        attempt_branch_name("WG-46", 1, "abcdef0123456789"),
        "wg/wg-46/attempt-1-abcdef01"
    );
}

#[test]
fn spaces_slashes_underscores_and_dots_become_hyphens() {
    assert_eq!(
        attempt_branch_name("wg 1/x_y.z", 12, "0123"),
        "wg/wg-1-x-y-z/attempt-12-0123"
    );
}

#[test]
fn non_ascii_letters_become_hyphens_after_lowercasing() {
    // Python: "ÄB-7".lower() == "äb-7", then every character outside a-z0-9- becomes "-".
    let id = "é".repeat(10);
    assert_eq!(
        attempt_branch_name("ÄB-7", 3, &id),
        format!("wg/-b-7/attempt-3-{}", "é".repeat(8))
    );
}

#[test]
fn short_attempt_ids_are_kept_whole() {
    assert_eq!(attempt_branch_name("T", 2, "ab"), "wg/t/attempt-2-ab");
}
