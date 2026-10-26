//! The SSE framing of the Python `/mcp/` endpoint (contract X6).
//!
//! The Python SDK writes every JSON-RPC message as one `event: message` event whose `data` is the
//! message (`mcp/server/streamable_http.py`, `_create_event_data`), with CRLF line endings as
//! `sse_starlette` writes them. No event store is configured, so no event carries an `id` or a
//! `retry`, and no priming event is sent. [`frame_events`] rewrites the complete events of the
//! Rust SDK's byte stream to that framing; the transport calls it as the bytes arrive.

/// The complete events at the front of `pending`, rewritten to the Python framing, and how many
/// bytes of `pending` they used. A trailing partial event is left for the next call.
#[must_use]
pub fn frame_events(pending: &[u8]) -> (Vec<u8>, usize) {
    match find_blank_line(pending) {
        None => (Vec::new(), 0),
        Some(end) => {
            let (rest, used) = frame_events(&pending[end + 2..]);
            (
                [frame_block(&pending[..end]), rest].concat(),
                end + 2 + used,
            )
        }
    }
}

/// The index of the first `\n\n` in `bytes`, which ends an event.
fn find_blank_line(bytes: &[u8]) -> Option<usize> {
    bytes.windows(2).position(|pair| pair == b"\n\n")
}

/// One event (its bytes without the blank line) in the Python framing. Its `data` lines make one
/// `event: message` event; an event with no data (a priming event, or one that only carries an
/// `id` or a `retry`) is dropped; a comment-only event (a keep-alive) is kept as it is.
fn frame_block(block: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(block);
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let data: Vec<&str> = lines
        .iter()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|value| value.strip_prefix(' ').unwrap_or(value))
        .collect();
    let comments: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with(':'))
        .collect();
    if data.iter().any(|value| !value.is_empty()) {
        let data_lines: String = data
            .iter()
            .map(|value| format!("data: {value}\r\n"))
            .collect();
        format!("event: message\r\n{data_lines}\r\n").into_bytes()
    } else if comments.is_empty() {
        Vec::new()
    } else {
        let mut kept: String = comments
            .iter()
            .map(|comment| format!("{comment}\r\n"))
            .collect();
        kept.push_str("\r\n");
        kept.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_event_is_written_with_the_python_framing() {
        let input = b"id: 0/1\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n";
        let (out, used) = frame_events(input);
        assert_eq!(used, input.len());
        assert_eq!(
            out,
            b"event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\r\n\r\n"
                .to_vec()
        );
    }

    #[test]
    fn a_priming_event_with_no_data_is_dropped() {
        let (out, used) = frame_events(b"id: 0/0\nretry: 3000\ndata:\n\n");
        assert_eq!(used, 27);
        assert!(out.is_empty());
    }

    #[test]
    fn a_keep_alive_comment_is_kept() {
        let (out, _) = frame_events(b": ping\n\n");
        assert_eq!(out, b": ping\r\n\r\n".to_vec());
    }

    #[test]
    fn a_partial_event_waits_for_its_end() {
        let (out, used) = frame_events(b"data: {\"a\":1}\n");
        assert!(out.is_empty());
        assert_eq!(used, 0);
    }
}
