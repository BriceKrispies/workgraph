//! The result of `GET /analytics/failures` (`detect_failures`) and its findings.

use wg_json::{Value, dict};
use wg_types::Timestamp;

use crate::convention::{TimeFormat, int, optional_float, text, timestamp};

/// The name of the report dict; it has no OpenAPI component schema.
pub const NAME: &str = "FailuresOut";

/// The fields of the report, in order.
pub const FIELDS: [&str; 3] = ["generated_at", "findings", "note"];

/// The fields of one finding, in the order `_finding` builds them.
pub const FINDING_FIELDS: [&str; 7] = [
    "kind",
    "title",
    "severity",
    "metric_value",
    "sample_size",
    "evidence",
    "fingerprint",
];

/// One finding. `evidence` is the detector's evidence dict, passed in as a value.
pub fn finding(
    kind: &str,
    title: &str,
    severity: &str,
    metric_value: Option<f64>,
    sample_size: i64,
    evidence: Value,
    fingerprint: &str,
) -> Value {
    dict!({
        "kind": text(kind),
        "title": text(title),
        "severity": text(severity),
        "metric_value": optional_float(metric_value),
        "sample_size": int(sample_size),
        "evidence": evidence,
        "fingerprint": text(fingerprint),
    })
}

/// The report. `generated_at` is `isoformat()` of an aware UTC datetime.
pub fn from_parts(generated_at: Timestamp, findings: Vec<Value>, note: &str) -> Value {
    dict!({
        "generated_at": timestamp(generated_at, TimeFormat::Isoformat),
        "findings": Value::List(findings),
        "note": text(note),
    })
}
