//! Deterministic failure-pattern detection (`analytics/failures.py`): the findings over the checks,
//! attempts, tasks, files, dependencies and relations of one workspace, each with the evidence it
//! cites. The fingerprint of a finding is a digest of its kind and title. The caller supplies the
//! digest (SHA-256 as hex; the first 32 digits are kept), so the analysis needs no hash function and
//! stays free of I/O. The instant the report is generated at is also the caller's.

use std::cmp::Reverse;

use rpds::{RedBlackTreeMapSync, RedBlackTreeSetSync, VectorSync};
use serde_json::Value;

use super::service::object;
use crate::Facts;
use crate::source::AnalyticsInput;

/// A failed (or errored) check of the workspace, with the task and the attempt of its run.
#[derive(Debug, Clone)]
struct FailedCheck {
    name: Option<String>,
    task_key: Option<String>,
    attempt_id: Option<String>,
    signatures: Vec<String>,
}

/// The findings of the workspace, in the order of the Python report, with the report's generation
/// instant and its note. `fingerprint` digests a finding's `kind|title` text.
pub fn failures(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    generated_at: &str,
    fingerprint: &impl Fn(&str) -> String,
) -> Value {
    let checks = failed_checks(input, workspace);
    let order = task_order(input);
    let findings: Vec<Value> = [
        frequent_tests(&checks, fingerprint),
        failure_categories(facts, workspace, fingerprint),
        failing_checks(&checks, fingerprint),
        retry_loops(input, facts, workspace, fingerprint),
        high_rework(input, facts, workspace, &order, fingerprint),
        integration_failures(input, workspace, &order, fingerprint),
        dependency_bottlenecks(input, facts, workspace, fingerprint),
        regression_follow_ups(input, facts, workspace, fingerprint),
    ]
    .concat();
    object(vec![
        ("generated_at", Value::from(generated_at)),
        ("findings", Value::Array(findings)),
        (
            "note",
            Value::from(
                "Findings are computed deterministically from recorded tasks, attempts, checks and commits.",
            ),
        ),
    ])
}

/// One finding: its kind, title, severity, metric value, sample size, evidence and fingerprint.
fn finding(
    kind: &str,
    title: String,
    severity: &str,
    value: Value,
    sample_size: usize,
    evidence: Value,
    fingerprint: &impl Fn(&str) -> String,
) -> Value {
    let digest: String = fingerprint(&format!("{kind}|{title}"))
        .chars()
        .take(32)
        .collect();
    object(vec![
        ("kind", Value::from(kind)),
        ("title", Value::from(title)),
        ("severity", Value::from(severity)),
        ("metric_value", value),
        ("sample_size", Value::from(sample_size as i64)),
        ("evidence", evidence),
        ("fingerprint", Value::from(digest)),
    ])
}

/// A nullable text as Python prints it in an f-string: the text, or `None`.
fn shown(text: &Option<String>) -> &str {
    text.as_deref().unwrap_or("None")
}

fn text(value: &Option<String>) -> Value {
    value.clone().map_or(Value::Null, Value::String)
}

/// The distinct values of a list, sorted, as `sorted(set(...))`.
fn sorted_unique(values: impl IntoIterator<Item = String>) -> Vec<String> {
    values
        .into_iter()
        .fold(RedBlackTreeSetSync::<String>::new_sync(), |set, value| {
            set.insert(value)
        })
        .iter()
        .cloned()
        .collect()
}

/// The groups of `items` by key, each with its items in order, the groups in the order their first
/// item appears.
fn groups<K: Ord + Clone, V: Clone>(items: impl IntoIterator<Item = (K, V)>) -> Vec<(K, Vec<V>)> {
    let collected = items.into_iter().fold(
        RedBlackTreeMapSync::<K, (usize, VectorSync<V>)>::new_sync(),
        |map, (key, value)| {
            let (rank, values) = map
                .get(&key)
                .cloned()
                .unwrap_or_else(|| (map.size(), VectorSync::new_sync()));
            map.insert(key, (rank, values.push_back(value)))
        },
    );
    collected
        .iter()
        .fold(
            RedBlackTreeMapSync::<usize, (K, Vec<V>)>::new_sync(),
            |ranked, (key, (rank, values))| {
                ranked.insert(*rank, (key.clone(), values.iter().cloned().collect()))
            },
        )
        .values()
        .cloned()
        .collect()
}

/// The position of each task key in the task table: the order `list(DISTINCT key)` gives, which
/// follows the rows the aggregate scans (creation order in practice).
type TaskOrder = RedBlackTreeMapSync<String, usize>;

fn task_order(input: &AnalyticsInput) -> TaskOrder {
    input
        .tasks
        .iter()
        .enumerate()
        .filter_map(|(position, task)| Some((task.key.clone()?, position)))
        .fold(TaskOrder::new_sync(), |order, (key, position)| {
            order.insert(key, position)
        })
}

/// The distinct keys in task-table order.
fn in_order(keys: impl IntoIterator<Item = String>, order: &TaskOrder) -> Vec<String> {
    sorted_by(sorted_unique(keys), |key| {
        order.get(key).copied().unwrap_or(usize::MAX)
    })
}

/// The items sorted by a key, ties kept in their order (an `ORDER BY` with a stable tie-break).
fn sorted_by<T: Clone, S: Ord>(items: Vec<T>, key: impl Fn(&T) -> S) -> Vec<T> {
    items
        .iter()
        .enumerate()
        .fold(
            RedBlackTreeMapSync::<(S, usize), T>::new_sync(),
            |ordered, (position, item)| ordered.insert((key(item), position), item.clone()),
        )
        .values()
        .cloned()
        .collect()
}

/// The signatures of a check, from its JSON list (`json.loads(... or "[]")`).
fn signatures(text: Option<&str>) -> Vec<String> {
    text.filter(|text| !text.is_empty())
        .and_then(|text| serde_json::from_str::<Vec<String>>(text).ok())
        .unwrap_or_default()
}

/// The failed checks of the workspace's verification runs, joined to their task.
fn failed_checks(input: &AnalyticsInput, workspace: &str) -> Vec<FailedCheck> {
    let keys = input
        .tasks
        .iter()
        .filter_map(|task| Some((task.id.clone()?, task.key.clone())))
        .fold(
            RedBlackTreeMapSync::<String, Option<String>>::new_sync(),
            |keys, (id, key)| keys.insert(id, key),
        );
    let runs = input
        .verification_runs
        .iter()
        .filter(|run| run.workspace_id.as_deref() == Some(workspace))
        .filter_map(|run| Some((run.id.clone()?, run)))
        .fold(
            RedBlackTreeMapSync::<String, _>::new_sync(),
            |runs, (id, run)| runs.insert(id, run),
        );
    input
        .check_results
        .iter()
        .filter(|check| matches!(check.status.as_deref(), Some("failed" | "error")))
        .filter_map(|check| {
            let run = runs.get(check.run_id.as_ref()?)?;
            let task_key = keys.get(run.task_id.as_ref()?)?.clone();
            Some(FailedCheck {
                name: check.name.clone(),
                task_key,
                attempt_id: run.attempt_id.clone(),
                signatures: signatures(check.failure_signatures.as_deref()),
            })
        })
        .collect()
}

/// Test signatures that failed in at least two checks.
fn frequent_tests(checks: &[FailedCheck], fingerprint: &impl Fn(&str) -> String) -> Vec<Value> {
    let by_signature = groups(checks.iter().flat_map(|check| {
        check
            .signatures
            .iter()
            .map(move |signature| (signature.clone(), check.clone()))
    }));
    by_signature
        .into_iter()
        .filter(|(signature, items)| items.len() >= 2 && signature.starts_with("test:"))
        .map(|(signature, items)| {
            let count = items.len();
            finding(
                "frequently_failing_test",
                format!("{} failed {count} times", &signature[5..]),
                if count < 5 { "warning" } else { "error" },
                Value::from(count as i64),
                count,
                object(vec![
                    (
                        "tasks",
                        Value::Array(
                            sorted_unique(items.iter().filter_map(|item| item.task_key.clone()))
                                .into_iter()
                                .map(Value::from)
                                .collect(),
                        ),
                    ),
                    (
                        "attempts",
                        Value::Array(
                            sorted_unique(items.iter().filter_map(|item| item.attempt_id.clone()))
                                .into_iter()
                                .map(Value::from)
                                .collect(),
                        ),
                    ),
                ]),
                fingerprint,
            )
        })
        .collect()
}

/// Exit reasons of at least two failed attempts, most attempts first.
fn failure_categories(
    facts: &Facts,
    workspace: &str,
    fingerprint: &impl Fn(&str) -> String,
) -> Vec<Value> {
    let failed = groups(
        facts
            .attempt_facts
            .iter()
            .filter(|fact| fact.attempt.workspace_id.as_deref() == Some(workspace))
            .filter(|fact| matches!(fact.attempt.status.as_deref(), Some("failed" | "timed_out")))
            .map(|fact| {
                (
                    fact.attempt.exit_reason.clone(),
                    (fact.task_key.clone(), fact.attempt.id.clone()),
                )
            }),
    );
    sorted_by(failed, |(_, items)| Reverse(items.len()))
        .into_iter()
        .filter(|(_, items)| items.len() >= 2)
        .map(|(reason, items)| {
            let count = items.len();
            finding(
                "failure_category",
                format!("{count} attempts failed with '{}'", shown(&reason)),
                "info",
                Value::from(count as i64),
                count,
                object(vec![
                    (
                        "tasks",
                        Value::Array(
                            sorted_unique(items.iter().filter_map(|(task, _)| task.clone()))
                                .into_iter()
                                .take(50)
                                .map(Value::from)
                                .collect(),
                        ),
                    ),
                    (
                        "attempts",
                        Value::Array(items.iter().take(50).map(|(_, id)| text(id)).collect()),
                    ),
                ]),
                fingerprint,
            )
        })
        .collect()
}

/// Check names that failed at least three times.
fn failing_checks(checks: &[FailedCheck], fingerprint: &impl Fn(&str) -> String) -> Vec<Value> {
    groups(
        checks
            .iter()
            .map(|check| (check.name.clone(), check.task_key.clone())),
    )
    .into_iter()
    .filter(|(_, items)| items.len() >= 3)
    .map(|(name, items)| {
        let count = items.len();
        finding(
            "failing_check",
            format!("check '{}' failed {count} times", shown(&name)),
            "warning",
            Value::from(count as i64),
            count,
            object(vec![(
                "tasks",
                Value::Array(
                    sorted_unique(items.iter().filter_map(|task| task.clone()))
                        .into_iter()
                        .take(50)
                        .map(Value::from)
                        .collect(),
                ),
            )]),
            fingerprint,
        )
    })
    .collect()
}

/// Tasks that needed at least three attempts, most attempts first, with their attempt ids in order.
fn retry_loops(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    fingerprint: &impl Fn(&str) -> String,
) -> Vec<Value> {
    let looping = sorted_by(
        facts
            .task_facts
            .iter()
            .filter(|fact| fact.task.workspace_id.as_deref() == Some(workspace))
            .filter(|fact| fact.attempts >= 3)
            .collect(),
        |fact| Reverse(fact.attempts),
    );
    looping
        .into_iter()
        .map(|fact| {
            let numbered: Vec<(Option<i64>, Option<String>)> = input
                .attempts
                .iter()
                .filter(|attempt| fact.task.id.is_some() && attempt.task_id == fact.task.id)
                .map(|attempt| (attempt.number, attempt.id.clone()))
                .collect();
            let attempt_ids: Vec<Value> = sorted_by(numbered, |(number, _)| *number)
                .into_iter()
                .map(|(_, id)| text(&id))
                .collect();
            let key = shown(&fact.task.key).to_string();
            finding(
                "retry_loop",
                format!("{key} needed {} attempts", fact.attempts),
                "warning",
                Value::from(fact.attempts),
                fact.attempts as usize,
                object(vec![
                    ("tasks", Value::Array(vec![text(&fact.task.key)])),
                    ("attempts", Value::Array(attempt_ids)),
                ]),
                fingerprint,
            )
        })
        .collect()
}

/// A module of a repository with its edits, the tasks that touched it and their keys.
type Hot = (Option<String>, String, i64, usize, Vec<String>);

/// The repositories of a workspace's task files: each `repository:module` edited more than twice per
/// task, the ten with the most edits.
fn high_rework(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    order: &TaskOrder,
    fingerprint: &impl Fn(&str) -> String,
) -> Vec<Value> {
    let repository_names = input
        .repositories
        .iter()
        .filter_map(|repository| Some((repository.id.clone()?, repository.name.clone()?)))
        .fold(
            RedBlackTreeMapSync::<String, String>::new_sync(),
            |names, (id, name)| names.insert(id, name),
        );
    let task_keys = facts
        .task_facts
        .iter()
        .filter(|fact| fact.task.workspace_id.as_deref() == Some(workspace))
        .filter_map(|fact| Some((fact.task.id.clone()?, fact.task.key.clone())))
        .fold(
            RedBlackTreeMapSync::<String, Option<String>>::new_sync(),
            |keys, (id, key)| keys.insert(id, key),
        );
    let edits = groups(facts.task_files.iter().filter_map(|file| {
        let task_id = file.task_id.clone()?;
        let key = task_keys.get(&task_id)?.clone();
        let repository = repository_names.get(file.repository_id.as_ref()?)?.clone();
        Some((
            (file.module.clone(), repository),
            (task_id, key, file.edits),
        ))
    }));
    let hot: Vec<Hot> = edits
        .into_iter()
        .map(|((module, repository), items)| {
            let total: i64 = items.iter().map(|(_, _, count)| count).sum();
            let tasks = sorted_unique(items.iter().map(|(task, _, _)| task.clone())).len();
            let keys = in_order(items.iter().filter_map(|(_, key, _)| key.clone()), order);
            (module, repository, total, tasks, keys)
        })
        .filter(|(_, _, total, tasks, _)| *total > (*tasks as i64) * 2)
        .collect();
    sorted_by(hot, |(_, _, total, _, _)| Reverse(*total))
        .into_iter()
        .take(10)
        .map(|(module, repository, total, tasks, keys)| {
            finding(
                "high_rework",
                format!(
                    "{repository}:{} edited {total} times across {tasks} task(s)",
                    module.as_deref().unwrap_or("None")
                ),
                "info",
                Value::from(total),
                tasks,
                object(vec![(
                    "tasks",
                    Value::Array(keys.into_iter().take(50).map(Value::from).collect()),
                )]),
                fingerprint,
            )
        })
        .collect()
}

/// Repositories whose integration checks failed in at least 30% of at least two checks. A check is
/// counted once for each repository its task is linked to.
fn integration_failures(
    input: &AnalyticsInput,
    workspace: &str,
    order: &TaskOrder,
    fingerprint: &impl Fn(&str) -> String,
) -> Vec<Value> {
    let repository_names = input
        .repositories
        .iter()
        .filter_map(|repository| Some((repository.id.clone()?, repository.name.clone()?)))
        .fold(
            RedBlackTreeMapSync::<String, String>::new_sync(),
            |names, (id, name)| names.insert(id, name),
        );
    let runs = input
        .verification_runs
        .iter()
        .filter(|run| run.workspace_id.as_deref() == Some(workspace))
        .filter_map(|run| Some((run.id.clone()?, run.task_id.clone()?)))
        .fold(
            RedBlackTreeMapSync::<String, String>::new_sync(),
            |runs, (id, task)| runs.insert(id, task),
        );
    let task_keys = input
        .tasks
        .iter()
        .filter_map(|task| Some((task.id.clone()?, task.key.clone())))
        .fold(
            RedBlackTreeMapSync::<String, Option<String>>::new_sync(),
            |keys, (id, key)| keys.insert(id, key),
        );
    let checks = groups(input.check_results.iter().filter_map(|check| {
        if check.kind.as_deref() != Some("integration") {
            return None;
        }
        let task = runs.get(check.run_id.as_ref()?)?;
        let key = task_keys.get(task)?.clone();
        Some((task.clone(), (key, check.status.clone())))
    }));
    let per_repository = groups(checks.into_iter().flat_map(|(task, items)| {
        input
            .task_repositories
            .iter()
            .filter(move |link| link.task_id.as_deref() == Some(task.as_str()))
            .filter_map(|link| repository_names.get(link.repository_id.as_ref()?).cloned())
            .flat_map(move |repository| {
                items
                    .clone()
                    .into_iter()
                    .map(move |item| (repository.clone(), item))
            })
    }));
    sorted_by(per_repository, |(name, _)| name.clone())
        .into_iter()
        .filter_map(|(repository, items)| {
            let checks = items.len();
            let failed = items
                .iter()
                .filter(|(_, status)| matches!(status.as_deref(), Some("failed" | "error")))
                .count();
            let rate = failed as f64 / checks as f64;
            (checks >= 2 && rate >= 0.3).then(|| {
                finding(
                    "integration_failures",
                    format!("{repository}: {failed}/{checks} integration checks failed"),
                    "warning",
                    Value::from(rate),
                    checks,
                    object(vec![(
                        "tasks",
                        Value::Array(
                            in_order(items.iter().filter_map(|(key, _)| key.clone()), order)
                                .into_iter()
                                .take(50)
                                .map(Value::from)
                                .collect(),
                        ),
                    )]),
                    fingerprint,
                )
            })
        })
        .collect()
}

/// Tasks that block at least two other tasks, most blocked first.
fn dependency_bottlenecks(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    fingerprint: &impl Fn(&str) -> String,
) -> Vec<Value> {
    let task_keys = input
        .tasks
        .iter()
        .filter_map(|task| Some((task.id.clone()?, task.key.clone())))
        .fold(
            RedBlackTreeMapSync::<String, Option<String>>::new_sync(),
            |keys, (id, key)| keys.insert(id, key),
        );
    let workspace_tasks = facts
        .task_facts
        .iter()
        .filter(|fact| fact.task.workspace_id.as_deref() == Some(workspace))
        .filter_map(|fact| fact.task.id.clone())
        .fold(RedBlackTreeSetSync::<String>::new_sync(), |set, id| {
            set.insert(id)
        });
    let blocked = groups(input.task_dependencies.iter().filter_map(|dependency| {
        let task = dependency.task_id.clone()?;
        let blocker = dependency.depends_on_id.as_ref()?;
        if !workspace_tasks.contains(&task) {
            return None;
        }
        let blocker_key = task_keys.get(blocker)?.clone();
        Some((blocker_key, task_keys.get(&task)?.clone()))
    }));
    sorted_by(
        blocked
            .into_iter()
            .filter(|(_, items)| items.len() >= 2)
            .collect(),
        |(_, items)| Reverse(items.len()),
    )
    .into_iter()
    .map(|(blocker, items)| {
        let count = items.len();
        finding(
            "dependency_bottleneck",
            format!("{} blocks {count} tasks", shown(&blocker)),
            "info",
            Value::from(count as i64),
            count,
            object(vec![
                ("blocker", text(&blocker)),
                (
                    "tasks",
                    Value::Array(items.iter().take(50).map(text).collect()),
                ),
            ]),
            fingerprint,
        )
    })
    .collect()
}

/// Changes that were followed by at least one regression or follow-up fix, with the commits of the
/// original task (at most twenty).
fn regression_follow_ups(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    fingerprint: &impl Fn(&str) -> String,
) -> Vec<Value> {
    let task_keys = input
        .tasks
        .iter()
        .filter_map(|task| Some((task.id.clone()?, task.key.clone())))
        .fold(
            RedBlackTreeMapSync::<String, Option<String>>::new_sync(),
            |keys, (id, key)| keys.insert(id, key),
        );
    let fixes = groups(input.relations.iter().filter_map(|relation| {
        if !matches!(
            relation.relation_type.as_deref(),
            Some("regression_of" | "follows_up")
        ) {
            return None;
        }
        let source = relation.source_task_id.as_ref()?;
        let target = relation.target_task_id.as_ref()?;
        let source_key = task_keys.get(source)?.clone();
        let target_key = task_keys.get(target)?.clone();
        let source_in_workspace = input.tasks.iter().any(|task| {
            task.id.as_deref() == Some(source.as_str())
                && task.workspace_id.as_deref() == Some(workspace)
        });
        source_in_workspace.then_some((target_key, source_key))
    }));
    sorted_by(fixes, |(original, _)| original.clone())
        .into_iter()
        .map(|(original, items)| {
            let count = items.len();
            let commits: Vec<Value> = match &original {
                Some(key) => facts
                    .task_commits
                    .iter()
                    .filter(|link| {
                        facts.task_facts.iter().any(|fact| {
                            fact.task.id == link.task_id
                                && fact.task.key.as_deref() == Some(key.as_str())
                                && fact.task.workspace_id.as_deref() == Some(workspace)
                        })
                    })
                    .take(20)
                    .map(|link| text(&link.commit.sha))
                    .collect(),
                None => Vec::new(),
            };
            finding(
                "regression_follow_up",
                format!("{} was followed by {count} fix task(s)", shown(&original)),
                if count > 1 { "warning" } else { "info" },
                Value::from(count as i64),
                count,
                object(vec![
                    ("original", text(&original)),
                    ("fixes", Value::Array(items.iter().map(text).collect())),
                    ("commits", Value::Array(commits)),
                ]),
                fingerprint,
            )
        })
        .collect()
}
