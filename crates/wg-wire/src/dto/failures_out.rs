//! The result of `GET /analytics/failures` (`detect_failures`) and its findings.

use wg_json::Value;
use wg_types::Timestamp;

use crate::convention::isoformat;
use crate::wire_dto;

wire_dto! {
    /// One finding, fields in the order `_finding` builds them. `evidence` is the detector's
    /// evidence dict, passed in as a value.
    pub struct Finding<'a>("Finding")
    dict finding(
        kind: &'a str,
        title: &'a str,
        severity: &'a str,
        metric_value: Option<f64>,
        sample_size: i64,
        evidence: Value,
        fingerprint: &'a str
    ) {
        kind: &'a str = kind,
        title: &'a str = title,
        severity: &'a str = severity,
        metric_value: Option<f64> = metric_value,
        sample_size: i64 = sample_size,
        evidence: Value = evidence,
        fingerprint: &'a str = fingerprint,
    }
}

wire_dto! {
    /// The report. `generated_at` is `isoformat()` of an aware UTC datetime.
    pub struct FailuresOut<'a>("FailuresOut")
    dict from_parts(generated_at: Timestamp, findings: Vec<Value>, note: &'a str) {
        #[serde(serialize_with = "isoformat")]
        generated_at: Timestamp = generated_at,
        findings: Vec<Value> = findings,
        note: &'a str = note,
    }
}
