//! Shell: `generate_retrospective` (`analytics/retrospective.py`). The measured part is the pure
//! [`wg_analytics::core::retrospective`]; this module reads the warehouse, asks the summarization
//! model for the narrative (or falls back to the deterministic report), stores the report with its
//! `report.generated` event and writes the `--output` file. The narrative's prompt, schema and
//! validation are the pure [`crate::spec::narrative`].

use std::path::Path;

use serde_json::{Value as Json, json};
use time::Duration;
use wg_analytics::build;
use wg_analytics::core::retrospective::{Request, Retrospective, retrospective as measured};
use wg_intel_feature::service::llm::{Live, Providers};
use wg_intel_feature::spec::llm::selection;
use wg_json::Value;
use wg_model::entity::{JsonObject, Report};
use wg_model::event::{EventDraft, EventPayload, ReportGenerated};
use wg_model::{Decision, Row, Write};
use wg_runtime::{App, RequestContext};
use wg_types::{Id, Timestamp, WgError};
use wg_wire::convention::json_value;

use super::{load, require};
use crate::spec::analyses::fingerprint;
use crate::spec::args::RetrospectiveOptions;
use crate::spec::narrative::{
    NARRATIVE_SYSTEM, facts_for_model, narrative_of, narrative_schema, narrative_text,
};

/// The deterministic narrative source, when no model narrative was written.
const DETERMINISTIC: &str = "deterministic";

/// The narrative of one retrospective: where it came from and the text appended to the report.
struct Narrative {
    source: String,
    text: String,
}

/// `generate_retrospective`: the report of the last `period_days` days, `project` and `repository`
/// narrowing it. The report is stored and, with `--output`, written to that file.
pub fn retrospective(
    app: &mut App,
    context: &RequestContext,
    workspace: Option<&str>,
    options: &RetrospectiveOptions,
) -> Result<Value, WgError> {
    let (period_days, project, repository, use_model, output) = options;
    let until = require(app, context)?;
    let since = Timestamp::from_offset_datetime(
        until
            .to_offset_datetime()
            .checked_sub(Duration::days(*period_days))
            .ok_or_else(|| WgError::validation("the period reaches before the calendar's start"))?,
    )?;
    let loaded = load(app, workspace, project.as_deref(), repository.as_deref())?;
    let input = loaded.input;
    let facts = build(&input);
    let generated_at = until.format_isoformat();
    let request = Request {
        window: (since, until),
        project: project.as_deref(),
        repository: repository.as_deref(),
        project_id: loaded.project.as_deref(),
        repository_id: loaded.repository.as_deref(),
        generated_at: &generated_at,
    };
    let retro = measured(
        &input,
        &facts,
        &loaded.workspace.to_string(),
        &request,
        &fingerprint,
    )
    .map_err(|err| crate::spec::query::refusal(&err))?;
    let narrative = if *use_model {
        model_narrative(app, loaded.workspace, &retro)
    } else {
        Narrative {
            source: DETERMINISTIC.to_owned(),
            text: String::new(),
        }
    };
    let content = if narrative.text.is_empty() {
        retro.content.clone()
    } else {
        format!("{}\n{}", retro.content, narrative.text)
    };
    let stamp = app.runtime.stamps.stamp(context)?;
    let report_id = stamp.id(0);
    let event_id = stamp.id(1);
    let facts_object: JsonObject = retro.facts.as_object().cloned().unwrap_or_default();
    let scope: JsonObject = facts_object
        .get("period")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let report = Report {
        id: report_id,
        workspace_id: loaded.workspace,
        kind: "retrospective".to_owned(),
        title: retro.title.clone(),
        scope,
        content_markdown: content.clone(),
        data: facts_object,
        narrative_source: narrative.source.clone(),
        created_by: stamp.actor.to_string(),
        created_at: until,
    };
    let event = EventDraft::new(
        &stamp,
        event_id,
        Some(loaded.workspace),
        report_id,
        EventPayload::ReportGenerated(ReportGenerated {
            kind: "retrospective".to_owned(),
            narrative_source: narrative.source.clone(),
        }),
    );
    let decision = Decision::new(report_id)
        .write(Write::insert(Row::from(report)))
        .event(event);
    app.runtime
        .store
        .write(|session| session.apply_decision(&decision))?;
    if let Some(path) = output {
        write_output(path, &content)?;
    }
    Ok(json_value(&json!({
        "id": report_id.to_string(),
        "title": retro.title,
        "narrative_source": narrative.source,
        "content": content,
        "facts": retro.facts,
    })))
}

/// The model's narrative of the facts. A model that is unavailable or answers with an invalid
/// document leaves the deterministic report, with the reason it was skipped.
fn model_narrative(app: &mut App, workspace: Id, retro: &Retrospective) -> Narrative {
    match write_narrative(app, workspace, retro) {
        Ok(narrative) => narrative,
        Err(reason) => Narrative {
            source: DETERMINISTIC.to_owned(),
            text: format!(
                "\n_Model narrative unavailable ({reason}); deterministic report only._\n"
            ),
        },
    }
}

/// Asks the summarization provider for the narrative. The error is the Python exception class the
/// fallback names.
fn write_narrative(
    app: &mut App,
    workspace: Id,
    retro: &Retrospective,
) -> Result<Narrative, &'static str> {
    let slug = app
        .workspace_slug(&workspace.to_string())
        .map_err(|_| "WorkgraphError")?;
    let config = app
        .effective_config(Some(&slug))
        .map_err(|_| "WorkgraphError")?;
    let (provider_name, model) =
        selection(&config, "summarization").map_err(|_| "WorkgraphError")?;
    let mut providers = Live;
    let mut llm = providers
        .provider(&provider_name, &config)
        .map_err(|_| "WorkgraphError")?;
    let prompt = format!("<facts>{}</facts>", facts_for_model(retro));
    let generation = llm
        .generate_json(&model, NARRATIVE_SYSTEM, &prompt, &narrative_schema())
        .map_err(|_| "WorkgraphError")?;
    let (summary, observations) = narrative_of(&generation.data).ok_or("ValidationError")?;
    Ok(Narrative {
        source: format!("model:{provider_name}/{model}"),
        text: narrative_text(&provider_name, &model, summary, &observations),
    })
}

/// Writes the content of a generated report to `path` (`--output`).
pub fn write_output(path: &Path, content: &str) -> Result<(), WgError> {
    std::fs::write(path, content.as_bytes())
        .map_err(|err| WgError::workgraph(format!("cannot write {}: {err}", path.display())))
}
