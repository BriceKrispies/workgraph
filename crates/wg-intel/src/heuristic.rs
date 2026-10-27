//! The deterministic offline provider (`workgraph.integrations.llm.heuristic`).
//!
//! Its outputs are heuristic and recorded as such: a keyword-weighted classification, a split of
//! bullet items into children, a templated narrative, and signed feature-hash embeddings. The
//! prompt marks the analysed text with `<task>...</task>` or `<facts>...</facts>`.

use std::collections::BTreeSet;
use std::fmt;
use std::sync::LazyLock;

use regex::Regex;
use rpds::RedBlackTreeMap;
use serde_json::{Value, json};

use crate::order::{py_sum, round_to, sorted_by};
use crate::pyrepr::py_str;
use wg_types::pyval::{or_value, prefix, truthy};

pub const MODEL: &str = "heuristic-v1";
pub const EMBED_DIM: usize = 256;

/// Weighted keywords per category. A keyword with a space counts substring occurrences; any other
/// keyword counts the words that start with it.
pub const CATEGORY_KEYWORDS: &[(&str, &[(&str, f64)])] = &[
    (
        "bug_fix",
        &[
            ("bug", 3.0),
            ("fix", 2.5),
            ("crash", 3.0),
            ("error", 1.5),
            ("broken", 2.5),
            ("regression", 3.0),
            ("incorrect", 2.0),
            ("fails", 2.0),
            ("exception", 2.0),
            ("wrong", 1.5),
            ("issue", 1.0),
        ],
    ),
    (
        "feature",
        &[
            ("add", 1.5),
            ("feature", 3.0),
            ("implement", 2.0),
            ("support", 1.5),
            ("new", 1.5),
            ("allow", 1.5),
            ("introduce", 2.0),
            ("enable", 1.5),
            ("endpoint", 1.5),
            ("ui", 1.0),
        ],
    ),
    (
        "refactor",
        &[
            ("refactor", 4.0),
            ("restructure", 3.0),
            ("cleanup", 2.5),
            ("clean up", 2.5),
            ("simplify", 2.5),
            ("extract", 2.0),
            ("rename", 2.0),
            ("decouple", 3.0),
            ("reorganize", 3.0),
        ],
    ),
    (
        "maintenance",
        &[
            ("upgrade", 2.5),
            ("bump", 3.0),
            ("dependency", 2.0),
            ("dependencies", 2.0),
            ("deprecat", 2.0),
            ("chore", 3.0),
            ("update version", 3.0),
            ("housekeeping", 3.0),
        ],
    ),
    (
        "infrastructure",
        &[
            ("ci", 2.5),
            ("pipeline", 2.5),
            ("docker", 3.0),
            ("kubernetes", 3.0),
            ("deploy", 3.0),
            ("terraform", 3.0),
            ("build system", 3.0),
            ("github actions", 3.0),
            ("infrastructure", 4.0),
        ],
    ),
    (
        "performance",
        &[
            ("performance", 4.0),
            ("slow", 3.0),
            ("latency", 3.0),
            ("optimiz", 3.0),
            ("speed up", 3.0),
            ("memory", 2.0),
            ("faster", 2.5),
            ("cache", 2.0),
            ("throughput", 3.0),
        ],
    ),
    (
        "security",
        &[
            ("security", 4.0),
            ("vulnerab", 4.0),
            ("cve", 4.0),
            ("auth", 2.0),
            ("xss", 4.0),
            ("injection", 3.5),
            ("csrf", 4.0),
            ("permission", 2.0),
            ("encrypt", 3.0),
            ("secret", 2.5),
            ("token", 1.5),
        ],
    ),
    (
        "testing",
        &[
            ("test", 3.0),
            ("tests", 3.0),
            ("coverage", 3.0),
            ("flaky", 3.5),
            ("pytest", 3.0),
            ("e2e", 2.5),
            ("unit test", 3.0),
            ("integration test", 3.0),
        ],
    ),
    (
        "documentation",
        &[
            ("doc", 2.5),
            ("docs", 3.0),
            ("documentation", 4.0),
            ("readme", 4.0),
            ("guide", 2.0),
            ("tutorial", 3.0),
            ("typo", 3.0),
            ("comment", 1.5),
        ],
    ),
    (
        "research",
        &[
            ("investigate", 3.5),
            ("research", 4.0),
            ("spike", 4.0),
            ("evaluate", 2.5),
            ("explore", 2.5),
            ("prototype", 2.5),
            ("compare", 2.0),
            ("feasibility", 3.5),
        ],
    ),
    (
        "architecture",
        &[
            ("architecture", 4.0),
            ("design", 2.0),
            ("adr", 3.5),
            ("boundary", 2.5),
            ("module", 1.5),
            ("migration plan", 3.0),
            ("system design", 4.0),
            ("service split", 3.5),
            ("monolith", 3.0),
        ],
    ),
];

/// Domain keywords; the domain with the most substring occurrences wins (ties go to the greater
/// name), or `general` when none occurs.
pub const DOMAINS: &[(&str, &[&str])] = &[
    (
        "frontend",
        &[
            "ui",
            "react",
            "css",
            "page",
            "button",
            "frontend",
            "component",
            "web client",
            "browser",
        ],
    ),
    (
        "backend",
        &[
            "api", "endpoint", "server", "service", "database", "query", "backend", "handler",
        ],
    ),
    (
        "data",
        &[
            "etl",
            "analytics",
            "pipeline",
            "warehouse",
            "duckdb",
            "report",
            "metrics",
        ],
    ),
    (
        "devops",
        &[
            "ci",
            "deploy",
            "docker",
            "kubernetes",
            "terraform",
            "pipeline",
        ],
    ),
    (
        "auth",
        &[
            "auth",
            "login",
            "oauth",
            "session",
            "token",
            "password",
            "permission",
        ],
    ),
    ("sdk", &["sdk", "client library", "package", "library"]),
];

/// Words that do not count as tags or embedding features.
const STOP: &[&str] = &[
    "the", "a", "an", "and", "or", "to", "of", "in", "for", "on", "with", "is", "be", "this",
    "that", "it",
];

fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|error| panic!("built-in pattern {pattern:?}: {error}"))
}

/// `[a-z0-9]+` over lowercased text: the word tokens the heuristics count.
static ASCII_WORD: LazyLock<Regex> = LazyLock::new(|| compile("[a-z0-9]+"));
/// Hyphenated names such as `api-gateway`, which the classifier reports as affected systems.
static HYPHENATED: LazyLock<Regex> = LazyLock::new(|| compile(r"\b([a-z0-9]+(?:-[a-z0-9]+)+)\b"));
/// A bullet or numbered item; the item text is group 1.
static BULLET: LazyLock<Regex> = LazyLock::new(|| compile(r"(?m)^\s*(?:[-*]|\d+[.)])\s+(.+)$"));
static SEQUENCE_WORD: LazyLock<Regex> = LazyLock::new(|| compile(r"\b(then|after|once)\b"));

/// Errors of the heuristic provider. The messages match the Python exceptions they replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeuristicError {
    /// The prompt has no `<marker>...</marker>` block.
    MissingBlock(&'static str),
    /// The facts block is not JSON.
    NotJson,
    /// The facts are JSON but not an object (Python calls `.get` on them).
    NotAnObject(&'static str),
    /// A highlight or opportunity value is neither a list nor a string.
    NotSliceable(&'static str),
    /// The schema title names an output this provider does not produce.
    Unsupported(String),
}

impl fmt::Display for HeuristicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBlock(marker) => write!(
                f,
                "heuristic provider requires <{marker}>...</{marker}> in the prompt"
            ),
            Self::NotJson => write!(f, "facts block is not valid JSON"),
            Self::NotAnObject(type_name) => {
                write!(f, "'{type_name}' object has no attribute 'get'")
            }
            Self::NotSliceable(type_name) => {
                write!(f, "'{type_name}' object is not subscriptable")
            }
            Self::Unsupported(title) => {
                write!(f, "heuristic provider cannot produce '{title}' outputs")
            }
        }
    }
}

impl std::error::Error for HeuristicError {}

/// The offline model names this provider serves, and whether `model` is one of them.
#[must_use]
pub fn list_models() -> Vec<String> {
    vec![MODEL.to_string(), format!("hash-embed-{EMBED_DIM}")]
}

/// `(usable, message)` for a model name, as `check_model` reports it.
#[must_use]
pub fn check_model(model: &str) -> (bool, String) {
    let usable = list_models().iter().any(|name| name == model);
    let message = if usable {
        "built-in".to_string()
    } else {
        format!("unknown heuristic model '{model}'; use {MODEL} or hash-embed-{EMBED_DIM}")
    };
    (usable, message)
}

/// Answers a structured-output request. `schema_title` is the JSON schema's `title`; `prompt`
/// must carry the block the output reads.
pub fn generate_json(schema_title: Option<&str>, prompt: &str) -> Result<Value, HeuristicError> {
    match schema_title {
        Some("TaskClassification") => Ok(classify_text(extract("task", prompt)?)),
        Some("TaskDecomposition") => Ok(decompose_text(extract("task", prompt)?)),
        Some("Narrative") => narrative_from_facts(extract("facts", prompt)?),
        other => Err(HeuristicError::Unsupported(
            other.unwrap_or("None").to_string(),
        )),
    }
}

/// The text between the first `<marker>` and the first `</marker>` after it.
fn extract<'a>(marker: &'static str, prompt: &'a str) -> Result<&'a str, HeuristicError> {
    let open = format!("<{marker}>");
    let close = format!("</{marker}>");
    prompt
        .find(&open)
        .and_then(|start| {
            let body = &prompt[start + open.len()..];
            body.find(&close).map(|end| &body[..end])
        })
        .ok_or(HeuristicError::MissingBlock(marker))
}

/// The keyword-weighted `TaskClassification` of a text.
#[must_use]
pub fn classify_text(text: &str) -> Value {
    let lowered = text.to_lowercase();
    let words: Vec<&str> = ASCII_WORD.find_iter(&lowered).map(|m| m.as_str()).collect();
    let scores: Vec<(&str, f64)> = CATEGORY_KEYWORDS
        .iter()
        .map(|(category, keywords)| {
            let score = keywords.iter().fold(0.0, |total, (keyword, weight)| {
                let occurrences = if keyword.contains(' ') {
                    lowered.matches(keyword).count()
                } else {
                    words
                        .iter()
                        .filter(|word| word.starts_with(keyword))
                        .count()
                };
                total + weight * occurrences as f64
            });
            (*category, score)
        })
        .collect();
    // Python sorts by (-score, name) and reads the first two entries; the best is the maximum
    // score with the smallest name, and the runner-up is the maximum of the rest.
    let best = scores
        .iter()
        .copied()
        .reduce(|a, b| {
            if b.1 > a.1 || (b.1 == a.1 && b.0 < a.0) {
                b
            } else {
                a
            }
        })
        .unwrap_or(("other", 0.0));
    let second = scores
        .iter()
        .filter(|entry| entry.0 != best.0)
        .map(|entry| entry.1)
        .fold(0.0, f64::max);
    let best_score = best.1;
    let (category, confidence) = if best_score <= 0.0 {
        ("other", 0.2)
    } else {
        let raw = 0.35 + 0.5 * (best_score - second) / (best_score + 1e-9);
        (best.0, round_to(raw.min(0.9), 3))
    };

    let domain_counts: Vec<(&str, usize)> = DOMAINS
        .iter()
        .map(|(name, keywords)| {
            let count = keywords
                .iter()
                .map(|keyword| lowered.matches(keyword).count())
                .sum();
            (*name, count)
        })
        .collect();
    let top_domain = domain_counts
        .iter()
        .copied()
        .reduce(|a, b| if (b.1, b.0) > (a.1, a.0) { b } else { a })
        .unwrap_or(("general", 0));
    let domain = if top_domain.1 == 0 {
        "general"
    } else {
        top_domain.0
    };

    let complexity = match words.len() {
        0..15 => "trivial",
        15..60 => "small",
        60..200 => "medium",
        200..500 => "large",
        _ => "very_large",
    };

    let frequencies = words
        .iter()
        .copied()
        .filter(|word| !STOP.contains(word) && word.chars().count() > 3)
        .fold(RedBlackTreeMap::<&str, usize>::new(), |counts, token| {
            let seen = counts.get(token).copied().unwrap_or(0);
            counts.insert(token, seen + 1)
        });
    let tags: Vec<&str> = sorted_by(
        frequencies.iter().map(|(token, count)| (*token, *count)),
        |a, b| a.1 > b.1,
    )
    .into_iter()
    .take(4)
    .map(|(token, _)| token)
    .collect();

    let systems: Vec<String> = HYPHENATED
        .captures_iter(&lowered)
        .filter_map(|captures| captures.get(1))
        .map(|m| m.as_str().to_string())
        .filter(|system| system.chars().count() > 4)
        .collect::<BTreeSet<String>>()
        .into_iter()
        .take(5)
        .collect();

    json!({
        "category": category,
        "subtype": format!("{category}:{domain}"),
        "domain": domain,
        "affected_systems": systems,
        "complexity": complexity,
        "tags": tags,
        "confidence": confidence,
        "rationale": format!(
            "keyword score {best_score:.1} (runner-up {second:.1}); heuristic rules, not a model judgement"
        ),
    })
}

/// Splits the bullet and numbered items of a text into up to 12 child tasks. An item after the
/// first that says `then`, `after` or `once` depends on the child before it.
#[must_use]
pub fn decompose_text(text: &str) -> Value {
    let items: Vec<String> = BULLET
        .captures_iter(text)
        .filter_map(|captures| captures.get(1))
        .map(|m| m.as_str().trim_matches([' ', '-', '*', '\t']).to_string())
        .collect();
    let children: Vec<Value> = items
        .iter()
        .take(12)
        .enumerate()
        .map(|(index, item)| {
            let depends_on: Vec<String> =
                if index > 0 && SEQUENCE_WORD.is_match(&item.to_lowercase()) {
                    vec![format!("c{index}")]
                } else {
                    Vec::new()
                };
            json!({
                "ref": format!("c{}", index + 1),
                "title": prefix(item, 200),
                "description": format!("Derived from parent checklist item: {item}"),
                "depends_on": depends_on,
                "repositories": Vec::<String>::new(),
            })
        })
        .collect();
    json!({
        "children": children,
        "rationale": "split from the task's bullet/checklist items (heuristic)",
    })
}

/// The templated `Narrative` of a facts document: observations are the first 8 highlights,
/// opportunities the first 5. Non-string values are rendered as Python's `str()` renders them.
pub fn narrative_from_facts(facts_json: &str) -> Result<Value, HeuristicError> {
    let facts: Value = serde_json::from_str(facts_json).map_err(|_| HeuristicError::NotJson)?;
    let Some(object) = facts.as_object() else {
        return Err(HeuristicError::NotAnObject(python_type_name(&facts)));
    };
    let summary = or_value(
        object.get("summary"),
        Value::String("Engineering activity summary.".to_string()),
    );
    let observations = leading_items(object.get("highlights"), 8)?;
    let opportunities = leading_items(object.get("opportunities"), 5)?;
    Ok(json!({
        "summary": summary,
        "observations": observations,
        "opportunities": opportunities,
    }))
}

/// `(value or [])[:limit]` as strings. A string slices by characters.
fn leading_items(value: Option<&Value>, limit: usize) -> Result<Vec<String>, HeuristicError> {
    match value {
        None => Ok(Vec::new()),
        Some(found) if !truthy(found) => Ok(Vec::new()),
        Some(Value::Array(items)) => Ok(items.iter().take(limit).map(py_str).collect()),
        Some(Value::String(text)) => Ok(text.chars().take(limit).map(String::from).collect()),
        Some(other) => Err(HeuristicError::NotSliceable(python_type_name(other))),
    }
}

/// The Python type name of a JSON value, as its error messages name it.
fn python_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

/// A `dim`-dimensional signed feature-hash embedding of the words and word pairs of a text,
/// L2-normalised. Each feature adds `+1` or `-1` to one bucket chosen by its BLAKE2b digest.
#[must_use]
pub fn hash_embedding(text: &str, dim: usize) -> Vec<f64> {
    if dim == 0 {
        return Vec::new();
    }
    let lowered = text.to_lowercase();
    let words: Vec<&str> = ASCII_WORD
        .find_iter(&lowered)
        .map(|m| m.as_str())
        .filter(|word| !STOP.contains(word))
        .collect();
    let bigrams = words
        .windows(2)
        .map(|pair| format!("{}_{}", pair[0], pair[1]));
    let features: Vec<String> = words
        .iter()
        .map(|word| word.to_string())
        .chain(bigrams)
        .collect();
    let vector = features.iter().fold(vec![0.0; dim], |buckets, feature| {
        add_feature(&buckets, feature)
    });
    let norm = py_sum(vector.iter().map(|v| v * v)).sqrt();
    let norm = if norm == 0.0 { 1.0 } else { norm };
    vector.iter().map(|v| v / norm).collect()
}

fn add_feature(buckets: &[f64], feature: &str) -> Vec<f64> {
    let digest = wg_hash::blake2b(feature.as_bytes(), 8);
    let index =
        u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]) as usize % buckets.len();
    let sign = if digest[4] & 1 == 1 { 1.0 } else { -1.0 };
    buckets
        .iter()
        .enumerate()
        .map(|(bucket, value)| {
            if bucket == index {
                value + sign
            } else {
                *value
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_without_keywords_is_other_with_low_confidence() {
        let value = classify_text("");
        assert_eq!(value["category"], "other");
        assert_eq!(value["confidence"], 0.2);
        assert_eq!(value["domain"], "general");
        assert_eq!(value["complexity"], "trivial");
    }

    #[test]
    fn provider_reads_the_task_block_only() {
        let prompt = "Fix the crash.\n<task>Add a feature</task> and fix bug";
        let value = generate_json(Some("TaskClassification"), prompt).unwrap_or(Value::Null);
        assert_eq!(value["category"], "feature");
        assert_eq!(
            generate_json(Some("Narrative"), "no block"),
            Err(HeuristicError::MissingBlock("facts"))
        );
    }

    #[test]
    fn unknown_titles_are_unsupported() {
        assert_eq!(
            generate_json(None, "<task>x</task>"),
            Err(HeuristicError::Unsupported("None".to_string()))
        );
    }

    #[test]
    fn check_model_names_the_usable_models() {
        assert_eq!(check_model(MODEL), (true, "built-in".to_string()));
        assert!(!check_model("gpt").0);
    }
}
