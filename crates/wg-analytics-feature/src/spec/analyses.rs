//! Pure: the decisions of `analytics overview, compare, coupling, failures, history`
//! (`analytics/service.py`, `coupling.py`, `failures.py`): the compare document's windows as the
//! naive datetimes the CLI passed, the failure fingerprint and the rows `failures --persist`
//! writes.

use serde_json::Value as Json;
use wg_hash::sha256_hex;
use wg_json::{LocalDateTime, Value};
use wg_model::entity::{Finding, JsonObject, Real};
use wg_types::pyval::{prefix, text};
use wg_types::{Id, Timestamp, WgError};
use wg_wire::convention::json_value;

/// A `[since, until)` window as the CLI typed it (typer's naive `datetime`).
pub type Typed = (LocalDateTime, LocalDateTime);

/// A window as Python's `[x.isoformat() for x in a]` writes it: the datetimes are naive, so the
/// text has no offset.
fn window(typed: Typed) -> Value {
    Value::List(vec![
        Value::LocalDateTime(typed.0),
        Value::LocalDateTime(typed.1),
    ])
}

/// The `compare` document: the engine's result with `period_a` and `period_b` written from the
/// typed datetimes.
#[must_use]
pub fn compare_out(core: &Json, a: Typed, b: Typed) -> Value {
    match json_value(core) {
        Value::Dict(entries) => Value::Dict(
            entries
                .into_iter()
                .map(|(key, value)| {
                    let name: String = key
                        .code_points()
                        .iter()
                        .filter_map(|code| char::from_u32(*code))
                        .collect();
                    match name.as_str() {
                        "period_a" => (key, window(a)),
                        "period_b" => (key, window(b)),
                        _ => (key, value),
                    }
                })
                .collect(),
        ),
        other => other,
    }
}

/// `hashlib.sha256(f"{kind}|{title}".encode()).hexdigest()[:32]`: the engine hands over the
/// `kind|title` text.
#[must_use]
pub fn fingerprint(text: &str) -> String {
    prefix(&sha256_hex(text.as_bytes()), 32)
}

fn text_of(finding: &Json, key: &str) -> String {
    text(finding.get(key)).to_owned()
}

/// The finding row `persist_findings` writes for one entry of the report: a new row, or the
/// stored one with its title, severity, value, sample size and evidence replaced and its
/// `last_detected_at` moved to `now`.
pub fn finding_row(
    existing: Option<Finding>,
    id: Id,
    workspace_id: Id,
    report: &Json,
    now: Timestamp,
) -> Result<Finding, WgError> {
    let metric_value = report.get("metric_value").and_then(Json::as_f64).map(Real);
    let sample_size = report
        .get("sample_size")
        .and_then(Json::as_i64)
        .ok_or_else(|| WgError::workgraph("finding without a sample size"))?;
    let evidence: JsonObject = report
        .get("evidence")
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    Ok(match existing {
        Some(stored) => Finding {
            title: text_of(report, "title"),
            severity: text_of(report, "severity"),
            metric_value,
            sample_size,
            evidence,
            last_detected_at: now,
            ..stored
        },
        None => Finding {
            id,
            workspace_id,
            kind: text_of(report, "kind"),
            fingerprint: text_of(report, "fingerprint"),
            title: text_of(report, "title"),
            severity: text_of(report, "severity"),
            metric_value,
            sample_size,
            evidence,
            first_detected_at: now,
            last_detected_at: now,
        },
    })
}
