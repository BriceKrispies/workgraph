//! Event-stream responses (O08): `EventSourceResponse(generator, ping=15)`.

use std::convert::Infallible;

use axum::body::{Body, Bytes};
use axum::http::{HeaderName, HeaderValue};
use axum::response::Response;
use futures_util::{Stream, StreamExt};
use wg_runtime::draw;
use wg_serverspec::sse::{PING_SECONDS, STREAM_HEADERS, SseEvent, encode, ping};

/// A streaming response of `events`, framed as sse-starlette frames them, with a ping comment
/// every 15 seconds. The stream ends when `events` ends or the client goes away.
pub fn event_stream<S>(events: S) -> Response
where
    S: Stream<Item = SseEvent> + Send + 'static,
{
    let (sender, receiver) = tokio::sync::mpsc::channel::<Bytes>(16);
    tokio::spawn(async move {
        let period = std::time::Duration::from_secs(PING_SECONDS);
        let mut pings = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        let mut events = Box::pin(events);
        loop {
            let frame = tokio::select! {
                next = events.next() => match next {
                    Some(event) => encode(&event),
                    None => break,
                },
                _ = pings.tick() => match draw() {
                    Ok((now, _)) => ping(now),
                    Err(_) => continue,
                },
            };
            if sender.send(Bytes::from(frame)).await.is_err() {
                break;
            }
        }
    });
    let body = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver
            .recv()
            .await
            .map(|frame| (Ok::<Bytes, Infallible>(frame), receiver))
    });
    let mut response = Response::new(Body::from_stream(body));
    for (name, value) in STREAM_HEADERS {
        response.headers_mut().insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    response
}
