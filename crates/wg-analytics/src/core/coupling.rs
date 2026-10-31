//! Cross-repository coupling (`analytics/coupling.py`). A repository is touched by a task when the
//! task has a linked non-merge commit in it; two repositories co-change when one task touches
//! both. The report lists the co-change edges with their jaccard index and the tasks and commits
//! behind them, the declared dependencies, the observed-versus-declared gaps, the module impact of
//! cross-repository tasks and the coupling candidates (observed, undeclared, at least two tasks).
//!
//! Python orders repositories by their UUID and the ties of its edge and module lists by hash-set
//! order; the labels of the golden vectors erase both. Here the pair orientation follows the
//! repository ids, and ties are ordered by the pair (edges) or the module key (modules).

use std::cmp::Reverse;

use rpds::{RedBlackTreeMapSync, RedBlackTreeSetSync, VectorSync};
use serde_json::{Value, json};

use super::stats::sorted;
use crate::Facts;
use crate::source::{AnalyticsInput, CheckResult};
use crate::tasks::TaskFact;

/// A pair of repository ids, smaller first.
type Pair = (String, String);

const NOTES: [&str; 2] = [
    "Observed co-change indicates repositories modified within the same task; it is evidence of coupling, not proof of an architectural dependency.",
    "Coordination durations compare medians of different task populations and do not imply causation.",
];

/// The coupling report of one workspace. `min_support` keeps the edges of at least that many tasks,
/// and `project` (a project slug or id) keeps the tasks linked to that project.
pub fn coupling_report(
    input: &AnalyticsInput,
    facts: &Facts,
    workspace: &str,
    min_support: i64,
    project: Option<&str>,
) -> Value {
    let repos: Vec<(String, String)> = input
        .repositories
        .iter()
        .filter(|repository| repository.workspace_id.as_deref() == Some(workspace))
        .filter_map(|repository| Some((repository.id.clone()?, repository.name.clone()?)))
        .collect();
    let names = repos.iter().fold(
        RedBlackTreeMapSync::<String, String>::new_sync(),
        |names, (id, name)| names.insert(id.clone(), name.clone()),
    );
    let name_of = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_string());
    let fact_of = facts.task_facts.iter().fold(
        RedBlackTreeMapSync::<String, &TaskFact>::new_sync(),
        |index, fact| match fact.task.id.clone() {
            Some(id) => index.insert(id, fact),
            None => index,
        },
    );
    let kept_task = project_filter(input, project);

    // The non-merge commits of the workspace's tasks, one entry per task, repository and commit.
    let touches: Vec<(String, String, Option<String>)> = facts
        .task_commits
        .iter()
        .filter(|link| link.commit.is_merge == Some(false))
        .filter_map(|link| {
            let task = link.task_id.clone()?;
            let fact = fact_of.get(&task)?;
            let repository = link.commit.repository_id.clone()?;
            (fact.task.workspace_id.as_deref() == Some(workspace) && kept_task(task.as_str()))
                .then(|| (task, repository, link.commit.sha.clone()))
        })
        .collect();
    let task_repos = touches.iter().fold(
        RedBlackTreeMapSync::<String, RedBlackTreeSetSync<String>>::new_sync(),
        |map, (task, repository, _)| {
            let touched = map
                .get(task)
                .cloned()
                .unwrap_or_else(RedBlackTreeSetSync::new_sync)
                .insert(repository.clone());
            map.insert(task.clone(), touched)
        },
    );
    let commits = touches.iter().fold(
        RedBlackTreeMapSync::<Pair, VectorSync<Option<String>>>::new_sync(),
        |map, (task, repository, sha)| {
            let key = (task.clone(), repository.clone());
            let shas = map
                .get(&key)
                .cloned()
                .unwrap_or_else(VectorSync::new_sync)
                .push_back(sha.clone());
            map.insert(key, shas)
        },
    );
    // Python walks `task_repos` in insertion order: the order in which the join first returns each
    // task. Pairs and the tasks of a pair keep that order, which ranks edges of equal support.
    let task_order: Vec<&String> = touches.iter().fold(Vec::new(), |mut order, (task, _, _)| {
        if !order.contains(&task) {
            order.push(task);
        }
        order
    });
    let pair_tasks = task_order.iter().fold(
        RedBlackTreeMapSync::<Pair, VectorSync<String>>::new_sync(),
        |pairs, task| {
            let touched = task_repos
                .get(*task)
                .cloned()
                .unwrap_or_else(RedBlackTreeSetSync::new_sync);
            pairs_of(&touched).into_iter().fold(pairs, |pairs, pair| {
                let tasks = pairs
                    .get(&pair)
                    .cloned()
                    .unwrap_or_else(VectorSync::new_sync)
                    .push_back((*task).clone());
                pairs.insert(pair, tasks)
            })
        },
    );
    let pair_position: Vec<Pair> = task_order.iter().fold(Vec::new(), |mut seen, task| {
        let touched = task_repos
            .get(*task)
            .cloned()
            .unwrap_or_else(RedBlackTreeSetSync::new_sync);
        for pair in pairs_of(&touched) {
            if !seen.contains(&pair) {
                seen.push(pair);
            }
        }
        seen
    });
    let touch_counts = task_repos
        .values()
        .flat_map(|touched| touched.iter().cloned())
        .fold(
            RedBlackTreeMapSync::<String, i64>::new_sync(),
            |counts, repository| {
                let count = counts.get(&repository).copied().unwrap_or(0) + 1;
                counts.insert(repository, count)
            },
        );
    let declared: RedBlackTreeSetSync<Pair> = input
        .repository_dependencies
        .iter()
        .filter_map(|dependency| {
            let from = dependency.from_repository_id.clone()?;
            let to = dependency.to_repository_id.clone()?;
            Some(sorted_pair(from, to))
        })
        .fold(RedBlackTreeSetSync::new_sync(), |set, pair| {
            set.insert(pair)
        });

    // The edges with at least `min_support` tasks: most tasks first, ties in the order the pairs
    // were first seen (Python's stable sort of `pair_tasks`).
    let ranked = pair_tasks.iter().fold(
        RedBlackTreeMapSync::<(Reverse<usize>, usize), (Pair, VectorSync<String>)>::new_sync(),
        |ranked, (pair, tasks)| {
            let position = pair_position
                .iter()
                .position(|seen| seen == pair)
                .unwrap_or(usize::MAX);
            ranked.insert(
                (Reverse(tasks.len()), position),
                (pair.clone(), tasks.clone()),
            )
        },
    );
    let supported: Vec<(&Pair, &VectorSync<String>)> = ranked
        .iter()
        .filter(|((count, _), _)| count.0 as i64 >= min_support)
        .map(|(_, (pair, tasks))| (pair, tasks))
        .collect();
    let touched_count = |repository: &str| touch_counts.get(repository).copied().unwrap_or(0);
    let edges: Vec<Value> = supported
        .iter()
        .map(|(pair, tasks)| {
            let (a, b) = pair;
            let co_changes = tasks.len() as i64;
            let union = touched_count(a.as_str()) + touched_count(b.as_str()) - co_changes;
            let jaccard = (union != 0).then(|| round_to(co_changes as f64 / union as f64, 4));
            let evidence: Vec<Value> = tasks
                .iter()
                .take(50)
                .map(|task| {
                    json!({
                        "task_id": Value::from(task.as_str()),
                        "task_key": text(fact_of.get(task).and_then(|fact| fact.task.key.clone())),
                        "commits": Value::Object(
                            [a, b]
                                .into_iter()
                                .map(|repository| {
                                    (name_of(repository), shas(&commits, task, repository))
                                })
                                .collect(),
                        ),
                    })
                })
                .collect();
            let is_declared = declared.contains(*pair);
            json!({
                "source": Value::from(a.as_str()),
                "target": Value::from(b.as_str()),
                "source_name": Value::from(name_of(a)),
                "target_name": Value::from(name_of(b)),
                "co_changes": Value::from(co_changes),
                "jaccard": jaccard.map_or(Value::Null, Value::from),
                "declared": Value::Bool(is_declared),
                "relationship": Value::from(if is_declared {
                    "declared+observed"
                } else {
                    "observed"
                }),
                "evidence": Value::Array(evidence),
            })
        })
        .collect();
    let observed = supported
        .iter()
        .fold(RedBlackTreeSetSync::<Pair>::new_sync(), |set, (pair, _)| {
            set.insert((*pair).clone())
        });
    let declared_only: Vec<Value> = declared
        .iter()
        .filter(|(a, b)| !observed.contains(&(a.clone(), b.clone())))
        .filter(|(a, b)| names.get(a).is_some() && names.get(b).is_some())
        .map(|(a, b)| {
            json!({
                "source_name": Value::from(name_of(a)),
                "target_name": Value::from(name_of(b)),
                "relationship": Value::from("declared_only"),
            })
        })
        .collect();

    let touched_by = |task: &String| task_repos.get(task).map_or(0, RedBlackTreeSetSync::size);
    let cross: Vec<&String> = task_order
        .iter()
        .copied()
        .filter(|task| touched_by(task) > 1)
        .collect();
    let single: Vec<&String> = task_order
        .iter()
        .copied()
        .filter(|task| touched_by(task) == 1)
        .collect();
    let elapsed = |tasks: &[&String]| -> Value {
        median(
            tasks
                .iter()
                .filter_map(|task| fact_of.get(*task)?.elapsed_ms)
                .collect(),
        )
    };
    let integration: Vec<&CheckResult> = integration_check_results(input, workspace);
    let integration_failed = integration
        .iter()
        .filter(|check| matches!(check.status.as_deref(), Some("failed" | "error")))
        .count();
    let integration_checks = integration.len();
    let total_repositories: usize = task_repos.values().map(|touched| touched.size()).sum();
    let average_repositories = (task_repos.size() > 0)
        .then(|| round_to(total_repositories as f64 / task_repos.size() as f64, 3));

    let module_impact = module_impact(facts, workspace, &fact_of, &names, &cross);
    let modules: Vec<Value> = module_impact.into_iter().take(20).collect();

    let candidates: Vec<Value> = edges
        .iter()
        .filter(|edge| {
            edge["co_changes"]
                .as_i64()
                .is_some_and(|count| count >= 2_i64.max(min_support))
                && edge["declared"] == Value::Bool(false)
        })
        .map(|edge| {
            let observation = format!(
                "{} and {} changed together in {} task(s) without a declared dependency",
                edge["source_name"].as_str().unwrap_or_default(),
                edge["target_name"].as_str().unwrap_or_default(),
                edge["co_changes"].as_i64().unwrap_or_default(),
            );
            with_entry(edge, "observation", Value::from(observation))
        })
        .collect();

    let matrix = matrix(&repos, &names, &pair_tasks, &touch_counts);

    json!({
        "repositories": Value::Array(
            repos
                .iter()
                .map(|(id, name)| {
                    json!({
                        "id": Value::from(id.as_str()),
                        "name": Value::from(name.as_str()),
                        "tasks_touching": Value::from(touched_count(id.as_str())),
                    })
                })
                .collect(),
        ),
        "edges": Value::Array(edges.clone()),
        "declared_only": Value::Array(declared_only),
        "matrix": matrix,
        "summary": json!({
            "tasks_with_code": Value::from(task_repos.size() as i64),
            "cross_repository_tasks": Value::from(cross.len() as i64),
            "average_repositories_per_task": average_repositories.map_or(Value::Null, Value::from),
            "median_elapsed_ms_cross_repo": elapsed(&cross),
            "median_elapsed_ms_single_repo": elapsed(&single),
            "integration_checks": Value::from(integration_checks as i64),
            "integration_failure_rate": (integration_checks > 0)
                .then(|| round_to(integration_failed as f64 / integration_checks as f64, 4))
                .map_or(Value::Null, Value::from),
        }),
        "commonly_changed_pairs": Value::Array(
            edges
                .iter()
                .take(10)
                .map(|edge| {
                    json!({
                        "pair": Value::Array(vec![
                            edge["source_name"].clone(),
                            edge["target_name"].clone(),
                        ]),
                        "co_changes": edge["co_changes"].clone(),
                    })
                })
                .collect(),
        ),
        "modules_with_cross_repo_impact": Value::Array(modules),
        "coupling_candidates": Value::Array(candidates),
        "notes": Value::Array(NOTES.iter().map(|note| Value::from(*note)).collect()),
    })
}

/// The filter of `project`: the tasks linked to a project whose slug or id is `project`. Without
/// a project every task passes.
fn project_filter<'a>(
    input: &'a AnalyticsInput,
    project: Option<&'a str>,
) -> impl Fn(&str) -> bool + 'a {
    let matching = input
        .projects
        .iter()
        .filter(move |row| {
            project.is_some_and(|wanted| {
                row.slug.as_deref() == Some(wanted) || row.id.as_deref() == Some(wanted)
            })
        })
        .filter_map(|row| row.id.clone())
        .fold(RedBlackTreeSetSync::<String>::new_sync(), |set, id| {
            set.insert(id)
        });
    let linked = input
        .task_projects
        .iter()
        .filter_map(|link| Some((link.task_id.clone()?, link.project_id.clone()?)))
        .filter(move |(_, project_id)| matching.contains(project_id))
        .fold(
            RedBlackTreeSetSync::<String>::new_sync(),
            |set, (task, _)| set.insert(task),
        );
    move |task: &str| project.is_none() || linked.contains(task)
}

/// The unordered pairs of a task's repositories, each smaller first, in ascending order.
fn pairs_of(touched: &RedBlackTreeSetSync<String>) -> Vec<Pair> {
    let repos: Vec<&String> = touched.iter().collect();
    repos
        .iter()
        .enumerate()
        .flat_map(|(position, a)| {
            repos[position + 1..]
                .iter()
                .map(move |b| ((*a).clone(), (*b).clone()))
        })
        .collect()
}

fn sorted_pair(a: String, b: String) -> Pair {
    if a <= b { (a, b) } else { (b, a) }
}

/// The shas of a task in one repository, the first ten of them.
fn shas(
    commits: &RedBlackTreeMapSync<Pair, VectorSync<Option<String>>>,
    task: &str,
    repository: &str,
) -> Value {
    let key = (task.to_string(), repository.to_string());
    Value::Array(
        commits
            .get(&key)
            .into_iter()
            .flat_map(|shas| shas.iter().take(10))
            .map(|sha| sha.clone().map_or(Value::Null, Value::String))
            .collect(),
    )
}

/// A nullable text as a JSON string or null.
fn text(value: Option<String>) -> Value {
    value.map_or(Value::Null, Value::String)
}

/// The integration checks of the workspace's verification runs.
fn integration_check_results<'a>(
    input: &'a AnalyticsInput,
    workspace: &str,
) -> Vec<&'a CheckResult> {
    let runs = input
        .verification_runs
        .iter()
        .filter(|run| run.workspace_id.as_deref() == Some(workspace))
        .filter_map(|run| run.id.clone())
        .fold(RedBlackTreeSetSync::<String>::new_sync(), |set, id| {
            set.insert(id)
        });
    input
        .check_results
        .iter()
        .filter(|check| check.kind.as_deref() == Some("integration"))
        .filter(|check| check.run_id.as_ref().is_some_and(|run| runs.contains(run)))
        .collect()
}

/// The median of the present values, as Python's `statistics`-free helper in the report: an
/// integer for an odd count, a float for an even one, NULL when there are none.
fn median(values: Vec<i64>) -> Value {
    let clean = values
        .iter()
        .map(|value| *value as f64)
        .collect::<VectorSync<f64>>();
    let ordered = sorted(&clean);
    let middle = ordered.len() / 2;
    match ordered.len() {
        0 => Value::Null,
        len if len % 2 == 1 => Value::from(ordered[middle] as i64),
        _ => Value::from((ordered[middle - 1] + ordered[middle]) / 2.0),
    }
}

/// `round(value, digits)` for the digits the report rounds to, half to even.
fn round_to(value: f64, digits: i32) -> f64 {
    let scale = 10f64.powi(digits);
    (value * scale).round_ties_even() / scale
}

/// The modules of the cross-repository tasks: each `repository:module` with the number of
/// cross-repository tasks that edit it and their keys, most tasks first.
fn module_impact(
    facts: &Facts,
    workspace: &str,
    fact_of: &RedBlackTreeMapSync<String, &TaskFact>,
    names: &RedBlackTreeMapSync<String, String>,
    cross: &[&String],
) -> Vec<Value> {
    let modules_by_task = facts
        .task_files
        .iter()
        .filter_map(|file| {
            let task = file.task_id.clone()?;
            let fact = fact_of.get(&task)?;
            (fact.task.workspace_id.as_deref() == Some(workspace)).then(|| {
                (task, (file.repository_id.clone(), file.module.clone()))
            })
        })
        .fold(
            RedBlackTreeMapSync::<String, RedBlackTreeSetSync<(Option<String>, Option<String>)>>::new_sync(),
            |map, (task, entry)| {
                let entries = map
                    .get(&task)
                    .cloned()
                    .unwrap_or_else(RedBlackTreeSetSync::new_sync)
                    .insert(entry);
                map.insert(task, entries)
            },
        );
    let impact = cross.iter().fold(
        RedBlackTreeMapSync::<String, (i64, VectorSync<Option<String>>)>::new_sync(),
        |impact, task| {
            let task_key = fact_of.get(*task).and_then(|fact| fact.task.key.clone());
            modules_by_task
                .get(*task)
                .into_iter()
                .flat_map(|entries| entries.iter())
                .fold(impact, |impact, (repository, module)| {
                    let repository_name = repository.as_deref().map_or_else(
                        || "None".to_string(),
                        |id| names.get(id).cloned().unwrap_or_else(|| id.to_string()),
                    );
                    let key = format!("{repository_name}:{}", module.as_deref().unwrap_or("None"));
                    let (count, tasks) = impact
                        .get(&key)
                        .cloned()
                        .unwrap_or_else(|| (0, VectorSync::new_sync()));
                    impact.insert(key, (count + 1, tasks.push_back(task_key.clone())))
                })
        },
    );
    impact
        .iter()
        .fold(
            RedBlackTreeMapSync::<(Reverse<i64>, String), Value>::new_sync(),
            |ranked, (key, (count, tasks))| {
                let entry = json!({
                    "module": Value::from(key.as_str()),
                    "cross_repo_tasks": Value::from(*count),
                    "tasks": Value::Array(tasks.iter().cloned().map(text).collect()),
                });
                ranked.insert((Reverse(*count), key.clone()), entry)
            },
        )
        .values()
        .cloned()
        .collect()
}

/// The repository-by-repository matrix of co-changing tasks, with each repository's touched-task
/// count on its diagonal. Every repository of the workspace has a row and a column.
fn matrix(
    repos: &[(String, String)],
    names: &RedBlackTreeMapSync<String, String>,
    pair_tasks: &RedBlackTreeMapSync<Pair, VectorSync<String>>,
    touch_counts: &RedBlackTreeMapSync<String, i64>,
) -> Value {
    type Row = RedBlackTreeMapSync<String, i64>;
    type Matrix = RedBlackTreeMapSync<String, Row>;
    let name_of = |id: &str| names.get(id).cloned().unwrap_or_else(|| id.to_string());
    let set_cell = |grid: Matrix, row: &str, column: &str, value: i64| -> Matrix {
        let cells = grid
            .get(row)
            .cloned()
            .unwrap_or_else(Row::new_sync)
            .insert(column.to_string(), value);
        grid.insert(row.to_string(), cells)
    };
    let zeros: Row = repos.iter().fold(Row::new_sync(), |row, (_, name)| {
        row.insert(name.clone(), 0)
    });
    let blank: Matrix = repos.iter().fold(Matrix::new_sync(), |grid, (_, name)| {
        grid.insert(name.clone(), zeros.clone())
    });
    let with_pairs = pair_tasks
        .iter()
        .filter(|((a, b), _)| names.get(a).is_some() && names.get(b).is_some())
        .fold(blank, |grid, ((a, b), tasks)| {
            let count = tasks.len() as i64;
            let grid = set_cell(grid, &name_of(a), &name_of(b), count);
            set_cell(grid, &name_of(b), &name_of(a), count)
        });
    let full = touch_counts
        .iter()
        .filter(|(id, _)| names.get(id.as_str()).is_some())
        .fold(with_pairs, |grid, (id, count)| {
            set_cell(grid, &name_of(id), &name_of(id), *count)
        });
    Value::Object(
        full.iter()
            .map(|(row, cells)| {
                (
                    row.clone(),
                    Value::Object(
                        cells
                            .iter()
                            .map(|(column, value)| (column.clone(), Value::from(*value)))
                            .collect(),
                    ),
                )
            })
            .collect(),
    )
}

/// A copy of an edge object with one more entry.
fn with_entry(edge: &Value, key: &str, value: Value) -> Value {
    let entries = edge
        .as_object()
        .into_iter()
        .flat_map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())))
        .chain(std::iter::once((key.to_string(), value)));
    Value::Object(entries.collect())
}
