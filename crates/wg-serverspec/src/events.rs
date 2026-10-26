//! The frames of the event stream (O08, `api/routes/ops.py` `event_stream`): each new domain event
//! as an `event: domain` frame carrying its `EventOut` JSON, and the queue snapshot as an
//! `event: queue` frame carrying the job counts by status.

use std::collections::BTreeMap;

use wg_json::{http_json, log_json};
use wg_model::StoredEvent;
use wg_types::{ErrorCode, WgError};
use wg_wire::convention::counts;
use wg_wire::dto::event::from_stored;

use crate::sse::SseEvent;

fn dump_error(err: wg_json::DumpError) -> WgError {
    WgError::new(ErrorCode::WorkgraphError, err.to_string())
}

/// One domain event: `ServerSentEvent(data=model_dump_json(), event="domain", id=str(seq))`.
pub fn domain_frame(event: &StoredEvent) -> Result<SseEvent, WgError> {
    Ok(SseEvent {
        event: Some("domain".to_owned()),
        id: Some(event.seq.to_string()),
        data: Some(http_json(&from_stored(event)).map_err(dump_error)?),
    })
}

/// The domain frames of a batch, in the batch's order.
pub fn batch_frames(events: &[StoredEvent]) -> Result<Vec<SseEvent>, WgError> {
    events.iter().map(domain_frame).collect()
}

/// The queue snapshot: `ServerSentEvent(data=json.dumps(stats), event="queue")`, with the counts
/// in the store's status order.
pub fn queue_frame(stats: &BTreeMap<String, usize>) -> Result<SseEvent, WgError> {
    let pairs: Vec<(String, i64)> = stats
        .iter()
        .map(|(status, count)| (status.clone(), i64::try_from(*count).unwrap_or(i64::MAX)))
        .collect();
    Ok(SseEvent {
        event: Some("queue".to_owned()),
        id: None,
        data: Some(log_json(&counts(&pairs)).map_err(dump_error)?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sse::encode;
    use wg_model::event::StoredEvent;
    use wg_types::{Timestamp, parse_id};

    fn stored(seq: i64) -> Result<StoredEvent, WgError> {
        let id = parse_id("0190a1b2-0000-7000-8000-0000000000aa")?;
        Ok(StoredEvent {
            seq,
            id,
            workspace_id: Some(id),
            event_type: "workspace.created".to_owned(),
            entity_type: "workspace".to_owned(),
            entity_id: id,
            actor: "human:tester".to_owned(),
            actor_kind: "human".to_owned(),
            occurred_at: Timestamp::parse_iso("2026-10-25T13:56:00.123456+00:00")?,
            correlation_id: id.into(),
            causation_id: None,
            attempt_id: None,
            schema_version: 1,
            data: Default::default(),
        })
    }

    #[test]
    fn a_domain_frame_carries_the_seq_and_the_event_out_json() -> Result<(), WgError> {
        let frame = domain_frame(&stored(3)?)?;
        assert_eq!(frame.event.as_deref(), Some("domain"));
        assert_eq!(frame.id.as_deref(), Some("3"));
        let data = frame.data.clone().unwrap_or_default();
        assert!(data.starts_with("{\"seq\":3,"));
        assert!(data.contains("\"occurred_at\":\"2026-10-25T13:56:00.123456Z\""));
        assert!(encode(&frame).starts_with("id: 3\r\nevent: domain\r\ndata: {"));
        Ok(())
    }

    #[test]
    fn the_queue_snapshot_is_python_json_dumps_of_the_counts() -> Result<(), WgError> {
        let mut stats = BTreeMap::new();
        assert_eq!(queue_frame(&stats)?.data.as_deref(), Some("{}"));
        stats.insert("queued".to_owned(), 2);
        stats.insert("succeeded".to_owned(), 5);
        let frame = queue_frame(&stats)?;
        assert_eq!(frame.event.as_deref(), Some("queue"));
        assert_eq!(frame.id, None);
        assert_eq!(
            frame.data.as_deref(),
            Some("{\"queued\": 2, \"succeeded\": 5}")
        );
        Ok(())
    }
}
