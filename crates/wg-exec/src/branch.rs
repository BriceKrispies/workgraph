//! Worktree branch naming (CORE-13), mirroring `_branch_name` in `integrations/git/worktrees.py`.

/// The branch an attempt's managed worktree is created on: `wg/<slug>/attempt-<n>-<id prefix>`.
///
/// The slug is the task key lower-cased with every character outside `a-z0-9-` replaced by `-`.
/// The id prefix is the first eight characters of the attempt id.
pub fn attempt_branch_name(task_key: &str, attempt_number: i64, attempt_id: &str) -> String {
    let slug: String = task_key
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let prefix: String = attempt_id.chars().take(8).collect();
    format!("wg/{slug}/attempt-{attempt_number}-{prefix}")
}
