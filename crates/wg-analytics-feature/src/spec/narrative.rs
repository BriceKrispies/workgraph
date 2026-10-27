//! Pure: the model narrative of `analytics/retrospective.py`. The prompt's facts document, the
//! `Narrative` schema the provider is asked for, the validation of its answer and the text the
//! report appends. The call to the provider is the shell's (`service::retrospective`).

use serde_json::{Map, Value as Json, json};
use wg_analytics::core::retrospective::Retrospective;

/// The system prompt of the narrative (`NARRATIVE_SYSTEM`).
pub const NARRATIVE_SYSTEM: &str = "You write concise engineering retrospectives. Use ONLY the facts in the <facts> block; do not invent numbers. Cite task keys exactly as given. Describe associations, never causation.";

/// The longest summary the narrative accepts (`Narrative.summary`, `max_length=4000`).
const SUMMARY_CHARS: usize = 4000;

/// The most observations and opportunities the narrative accepts.
const OBSERVATIONS: usize = 12;
const OPPORTUNITIES: usize = 8;

/// The facts document the narrative is written from: the report's facts with the highlights, a
/// one-line summary and the opportunities (`facts_for_model`), as the prompt's JSON text.
#[must_use]
pub fn facts_for_model(retro: &Retrospective) -> String {
    let mut object: Map<String, Json> = retro.facts.as_object().cloned().unwrap_or_default();
    let accepted = retro
        .facts
        .get("tasks")
        .and_then(|tasks| tasks.get("accepted"))
        .map_or_else(|| "None".to_owned(), Json::to_string);
    object.insert("highlights".to_owned(), json!(retro.highlights));
    object.insert(
        "summary".to_owned(),
        json!(format!("{accepted} tasks accepted")),
    );
    object.insert("opportunities".to_owned(), json!(retro.opportunities));
    serde_json::to_string(&Json::Object(object)).unwrap_or_default()
}

/// The JSON schema of `Narrative` (what `Narrative.model_json_schema()` writes for it).
#[must_use]
pub fn narrative_schema() -> Json {
    json!({
        "properties": {
            "summary": {"maxLength": SUMMARY_CHARS, "title": "Summary", "type": "string"},
            "observations": {
                "default": [],
                "items": {"type": "string"},
                "maxItems": OBSERVATIONS,
                "title": "Observations",
                "type": "array",
            },
            "opportunities": {
                "default": [],
                "items": {"type": "string"},
                "maxItems": OPPORTUNITIES,
                "title": "Opportunities",
                "type": "array",
            },
        },
        "required": ["summary"],
        "title": "Narrative",
        "type": "object",
        "additionalProperties": false,
    })
}

/// `Narrative.model_validate`: the summary and the observations of a document that has no other
/// keys and keeps within the limits; `None` when it does not validate.
#[must_use]
pub fn narrative_of(data: &Map<String, Json>) -> Option<(String, Vec<String>)> {
    if data
        .keys()
        .any(|key| !matches!(key.as_str(), "summary" | "observations" | "opportunities"))
    {
        return None;
    }
    let summary = data.get("summary")?.as_str()?;
    if summary.chars().count() > SUMMARY_CHARS {
        return None;
    }
    let observations = strings(data.get("observations"), OBSERVATIONS)?;
    strings(data.get("opportunities"), OPPORTUNITIES)?;
    Some((summary.to_owned(), observations))
}

/// A list of strings of at most `limit` items; a missing list is empty.
fn strings(value: Option<&Json>, limit: usize) -> Option<Vec<String>> {
    match value {
        None => Some(Vec::new()),
        Some(Json::Array(items)) if items.len() <= limit => items
            .iter()
            .map(|item| item.as_str().map(str::to_owned))
            .collect(),
        _ => None,
    }
}

/// The section the report appends for a model narrative: its heading, the model it came from, the
/// summary and one bullet per observation.
#[must_use]
pub fn narrative_text(
    provider: &str,
    model: &str,
    summary: String,
    observations: &[String],
) -> String {
    let mut lines = vec![
        "## Narrative (model-generated interpretation)".to_owned(),
        String::new(),
        format!("_Written by {provider}/{model} from the measured facts above._"),
        String::new(),
        summary,
        String::new(),
    ];
    lines.extend(
        observations
            .iter()
            .map(|observation| format!("- {observation}")),
    );
    format!("{}\n", lines.join("\n"))
}
