//! Server-sent events as sse-starlette frames them (O08): `\r\n` line separators, a blank line
//! after each event, multi-line data split into `data:` lines, and a ping comment every 15
//! seconds.

use wg_types::Timestamp;

/// The interval between ping comments.
pub const PING_SECONDS: u64 = 15;

/// The headers of an event-stream response besides the security headers.
pub const STREAM_HEADERS: [(&str, &str); 4] = [
    ("content-type", "text/event-stream; charset=utf-8"),
    ("cache-control", "no-store"),
    ("connection", "keep-alive"),
    ("x-accel-buffering", "no"),
];

/// One event: `ServerSentEvent(data, event=..., id=...)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub id: Option<String>,
    pub data: Option<String>,
}

const SEP: &str = "\r\n";

fn split_lines(text: &str) -> Vec<&str> {
    // `re.split(r"\r\n|\r|\n", text)`.
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(['\r', '\n']) {
        parts.push(&rest[..at]);
        let skip = if rest[at..].starts_with("\r\n") { 2 } else { 1 };
        rest = &rest[at + skip..];
    }
    parts.push(rest);
    parts
}

fn strip_newlines(text: &str) -> String {
    text.chars()
        .filter(|ch| *ch != '\r' && *ch != '\n')
        .collect()
}

/// `ServerSentEvent.encode()`.
pub fn encode(event: &SseEvent) -> String {
    let mut out = String::new();
    if let Some(id) = &event.id {
        out.push_str(&format!("id: {}{SEP}", strip_newlines(id)));
    }
    if let Some(name) = &event.event {
        out.push_str(&format!("event: {}{SEP}", strip_newlines(name)));
    }
    if let Some(data) = &event.data {
        for line in split_lines(data) {
            out.push_str(&format!("data: {line}{SEP}"));
        }
    }
    out.push_str(SEP);
    out
}

/// The ping comment: `: ping - <str(datetime.now(timezone.utc))>`.
pub fn ping(now: Timestamp) -> String {
    format!(
        ": ping - {}{SEP}{SEP}",
        now.format_isoformat().replacen('T', " ", 1)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_like_sse_starlette() {
        let event = SseEvent {
            event: Some("domain".into()),
            id: Some("1".into()),
            data: Some("{\"a\":1}".into()),
        };
        assert_eq!(
            encode(&event),
            "id: 1\r\nevent: domain\r\ndata: {\"a\":1}\r\n\r\n"
        );
        let multi = SseEvent {
            event: None,
            id: None,
            data: Some("a\nb\r\nc".into()),
        };
        assert_eq!(encode(&multi), "data: a\r\ndata: b\r\ndata: c\r\n\r\n");
        let at = Timestamp::parse_iso("2026-10-25T13:56:00.123456+00:00").unwrap();
        assert_eq!(
            ping(at),
            ": ping - 2026-10-25 13:56:00.123456+00:00\r\n\r\n"
        );
    }
}
