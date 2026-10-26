//! The failure carried through a store transaction: a hook ingestion's `Failure` travels as a
//! `WgError` with its type name in the details, and comes back out unchanged.

use serde_json::Value;
use wg_telemetry::ingest::Failure;
use wg_types::{ErrorCode, WgError};

/// A failure carried through a transaction as a `WgError`: the type name rides in the details.
pub fn failure_error(failure: &Failure) -> WgError {
    WgError::new(ErrorCode::WorkgraphError, failure.message.clone())
        .with_detail("failure_type", Value::String(failure.type_name.clone()))
}

/// The failure a transaction's error stands for.
pub fn restore_failure(error: WgError) -> Failure {
    match error.details.get("failure_type").and_then(Value::as_str) {
        Some(type_name) => Failure::new(type_name, error.message),
        None => Failure::from(error),
    }
}
