//! The controlled, versioned task taxonomy: the classifier prompt, its input hash and the validation
//! of a model's `TaskClassification` output (`workgraph.classification.taxonomy`).

use serde_json::{Map, Value};

use crate::order::unique;

pub const TAXONOMY_VERSION: &str = "2026.1";
pub const PROMPT_VERSION: &str = "classify-v1";

pub const CATEGORIES: [&str; 12] = [
    "bug_fix",
    "feature",
    "refactor",
    "maintenance",
    "infrastructure",
    "performance",
    "security",
    "testing",
    "documentation",
    "research",
    "architecture",
    "other",
];

/// One description per category, in the order of [`CATEGORIES`].
pub const CATEGORY_DESCRIPTIONS: [(&str, &str); 12] = [
    ("bug_fix", "Correcting behaviour that is wrong or broken."),
    ("feature", "New user- or developer-facing capability."),
    ("refactor", "Restructuring code without changing behaviour."),
    (
        "maintenance",
        "Dependency upgrades, chores, deprecations, housekeeping.",
    ),
    (
        "infrastructure",
        "CI/CD, build, deployment, environments, tooling.",
    ),
    (
        "performance",
        "Speed, latency, memory or throughput improvements.",
    ),
    (
        "security",
        "Vulnerabilities, authn/authz, secrets, hardening.",
    ),
    ("testing", "Adding or fixing tests, coverage, flakiness."),
    ("documentation", "Docs, READMEs, guides, comments."),
    (
        "research",
        "Investigation, spikes, evaluations, prototypes.",
    ),
    (
        "architecture",
        "System design, boundaries, large structural decisions.",
    ),
    ("other", "Does not fit any category above."),
];

pub const COMPLEXITY: [&str; 5] = ["trivial", "small", "medium", "large", "very_large"];

pub const SYSTEM_PROMPT: &str = "You classify software engineering tasks into a fixed taxonomy. Respond with JSON that matches \
the provided schema exactly. Base the answer only on the task text. Use 'other' when nothing fits. \
Confidence is your calibrated probability that the category is correct.";

/// Limits of `TaskClassification`, as the Python model declares them.
const TEXT_MAX_CHARS: usize = 100;
const RATIONALE_MAX_CHARS: usize = 2000;
const LIST_MAX_ITEMS: usize = 10;
const DESCRIPTION_PROMPT_CHARS: usize = 8000;
const CRITERIA_PROMPT_ITEMS: usize = 20;

/// The classifier prompt for a task. The task text sits between `<task>` and `</task>`.
#[must_use]
pub fn build_prompt(
    title: &str,
    description: &str,
    criteria: &[String],
    tags: &[String],
) -> String {
    let description_lines: Vec<String> = if description.is_empty() {
        Vec::new()
    } else {
        vec![
            "Description:".to_string(),
            description.chars().take(DESCRIPTION_PROMPT_CHARS).collect(),
        ]
    };
    let criteria_lines: Vec<String> = if criteria.is_empty() {
        Vec::new()
    } else {
        std::iter::once("Acceptance criteria:".to_string())
            .chain(
                criteria
                    .iter()
                    .take(CRITERIA_PROMPT_ITEMS)
                    .map(|criterion| format!("- {criterion}")),
            )
            .collect()
    };
    let tag_lines: Vec<String> = if tags.is_empty() {
        Vec::new()
    } else {
        vec![format!("Existing tags: {}", tags.join(", "))]
    };
    std::iter::once("Categories:".to_string())
        .chain(
            CATEGORY_DESCRIPTIONS
                .iter()
                .map(|(name, text)| format!("- {name}: {text}")),
        )
        .chain([
            format!("Complexity levels: {}", COMPLEXITY.join(", ")),
            String::new(),
            "Task to classify:".to_string(),
            "<task>".to_string(),
            format!("Title: {title}"),
        ])
        .chain(description_lines)
        .chain(criteria_lines)
        .chain(tag_lines)
        .chain(std::iter::once("</task>".to_string()))
        .collect::<Vec<String>>()
        .join("\n")
}

/// SHA-256 over the prompt version, taxonomy version, title, description and criteria, joined by
/// the unit separator (`\x1f`). Changing any of them yields a new hash.
#[must_use]
pub fn input_hash(title: &str, description: &str, criteria: &[String]) -> String {
    let material = [PROMPT_VERSION, TAXONOMY_VERSION, title, description]
        .into_iter()
        .map(str::to_string)
        .chain(criteria.iter().cloned())
        .collect::<Vec<String>>()
        .join("\u{1f}");
    wg_hash::sha256_hex(material.as_bytes())
}

/// Validates a model's output against `TaskClassification` (pydantic semantics: lax float
/// coercion, `extra = "forbid"`, field order in the error list). Returns the dumped model, with
/// defaults filled in and the list fields cleaned, or the pydantic error keys `type:location`.
pub fn validate_classification(payload: &Value) -> Result<Value, Vec<String>> {
    let Some(object) = payload.as_object() else {
        return Err(vec!["model_type:".to_string()]);
    };
    let fields: Vec<(&str, Result<Value, Vec<String>>)> = vec![
        ("category", literal(object, "category", &CATEGORIES)),
        ("subtype", text(object, "subtype", TEXT_MAX_CHARS, None)),
        ("domain", text(object, "domain", TEXT_MAX_CHARS, None)),
        ("affected_systems", string_list(object, "affected_systems")),
        ("complexity", literal(object, "complexity", &COMPLEXITY)),
        ("tags", string_list(object, "tags")),
        ("confidence", confidence(object)),
        (
            "rationale",
            text(object, "rationale", RATIONALE_MAX_CHARS, Some("")),
        ),
    ];
    let known: Vec<&str> = fields.iter().map(|(name, _)| *name).collect();
    let extra_errors = object
        .keys()
        .filter(|key| !known.contains(&key.as_str()))
        .map(|key| format!("extra_forbidden:{key}"));
    let errors: Vec<String> = fields
        .iter()
        .filter_map(|(_, result)| result.as_ref().err())
        .flatten()
        .cloned()
        .chain(extra_errors)
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(Value::Object(
        fields
            .into_iter()
            .filter_map(|(name, result)| result.ok().map(|value| (name.to_string(), value)))
            .collect::<Map<String, Value>>(),
    ))
}

fn literal(
    object: &Map<String, Value>,
    name: &str,
    allowed: &[&str],
) -> Result<Value, Vec<String>> {
    match object.get(name) {
        None => Err(vec![format!("missing:{name}")]),
        Some(Value::String(value)) if allowed.contains(&value.as_str()) => {
            Ok(Value::String(value.clone()))
        }
        Some(_) => Err(vec![format!("literal_error:{name}")]),
    }
}

fn text(
    object: &Map<String, Value>,
    name: &str,
    max_chars: usize,
    default: Option<&str>,
) -> Result<Value, Vec<String>> {
    match (object.get(name), default) {
        (None, Some(default)) => Ok(Value::String(default.to_string())),
        (None, None) => Err(vec![format!("missing:{name}")]),
        (Some(Value::String(value)), _) if value.chars().count() > max_chars => {
            Err(vec![format!("string_too_long:{name}")])
        }
        (Some(Value::String(value)), _) => Ok(Value::String(value.clone())),
        (Some(_), _) => Err(vec![format!("string_type:{name}")]),
    }
}

/// `list[str]` with `max_length=10`, then the `_clean` field validator: each item is stripped,
/// empty items are dropped, items are cut to 100 characters, and duplicates are removed.
fn string_list(object: &Map<String, Value>, name: &str) -> Result<Value, Vec<String>> {
    let items = match object.get(name) {
        None => return Ok(Value::Array(Vec::new())),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(vec![format!("list_type:{name}")]),
    };
    if items.len() > LIST_MAX_ITEMS {
        return Err(vec![format!("too_long:{name}")]);
    }
    let strings: Vec<Result<&str, String>> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            item.as_str()
                .ok_or_else(|| format!("string_type:{name}.{index}"))
        })
        .collect();
    let errors: Vec<String> = strings
        .iter()
        .filter_map(|item| item.as_ref().err().cloned())
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    let cleaned = strings
        .into_iter()
        .filter_map(Result::ok)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.chars().take(TEXT_MAX_CHARS).collect::<String>());
    Ok(Value::Array(
        unique(cleaned).into_iter().map(Value::String).collect(),
    ))
}

/// `float` with `ge=0`, `le=1` in lax mode: numbers, booleans and numeric strings are accepted.
/// A NaN fails the upper bound first, as pydantic reports it.
fn confidence(object: &Map<String, Value>) -> Result<Value, Vec<String>> {
    let value = match object.get("confidence") {
        None => return Err(vec!["missing:confidence".to_string()]),
        Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
        Some(Value::Bool(flag)) => f64::from(u8::from(*flag)),
        Some(Value::String(text)) => text
            .trim()
            .parse::<f64>()
            .map_err(|_| vec!["float_parsing:confidence".to_string()])?,
        Some(_) => return Err(vec!["float_type:confidence".to_string()]),
    };
    if value.is_nan() || value > 1.0 {
        Err(vec!["less_than_equal:confidence".to_string()])
    } else if value < 0.0 {
        Err(vec!["greater_than_equal:confidence".to_string()])
    } else {
        Ok(Value::from(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn list_fields_are_cleaned_and_deduplicated() {
        let payload = json!({
            "category": "feature", "subtype": "x", "domain": "x", "complexity": "small",
            "confidence": 0.3, "tags": ["  a  ", "", "a", " b", "   "],
        });
        let value = validate_classification(&payload).unwrap_or(Value::Null);
        assert_eq!(value["tags"], json!(["a", "b"]));
    }

    #[test]
    fn unknown_keys_are_reported_after_field_errors() {
        let payload = json!({"category": "nope", "complexity": "small", "confidence": 0.3,
            "subtype": "x", "domain": "x", "extra": 1});
        assert_eq!(
            validate_classification(&payload),
            Err(vec![
                "literal_error:category".to_string(),
                "extra_forbidden:extra".to_string()
            ])
        );
    }
}
