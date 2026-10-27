//! The controlled, versioned task taxonomy: the classifier prompt, its input hash and the validation
//! of a model's `TaskClassification` output (`workgraph.classification.taxonomy`).

use serde_json::{Map, Value};

use crate::order::unique;
use crate::pyrepr::py_repr;
use wg_types::pyval::prefix;

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
            prefix(description, DESCRIPTION_PROMPT_CHARS),
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

/// `pydantic`'s version in the error documentation links (`errors.pydantic.dev/<version>/v/...`),
/// the minor version of the pinned pydantic.
const PYDANTIC_DOCS_VERSION: &str = "2.14";

/// How much of an input's `repr` a validation error shows.
const INPUT_SHOWN_CHARS: usize = 50;

const FIELD_REQUIRED: &str = "Field required";
const EXTRA_FORBIDDEN: &str = "Extra inputs are not permitted";
const MODEL_TYPE: &str = "Input should be a valid dictionary or object to extract fields from";
const STRING_TYPE: &str = "Input should be a valid string";
const LIST_TYPE: &str = "Input should be a valid list";
const FLOAT_TYPE: &str = "Input should be a valid number";
const FLOAT_PARSING: &str = "Input should be a valid number, unable to parse string as a number";

/// One error of a `TaskClassification` validation: an entry of pydantic's `errors()`, with the
/// error type, the location, the message, the input it failed on and the context it carries.
#[derive(Debug, Clone, PartialEq)]
pub struct SchemaError {
    /// The pydantic error type, such as `literal_error` or `missing`.
    pub kind: &'static str,
    /// The location: field names and list indexes, as pydantic's `loc` tuple.
    pub loc: Vec<Value>,
    pub msg: String,
    pub input: Value,
    pub ctx: Vec<(&'static str, Value)>,
}

impl SchemaError {
    fn new(
        kind: &'static str,
        loc: Vec<Value>,
        msg: &str,
        input: &Value,
        ctx: Vec<(&'static str, Value)>,
    ) -> Self {
        Self {
            kind,
            loc,
            msg: msg.to_owned(),
            input: input.clone(),
            ctx,
        }
    }

    /// The location as pydantic prints it: the items joined by dots.
    fn location(&self) -> String {
        let items: Vec<String> = self
            .loc
            .iter()
            .map(|item| match item {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect();
        items.join(".")
    }

    /// The `type:location` key of the error (the golden vectors record these).
    fn key(&self) -> String {
        format!("{}:{}", self.kind, self.location())
    }

    /// The documentation link pydantic attaches to the error type.
    fn url(&self) -> String {
        format!(
            "https://errors.pydantic.dev/{PYDANTIC_DOCS_VERSION}/v/{}",
            self.kind
        )
    }

    /// The error as pydantic's `str(ValidationError)` lists it: the location, then the message
    /// with its type and input, then the documentation link.
    fn display(&self) -> String {
        format!(
            "{}\n  {} [type={}, input_value={}, input_type={}]\n    For further information visit {}",
            self.location(),
            self.msg,
            self.kind,
            shown_input(&self.input),
            py_type_name(&self.input),
            self.url(),
        )
    }

    /// The error as Python's `repr` shows its dictionary in `ValidationError.errors()`.
    fn repr(&self) -> String {
        let location = match self.loc.as_slice() {
            [only] => format!("({},)", py_repr(only)),
            items => format!(
                "({})",
                items
                    .iter()
                    .map(py_repr)
                    .collect::<Vec<String>>()
                    .join(", ")
            ),
        };
        let context = (!self.ctx.is_empty()).then(|| {
            let pairs: Vec<String> = self
                .ctx
                .iter()
                .map(|(name, value)| format!("'{name}': {}", py_repr(value)))
                .collect();
            format!("'ctx': {{{}}}", pairs.join(", "))
        });
        let url = self.url();
        let entries: Vec<String> = [
            Some(format!("'type': {}", py_repr(&Value::from(self.kind)))),
            Some(format!("'loc': {location}")),
            Some(format!(
                "'msg': {}",
                py_repr(&Value::from(self.msg.as_str()))
            )),
            Some(format!("'input': {}", py_repr(&self.input))),
            context,
            Some(format!("'url': {}", py_repr(&Value::from(url)))),
        ]
        .into_iter()
        .flatten()
        .collect();
        format!("{{{}}}", entries.join(", "))
    }
}

/// The text the classifier stores for a schema failure: `ValidationError.errors()[:3]` as Python
/// prints the list of error dictionaries.
#[must_use]
pub fn schema_error_text(errors: &[SchemaError]) -> String {
    let shown: Vec<String> = errors.iter().take(3).map(SchemaError::repr).collect();
    format!("[{}]", shown.join(", "))
}

/// `str(ValidationError)` of the model `title` (pydantic 2.14 with pydantic-core 2.50): the error
/// count and title, then each error as [`SchemaError::display`] lists it, one after another.
#[must_use]
pub fn validation_error_text(title: &str, errors: &[SchemaError]) -> String {
    let plural = if errors.len() == 1 { "" } else { "s" };
    std::iter::once(format!(
        "{} validation error{plural} for {title}",
        errors.len()
    ))
    .chain(errors.iter().map(SchemaError::display))
    .collect::<Vec<String>>()
    .join("\n")
}

/// The name of the Python type pydantic reports as `input_type` for a JSON value.
fn py_type_name(value: &Value) -> &'static str {
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

/// The input as pydantic shows it in `input_value`: its `repr`, cut to 50 characters with an
/// ellipsis when it is longer.
fn shown_input(value: &Value) -> String {
    let text = py_repr(value);
    if text.chars().count() > INPUT_SHOWN_CHARS {
        let head: String = prefix(&text, INPUT_SHOWN_CHARS);
        format!("{head}...")
    } else {
        text
    }
}

/// Validates a model's output against `TaskClassification` (pydantic semantics: lax float
/// coercion, `extra = "forbid"`, field order in the error list). Returns the dumped model, with
/// defaults filled in and the list fields cleaned, or the errors.
pub fn validate_classification_errors(payload: &Value) -> Result<Value, Vec<SchemaError>> {
    let Some(object) = payload.as_object() else {
        return Err(vec![SchemaError::new(
            "model_type",
            Vec::new(),
            MODEL_TYPE,
            payload,
            Vec::new(),
        )]);
    };
    let fields: Vec<(&str, Result<Value, Vec<SchemaError>>)> = vec![
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
        .iter()
        .filter(|(key, _)| !known.contains(&key.as_str()))
        .map(|(key, value)| {
            SchemaError::new(
                "extra_forbidden",
                vec![Value::from(key.as_str())],
                EXTRA_FORBIDDEN,
                value,
                Vec::new(),
            )
        });
    let errors: Vec<SchemaError> = fields
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

/// The `type:location` keys of [`validate_classification_errors`]'s errors.
pub fn validate_classification(payload: &Value) -> Result<Value, Vec<String>> {
    validate_classification_errors(payload)
        .map_err(|errors| errors.iter().map(SchemaError::key).collect())
}

/// `missing`: the field is absent. Pydantic's input is the whole model input.
fn missing(object: &Map<String, Value>, name: &str) -> SchemaError {
    SchemaError::new(
        "missing",
        vec![Value::from(name)],
        FIELD_REQUIRED,
        &Value::Object(object.clone()),
        Vec::new(),
    )
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// The `expected` of a literal error: the values as `repr`s, joined by `, ` with ` or ` before the
/// last one.
fn expected_literals(allowed: &[&str]) -> String {
    let quoted: Vec<String> = allowed
        .iter()
        .map(|item| py_repr(&Value::from(*item)))
        .collect();
    match quoted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

fn literal(
    object: &Map<String, Value>,
    name: &str,
    allowed: &[&str],
) -> Result<Value, Vec<SchemaError>> {
    match object.get(name) {
        None => Err(vec![missing(object, name)]),
        Some(Value::String(value)) if allowed.contains(&value.as_str()) => {
            Ok(Value::String(value.clone()))
        }
        Some(value) => {
            let expected = expected_literals(allowed);
            Err(vec![SchemaError::new(
                "literal_error",
                vec![Value::from(name)],
                &format!("Input should be {expected}"),
                value,
                vec![("expected", Value::from(expected.as_str()))],
            )])
        }
    }
}

fn text(
    object: &Map<String, Value>,
    name: &str,
    max_chars: usize,
    default: Option<&str>,
) -> Result<Value, Vec<SchemaError>> {
    match (object.get(name), default) {
        (None, Some(default)) => Ok(Value::String(default.to_string())),
        (None, None) => Err(vec![missing(object, name)]),
        (Some(Value::String(value)), _) if value.chars().count() > max_chars => {
            Err(vec![SchemaError::new(
                "string_too_long",
                vec![Value::from(name)],
                &format!("String should have at most {max_chars} characters"),
                &Value::String(value.clone()),
                vec![("max_length", Value::from(max_chars))],
            )])
        }
        (Some(Value::String(value)), _) => Ok(Value::String(value.clone())),
        (Some(other), _) => Err(vec![SchemaError::new(
            "string_type",
            vec![Value::from(name)],
            STRING_TYPE,
            other,
            Vec::new(),
        )]),
    }
}

/// `list[str]` with `max_length=10`, then the `_clean` field validator: each item is stripped,
/// empty items are dropped, items are cut to 100 characters, and duplicates are removed.
fn string_list(object: &Map<String, Value>, name: &str) -> Result<Value, Vec<SchemaError>> {
    let items = match object.get(name) {
        None => return Ok(Value::Array(Vec::new())),
        Some(Value::Array(items)) => items,
        Some(other) => {
            return Err(vec![SchemaError::new(
                "list_type",
                vec![Value::from(name)],
                LIST_TYPE,
                other,
                Vec::new(),
            )]);
        }
    };
    if items.len() > LIST_MAX_ITEMS {
        return Err(vec![SchemaError::new(
            "too_long",
            vec![Value::from(name)],
            &format!(
                "List should have at most {LIST_MAX_ITEMS} item{} after validation, not {}",
                plural(LIST_MAX_ITEMS),
                items.len()
            ),
            &Value::Array(items.clone()),
            vec![
                ("field_type", Value::from("List")),
                ("max_length", Value::from(LIST_MAX_ITEMS)),
                ("actual_length", Value::from(items.len())),
            ],
        )]);
    }
    let errors: Vec<SchemaError> = items
        .iter()
        .enumerate()
        .filter(|(_, item)| !item.is_string())
        .map(|(index, item)| {
            SchemaError::new(
                "string_type",
                vec![Value::from(name), Value::from(index)],
                STRING_TYPE,
                item,
                Vec::new(),
            )
        })
        .collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    let cleaned = items
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| prefix(item, TEXT_MAX_CHARS));
    Ok(Value::Array(
        unique(cleaned).into_iter().map(Value::String).collect(),
    ))
}

/// `float` with `ge=0`, `le=1` in lax mode: numbers, booleans and numeric strings are accepted.
/// A NaN fails the upper bound first, as pydantic reports it.
fn confidence(object: &Map<String, Value>) -> Result<Value, Vec<SchemaError>> {
    let input = match object.get("confidence") {
        None => return Err(vec![missing(object, "confidence")]),
        Some(input) => input,
    };
    let location = || vec![Value::from("confidence")];
    let value = match input {
        Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Value::Bool(flag) => f64::from(u8::from(*flag)),
        Value::String(text) => text.trim().parse::<f64>().map_err(|_| {
            vec![SchemaError::new(
                "float_parsing",
                location(),
                FLOAT_PARSING,
                input,
                Vec::new(),
            )]
        })?,
        _ => {
            return Err(vec![SchemaError::new(
                "float_type",
                location(),
                FLOAT_TYPE,
                input,
                Vec::new(),
            )]);
        }
    };
    if value.is_nan() || value > 1.0 {
        Err(vec![SchemaError::new(
            "less_than_equal",
            location(),
            "Input should be less than or equal to 1",
            input,
            vec![("le", Value::from(1.0))],
        )])
    } else if value < 0.0 {
        Err(vec![SchemaError::new(
            "greater_than_equal",
            location(),
            "Input should be greater than or equal to 0",
            input,
            vec![("ge", Value::from(0.0))],
        )])
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
    fn schema_errors_print_as_pydantic_error_dicts() {
        let payload = json!({"category": "nope", "complexity": "small", "confidence": 2,
            "subtype": "x", "domain": "x"});
        let errors = validate_classification_errors(&payload)
            .err()
            .unwrap_or_default();
        let text = schema_error_text(&errors);
        assert!(text.starts_with(
            "[{'type': 'literal_error', 'loc': ('category',), 'msg': \"Input should be 'bug_fix', 'feature',"
        ));
        assert!(text.contains(", 'input': 'nope', 'ctx': {'expected': \"'bug_fix', "));
        assert!(text.contains(
            ", {'type': 'less_than_equal', 'loc': ('confidence',), 'msg': 'Input should be less than or equal to 1', 'input': 2, 'ctx': {'le': 1.0}, 'url': 'https://errors.pydantic.dev/2.14/v/less_than_equal'}]"
        ));
    }

    #[test]
    fn missing_fields_take_the_whole_input_and_lists_show_their_index() {
        let payload = json!({"complexity": "small", "confidence": 0.3, "subtype": "x",
            "domain": "x", "tags": ["ok", 7]});
        let errors = validate_classification_errors(&payload)
            .err()
            .unwrap_or_default();
        let text = schema_error_text(&errors);
        assert!(text.starts_with(
            "[{'type': 'missing', 'loc': ('category',), 'msg': 'Field required', 'input': {'complexity': 'small'"
        ));
        assert!(text.contains("{'type': 'string_type', 'loc': ('tags', 1), 'msg': 'Input should be a valid string', 'input': 7, 'url': 'https://errors.pydantic.dev/2.14/v/string_type'}"));
    }

    #[test]
    fn validation_error_text_matches_pydantic_str() {
        let payload = json!({"category": "nope", "complexity": "small", "confidence": 2,
            "subtype": "x", "domain": "x"});
        let errors = validate_classification_errors(&payload)
            .err()
            .unwrap_or_default();
        let text = validation_error_text("TaskClassification", &errors);
        assert!(text.starts_with(
            "2 validation errors for TaskClassification\ncategory\n  Input should be 'bug_fix', 'feature', "
        ));
        assert!(text.contains(
            " [type=literal_error, input_value='nope', input_type=str]\n    For further information visit https://errors.pydantic.dev/2.14/v/literal_error\nconfidence\n  Input should be less than or equal to 1 [type=less_than_equal, input_value=2, input_type=int]\n    For further information visit https://errors.pydantic.dev/2.14/v/less_than_equal"
        ));
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
