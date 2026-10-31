//! The pure decisions of the classification service (`classification/service.py`): the output
//! schema, the rows a model run and a human confirmation write, the current-classification policy,
//! the promotion, the gold label and the evaluation rows.
//!
//! The shell stamps ids and times, loads the rows and performs the writes; this module returns the
//! rows and the event payloads as values.

use rpds::VectorSync;
use serde_json::{Value, json};
use wg_model::entity::{
    Classification, ClassificationEvaluation, ClassificationGoldLabel, JsonObject, Real, Task,
};
use wg_types::{Id, Timestamp};

use crate::evaluation::compute_metrics;
use crate::taxonomy::{CATEGORIES, COMPLEXITY, PROMPT_VERSION, TAXONOMY_VERSION, input_hash};
use wg_types::pyval::{object, prefix};

/// The longest stored error text of a failed classification (`error[:5000]`).
const ERROR_MAX_CHARS: usize = 5000;

/// The longest error text an evaluation result keeps (`str(exc)[:300]`).
const RESULT_ERROR_MAX_CHARS: usize = 300;

/// The JSON Schema of the classifier output (`output_schema()`): the schema of `TaskClassification`,
/// titled so that providers and the heuristic know which output is asked for.
#[must_use]
pub fn output_schema() -> Value {
    json!({
        "additionalProperties": false,
        "properties": {
            "category": {"enum": CATEGORIES, "title": "Category", "type": "string"},
            "subtype": {"maxLength": 100, "title": "Subtype", "type": "string"},
            "domain": {"maxLength": 100, "title": "Domain", "type": "string"},
            "affected_systems": {
                "default": [],
                "items": {"type": "string"},
                "maxItems": 10,
                "title": "Affected Systems",
                "type": "array"
            },
            "complexity": {"enum": COMPLEXITY, "title": "Complexity", "type": "string"},
            "tags": {
                "default": [],
                "items": {"type": "string"},
                "maxItems": 10,
                "title": "Tags",
                "type": "array"
            },
            "confidence": {"maximum": 1.0, "minimum": 0.0, "title": "Confidence", "type": "number"},
            "rationale": {"default": "", "maxLength": 2000, "title": "Rationale", "type": "string"}
        },
        "required": ["category", "subtype", "domain", "complexity", "confidence"],
        "title": "TaskClassification",
        "type": "object"
    })
}

/// The task text a classification reads: title, description and acceptance criteria.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskText {
    pub title: String,
    pub description: String,
    pub criteria: Vec<String>,
}

impl TaskText {
    /// The text of a task row.
    #[must_use]
    pub fn of(task: &Task) -> Self {
        TaskText {
            title: task.title.clone(),
            description: task.description.clone(),
            criteria: task.acceptance_criteria.iter().cloned().collect(),
        }
    }

    /// `input_hash(title, description, criteria)`.
    #[must_use]
    pub fn digest(&self) -> String {
        input_hash(&self.title, &self.description, &self.criteria)
    }
}

/// The identity and the shared columns of a `classifications` row written by a model run
/// (`row_kwargs` in `run_classification`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRow {
    pub id: Id,
    pub task_id: Id,
    pub provider: String,
    pub model: String,
    pub input_hash: String,
    pub batch_id: Option<Id>,
    pub created_by: String,
    pub created_at: Timestamp,
}

/// How a model run ended: the validated output, or the failure with its validation status.
#[derive(Debug, Clone, PartialEq)]
pub enum RunStatus<'a> {
    Succeeded {
        /// The validated output (`validate_classification`).
        parsed: &'a Value,
        /// The provider's JSON object, as stored.
        raw: &'a JsonObject,
        duration_ms: i64,
    },
    Failed {
        validation: &'static str,
        error: &'a str,
        raw: Option<&'a JsonObject>,
        duration_ms: Option<i64>,
    },
}

/// The classification row of a model run: its validated output, or the failure that ended it.
#[must_use]
pub fn model_row(run: &RunRow, status: RunStatus<'_>) -> Classification {
    let base = Classification {
        id: run.id,
        task_id: run.task_id,
        source: "model".to_owned(),
        status: "failed".to_owned(),
        provider: Some(run.provider.clone()),
        model: Some(run.model.clone()),
        prompt_version: Some(PROMPT_VERSION.to_owned()),
        taxonomy_version: TAXONOMY_VERSION.to_owned(),
        input_hash: run.input_hash.clone(),
        category: None,
        subtype: None,
        domain: None,
        affected_systems: VectorSync::new_sync(),
        complexity: None,
        tags: VectorSync::new_sync(),
        confidence: None,
        rationale: None,
        raw_output: None,
        validation_status: String::new(),
        error: None,
        duration_ms: None,
        is_current: false,
        confirmed_by: None,
        batch_id: run.batch_id,
        created_by: run.created_by.clone(),
        created_at: run.created_at,
    };
    match status {
        RunStatus::Succeeded {
            parsed,
            raw,
            duration_ms,
        } => Classification {
            status: "succeeded".to_owned(),
            category: text_field(parsed, "category"),
            subtype: text_field(parsed, "subtype"),
            domain: text_field(parsed, "domain"),
            affected_systems: list_field(parsed, "affected_systems"),
            complexity: text_field(parsed, "complexity"),
            tags: list_field(parsed, "tags"),
            confidence: parsed.get("confidence").and_then(Value::as_f64).map(Real),
            rationale: text_field(parsed, "rationale"),
            raw_output: Some(raw.clone()),
            validation_status: "valid".to_owned(),
            duration_ms: Some(duration_ms),
            ..base
        },
        RunStatus::Failed {
            validation,
            error,
            raw,
            duration_ms,
        } => Classification {
            validation_status: validation.to_owned(),
            error: Some(prefix(error, ERROR_MAX_CHARS)),
            raw_output: raw.cloned(),
            duration_ms,
            ..base
        },
    }
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn list_field(value: &Value, key: &str) -> VectorSync<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<String>>()
        })
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// The data of a `classification.superseded` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Superseded {
    pub previous_id: Option<Id>,
    pub new_id: Id,
    pub reason: &'static str,
}

/// The current-classification policy of a new row (`_apply_current_policy`), with the rows and
/// event payloads it implies.
#[derive(Debug, Clone, PartialEq)]
pub struct CurrentPlan {
    /// The new row, with `is_current` set by the policy.
    pub row: Classification,
    /// The former current row, with `is_current` cleared, when the new row replaces it.
    pub previous: Option<Classification>,
    /// `classification.disagreement`: the payload, when the categories differ.
    pub disagreement: Option<JsonObject>,
    /// `classification.superseded`, when the new row replaces a current row.
    pub superseded: Option<Superseded>,
    /// The task with its current classification and category, when the new row replaces one.
    pub task: Option<Task>,
    pub replaced: bool,
    pub reason: &'static str,
}

/// The policy for a new row against the current classification `current` (`None` when the task
/// has none). A manual classification is never replaced; a new model row replaces a model row
/// only when it comes from the same provider and model, or when `promote` is set.
#[must_use]
pub fn apply_current(
    task: &Task,
    current: Option<&Classification>,
    row: Classification,
    promote: bool,
) -> CurrentPlan {
    let (replaced, reason) = policy(current, &row, promote);
    let disagreement = current
        .filter(|current| current.category != row.category)
        .map(|current| disagreement_payload(current, &row, replaced));
    let superseded = match (replaced, current) {
        (true, Some(current)) => Some(Superseded {
            previous_id: Some(current.id),
            new_id: row.id,
            reason,
        }),
        _ => None,
    };
    let previous = match (replaced, current) {
        (true, Some(current)) => Some(Classification {
            is_current: false,
            ..current.clone()
        }),
        _ => None,
    };
    let row = Classification {
        is_current: replaced,
        ..row
    };
    let task = replaced.then(|| with_current(task, &row));
    CurrentPlan {
        row,
        previous,
        disagreement,
        superseded,
        task,
        replaced,
        reason,
    }
}

/// `(replace, reason)` of the policy.
fn policy(
    current: Option<&Classification>,
    row: &Classification,
    promote: bool,
) -> (bool, &'static str) {
    match current {
        None => (true, "first classification"),
        Some(current) if current.source == "manual" || has_text(&current.confirmed_by) => {
            (false, "manual classification is authoritative")
        }
        Some(_) if promote => (true, "explicitly promoted"),
        Some(current) if current.provider == row.provider && current.model == row.model => {
            (true, "same model re-classified")
        }
        Some(_) => (
            false,
            "different model; stored as alternative (promote to make current)",
        ),
    }
}

/// Python's truthiness of an optional text: present and non-empty.
fn has_text(value: &Option<String>) -> bool {
    value.as_deref().is_some_and(|text| !text.is_empty())
}

/// The `classification.disagreement` payload: the current row, the new row and whether the new
/// one replaced the current one.
fn disagreement_payload(
    current: &Classification,
    row: &Classification,
    replaced: bool,
) -> JsonObject {
    json!({
        "current": {
            "id": current.id.to_string(),
            "category": current.category,
            "source": current.source,
            "model": current.model,
        },
        "new": {
            "id": row.id.to_string(),
            "category": row.category,
            "model": row.model,
        },
        "replaced": replaced,
    })
    .as_object()
    .cloned()
    .unwrap_or_default()
}

/// The task with `row` as its current classification: `current_classification_id`, `category` and
/// the `updated_at` the ORM stamps on the update.
#[must_use]
pub fn with_current(task: &Task, row: &Classification) -> Task {
    Task {
        current_classification_id: Some(row.id),
        category: row.category.clone(),
        // The ORM's `onupdate` stamps the task row the moment the classification is stored.
        updated_at: row.created_at,
        ..task.clone()
    }
}

/// The plan of `promote`: `row` becomes current and the former current row loses the flag.
#[derive(Debug, Clone, PartialEq)]
pub struct PromotePlan {
    pub row: Classification,
    pub previous: Option<Classification>,
    pub task: Task,
    /// The `previous_id` of `classification.promoted`.
    pub previous_id: Option<Id>,
}

/// `ClassificationService.promote` for a succeeded row.
#[must_use]
pub fn promote(task: &Task, current: Option<&Classification>, row: &Classification) -> PromotePlan {
    let previous = current
        .filter(|current| current.id != row.id)
        .map(|current| Classification {
            is_current: false,
            ..current.clone()
        });
    let row = Classification {
        is_current: true,
        ..row.clone()
    };
    let task = with_current(task, &row);
    PromotePlan {
        previous_id: current.map(|current| current.id),
        previous,
        row,
        task,
    }
}

/// The inputs of a human confirmation (`ClassificationService.confirm`).
#[derive(Debug, Clone, PartialEq)]
pub struct Confirmation<'a> {
    pub id: Id,
    pub task: &'a Task,
    pub current: Option<&'a Classification>,
    pub category: &'a str,
    pub subtype: Option<&'a str>,
    pub domain: Option<&'a str>,
    pub complexity: Option<&'a str>,
    pub tags: &'a [String],
    pub note: &'a str,
    pub actor: &'a str,
    pub created_at: Timestamp,
}

/// The confirmed row, the former current row with its flag cleared, and the task.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfirmPlan {
    pub row: Classification,
    pub previous: Option<Classification>,
    pub task: Task,
}

/// The rows of a confirmation. The subtype, domain and complexity keep the current row's values
/// when none is given; the affected systems always come from the current row.
#[must_use]
pub fn confirm(request: &Confirmation<'_>) -> ConfirmPlan {
    let current = request.current;
    let row = Classification {
        id: request.id,
        task_id: request.task.id,
        source: "manual".to_owned(),
        status: "succeeded".to_owned(),
        provider: None,
        model: None,
        prompt_version: None,
        taxonomy_version: TAXONOMY_VERSION.to_owned(),
        input_hash: TaskText::of(request.task).digest(),
        category: Some(request.category.to_owned()),
        subtype: request
            .subtype
            .map(str::to_owned)
            .or_else(|| current.and_then(|c| c.subtype.clone())),
        domain: request
            .domain
            .map(str::to_owned)
            .or_else(|| current.and_then(|c| c.domain.clone())),
        affected_systems: current
            .map(|c| c.affected_systems.clone())
            .unwrap_or_default(),
        complexity: request
            .complexity
            .map(str::to_owned)
            .or_else(|| current.and_then(|c| c.complexity.clone())),
        tags: request.tags.iter().cloned().collect(),
        confidence: Some(Real(1.0)),
        rationale: Some(if request.note.is_empty() {
            "confirmed by a human".to_owned()
        } else {
            request.note.to_owned()
        }),
        raw_output: None,
        validation_status: "manual".to_owned(),
        error: None,
        duration_ms: None,
        is_current: true,
        confirmed_by: Some(request.actor.to_owned()),
        batch_id: None,
        created_by: request.actor.to_owned(),
        created_at: request.created_at,
    };
    let previous = current.map(|c| Classification {
        is_current: false,
        ..c.clone()
    });
    let task = with_current(request.task, &row);
    ConfirmPlan {
        row,
        previous,
        task,
    }
}

/// The gold label an explicit `gold --category` request or a confirmation adds. With a task, the
/// title and description come from the task; without one they are the given text.
#[must_use]
pub fn gold_label(
    id: Id,
    workspace_id: Id,
    task: Option<&Task>,
    text: (&str, &str),
    category: &str,
    labeled_by: &str,
    created_at: Timestamp,
) -> ClassificationGoldLabel {
    ClassificationGoldLabel {
        id,
        workspace_id,
        task_id: task.map(|task| task.id),
        title: task.map_or_else(|| text.0.to_owned(), |task| task.title.clone()),
        description: task.map_or_else(|| text.1.to_owned(), |task| task.description.clone()),
        category: category.to_owned(),
        labeled_by: labeled_by.to_owned(),
        created_at,
    }
}

/// The per-label result of an evaluation (`results.append`): the prediction, or the error text.
#[must_use]
pub fn evaluation_result(
    label_id: Id,
    title: &str,
    expected: &str,
    predicted: Result<&str, &str>,
) -> JsonObject {
    let entry = match predicted {
        Ok(category) => json!({
            "label_id": label_id.to_string(),
            "title": title,
            "expected": expected,
            "predicted": category,
            "correct": category == expected,
        }),
        Err(error) => json!({
            "label_id": label_id.to_string(),
            "title": title,
            "expected": expected,
            "predicted": null,
            "correct": false,
            "error": prefix(error, RESULT_ERROR_MAX_CHARS),
        }),
    };
    entry.as_object().cloned().unwrap_or_default()
}

/// The evaluation row over `results` (`ClassificationEvaluation`), with the metrics it stores and
/// the metrics object itself (`compute_metrics`).
#[must_use]
pub fn evaluation_row(
    id: Id,
    workspace_id: Id,
    provider: &str,
    model: &str,
    results: &[JsonObject],
    created_at: Timestamp,
) -> (ClassificationEvaluation, Value) {
    let pairs: Vec<(String, Option<String>)> = results
        .iter()
        .filter_map(|result| {
            let expected = result.get("expected")?.as_str()?.to_owned();
            let predicted = result
                .get("predicted")
                .and_then(Value::as_str)
                .map(str::to_owned);
            Some((expected, predicted))
        })
        .collect();
    let metrics = compute_metrics(&pairs);
    let failures = results
        .iter()
        .filter(|result| result.contains_key("error"))
        .count();
    let row = ClassificationEvaluation {
        id,
        workspace_id,
        provider: provider.to_owned(),
        model: model.to_owned(),
        prompt_version: PROMPT_VERSION.to_owned(),
        taxonomy_version: TAXONOMY_VERSION.to_owned(),
        sample_size: i64::try_from(results.len()).unwrap_or(i64::MAX),
        failures: i64::try_from(failures).unwrap_or(i64::MAX),
        accuracy: metric_real(&metrics, "accuracy"),
        macro_f1: metric_real(&metrics, "macro_f1"),
        per_class: object(metrics.get("per_class")),
        confusion: object(metrics.get("confusion")),
        results: results.iter().cloned().collect(),
        created_at,
    };
    (row, metrics)
}

fn metric_real(metrics: &Value, key: &str) -> Option<Real> {
    metrics.get(key).and_then(Value::as_f64).map(Real)
}
