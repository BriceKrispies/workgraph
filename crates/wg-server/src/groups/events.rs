//! The events group (plan IF-49): `GET /api/v1/ws/{ws}/events` (H123), the history, and `GET
//! /api/v1/ws/{ws}/events/stream` (H124), the server-sent stream.
//!
//! The table of operations is `wg_iface::http`'s ([`EVENTS`]); the queries are `wg-runtime`'s `spec`'s
//! and the frames are the `spec` module's. This module is the glue: it runs the store reads on the
//! application context, and it paces the stream.
//!
//! The stream polls the store once a second by `after_seq`, as the Python route does. The bus
//! carries no domain events (no decision publishes one), so it is not the source of the stream.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use crate::spec::events::{batch_frames, queue_frame};
use crate::spec::sse::SseEvent;
use futures_util::stream;
use serde_json::{Map, Value as Params};
use wg_iface::http::groups::EVENTS;
use wg_json::Value;
use wg_model::StoredEvent;
use wg_runtime::App;
use wg_runtime::spec::events::{
    after_seq, history_query, max_events, snapshot_due, stream_query, workspace_reference,
};
use wg_types::{Id, Permission, WgError};
use wg_wire::dto::event::from_stored;

use crate::route::{Answer, ApiRequest, ApiRoute, handler};
use crate::server::Server;
use crate::sse::event_stream;

/// The seconds between two stream ticks (`asyncio.sleep(1.0)`).
const TICK: Duration = Duration::from_secs(1);

/// Registers the group's routes into `table`, in the group's order.
pub fn register(table: &mut Vec<ApiRoute>) {
    for spec in &EVENTS {
        let id = spec.id;
        table.push(ApiRoute {
            id,
            method: spec.method,
            path: spec.path,
            status: spec.status,
            query: spec.query,
            body: None,
            handler: handler(move |server, request| async move {
                request.context.require(&[Permission::WorkspaceRead])?;
                if id == "H123" {
                    let value = server.blocking(move |app| history(app, &request)).await?;
                    Ok(Answer::Json(value))
                } else {
                    open_stream(server, &request.params).await
                }
            }),
        });
    }
}

/// H123: the events of the workspace matching the query, as `EventOut` objects in query order.
fn history(app: &mut App, request: &ApiRequest) -> Result<Value, WgError> {
    let workspace_id = app.workspace_id(&workspace_reference(&request.params))?;
    let Some(query) = history_query(&request.params, workspace_id) else {
        return Ok(Value::List(Vec::new()));
    };
    let records = app
        .runtime
        .store
        .read(|session| session.query_events(&query))?;
    Ok(Value::List(
        records
            .into_iter()
            .map(|record| from_stored(&StoredEvent::from(record)))
            .collect(),
    ))
}

/// H124: resolves the workspace and the starting cursor, then streams the frames.
async fn open_stream(server: Arc<Server>, params: &Map<String, Params>) -> Result<Answer, WgError> {
    let reference = workspace_reference(params);
    let after = after_seq(params);
    let (workspace_id, cursor) = server
        .blocking(move |app| {
            let workspace_id = app.workspace_id(&reference)?;
            let cursor = match after {
                Some(cursor) => cursor,
                None => app.runtime.store.read(|session| session.latest_seq())?,
            };
            Ok((workspace_id, cursor))
        })
        .await?;
    let feed = Feed {
        server,
        workspace_id,
        cursor,
        tick: 0,
        sent: 0,
        max: max_events(params),
        pending: VecDeque::new(),
        primed: false,
    };
    Ok(Answer::Stream(event_stream(stream::unfold(
        feed, next_frame,
    ))))
}

/// The state of one stream between frames.
struct Feed {
    server: Arc<Server>,
    workspace_id: Id,
    /// The `seq` of the last event sent.
    cursor: i64,
    /// Ticks run so far; a queue snapshot goes out on every fifth.
    tick: u64,
    /// Frames sent so far, against `max`.
    sent: u64,
    max: Option<u64>,
    pending: VecDeque<SseEvent>,
    /// Whether a tick has run, so the next one waits a second first.
    primed: bool,
}

/// The next frame of the stream, or `None` when it ends: after `max_events` frames, or when the
/// store cannot be read (the response then ends, as Python's generator would stop on an error).
async fn next_frame(mut feed: Feed) -> Option<(SseEvent, Feed)> {
    loop {
        if feed.max.is_some_and(|max| feed.sent >= max) {
            return None;
        }
        if let Some(frame) = feed.pending.pop_front() {
            feed.sent += 1;
            return Some((frame, feed));
        }
        if feed.primed {
            tokio::time::sleep(TICK).await;
        }
        feed.primed = true;
        let snapshot = snapshot_due(feed.tick);
        feed.tick += 1;
        let (workspace_id, cursor) = (feed.workspace_id, feed.cursor);
        let (frames, next) = feed
            .server
            .blocking(move |app| tick(app, workspace_id, cursor, snapshot))
            .await
            .ok()?;
        feed.cursor = next;
        feed.pending.extend(frames);
    }
}

/// One tick: the events after `cursor` (and the queue snapshot when due), with the new cursor.
fn tick(
    app: &mut App,
    workspace_id: Id,
    cursor: i64,
    snapshot: bool,
) -> Result<(Vec<SseEvent>, i64), WgError> {
    let query = stream_query(workspace_id, cursor);
    let records = app
        .runtime
        .store
        .read(|session| session.query_events(&query))?;
    let next = records.last().map_or(cursor, |record| record.seq);
    let events: Vec<StoredEvent> = records.into_iter().map(StoredEvent::from).collect();
    let mut frames = batch_frames(&events)?;
    if snapshot {
        let stats = app
            .runtime
            .store
            .read(|session| session.job_stats(Some(workspace_id)))?;
        frames.push(queue_frame(&stats)?);
    }
    Ok((frames, next))
}
