//! The HTTP driver (CR-05) and the scrubber it shares with the CF-04 replay.
//!
//! A small HTTP/1.1 client over `std::net::TcpStream`: one request per connection
//! (`Connection: close`), chunked bodies decoded, server-sent events parsed when a
//! step asks for `max_events`. Responses are scrubbed with the placeholders the
//! CF-04 recorder writes into `contracts/http/*.json` (`<token>`, `<uuid>`, `<ts>`,
//! `<home>`, `<user>`, `<session>`, `:<port>`), not with the section 3.4 numbering.
//! The pure parts (head, chunk and event parsing, scrubbing) take bytes and return
//! values; the socket and process glue sits around them.

use crate::runner::{Implementation, Sandbox, env_for};
use serde_json::{Map, Value};
use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Response headers the CF-04 recorder keeps (`RECORDED_HEADERS` in `record_http.py`).
pub const RECORDED_HEADERS: [&str; 11] = [
    "allow",
    "cache-control",
    "content-type",
    "location",
    "mcp-session-id",
    "referrer-policy",
    "set-cookie",
    "www-authenticate",
    "x-accel-buffering",
    "x-content-type-options",
    "x-frame-options",
];

/// One request to send. Header names are sent as given; `Host` is added unless given.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub headers: &'a [(String, String)],
    pub body: Option<&'a [u8]>,
}

/// A received response. Header names are lowercased, in the order received.
#[derive(Debug, Clone, PartialEq)]
pub struct Exchange {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Parsed server-sent events, filled only when `max_events` was given.
    pub events: Vec<Value>,
}

impl Exchange {
    /// The first value of a header (lowercase name).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The bytes of a request: request line, headers, `Content-Length` when a body or a
/// POST, PUT or PATCH is sent, then the body.
pub fn request_bytes(port: u16, req: &Request<'_>) -> Vec<u8> {
    let has = |name: &str| {
        req.headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(name))
    };
    let mut head = format!("{} {} HTTP/1.1\r\n", req.method, req.path);
    if !has("host") {
        head.push_str(&format!("Host: 127.0.0.1:{port}\r\n"));
    }
    head.push_str("Connection: close\r\n");
    // The recordings were made with an HTTP client that sends `Accept: */*` by default, and
    // the MCP route answers differently without it.
    if !has("accept") {
        head.push_str("Accept: */*\r\n");
    }
    for (name, value) in req.headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    let body = req.body.unwrap_or(&[]);
    let needs_length = !body.is_empty() || matches!(req.method, "POST" | "PUT" | "PATCH");
    if needs_length && !has("content-length") {
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("\r\n");
    let mut out = head.into_bytes();
    out.extend_from_slice(body);
    out
}

/// Send one request to `127.0.0.1:port` and read the whole response. With
/// `max_events`, the body is read as server-sent events and the connection is
/// closed as soon as that many events have arrived.
pub fn send(port: u16, req: &Request<'_>, max_events: Option<usize>) -> Result<Exchange, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))
        .map_err(|e| format!("connect 127.0.0.1:{port}: {e}"))?;
    let window = crate::wait::stall_timeout();
    stream
        .set_read_timeout(Some(crate::wait::POLL))
        .and_then(|()| stream.set_write_timeout(Some(window)))
        .map_err(|e| format!("socket timeout: {e}"))?;
    stream
        .write_all(&request_bytes(port, req))
        .map_err(|e| format!("write request: {e}"))?;
    read_exchange(&mut stream, max_events)
}

fn read_exchange(stream: &mut impl Read, max_events: Option<usize>) -> Result<Exchange, String> {
    let mut raw = Vec::new();
    let mut buf = [0_u8; 8192];
    let head_end = loop {
        if let Some(pos) = find(&raw, b"\r\n\r\n") {
            break pos;
        }
        let n = read_some(stream, &mut buf)?;
        if n == 0 {
            return Err("connection closed before the response head".to_owned());
        }
        raw.extend_from_slice(&buf[..n]);
    };
    let (status, headers) = parse_head(&raw[..head_end])?;
    let chunked = header_of(&headers, "transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    let length = header_of(&headers, "content-length").and_then(|v| v.parse::<usize>().ok());
    let mut pending = raw.split_off(head_end + 4);
    let mut decoder = ChunkDecoder::default();
    let mut sse = SseParser::default();
    let mut body = Vec::new();
    let mut events = Vec::new();
    loop {
        let data = if chunked {
            decoder.feed(&pending)?
        } else {
            std::mem::take(&mut pending)
        };
        body.extend_from_slice(&data);
        if let Some(max) = max_events {
            events.extend(sse.feed(&data));
            if events.len() >= max {
                events.truncate(max);
                break;
            }
        }
        if chunked && decoder.is_done() {
            break;
        }
        if !chunked && length.is_some_and(|n| body.len() >= n) {
            break;
        }
        let n = read_some(stream, &mut buf)?;
        if n == 0 {
            break;
        }
        pending = buf[..n].to_vec();
    }
    Ok(Exchange {
        status,
        headers,
        body,
        events,
    })
}

/// One read. A reset or closed peer counts as the end of the stream. The socket polls in short
/// slices (see `send`): a silent peer is an error once the stall window has passed, and a spent
/// budget is an error at once.
fn read_some(stream: &mut impl Read, buf: &mut [u8]) -> Result<usize, String> {
    let quiet = Instant::now();
    let window = crate::wait::stall_timeout();
    loop {
        match stream.read(buf) {
            Ok(n) => return Ok(n),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if crate::budget::cut() {
                    return Err(crate::budget::cut_error());
                }
                if quiet.elapsed() >= window {
                    return Err(format!("stalled: no response bytes: {e}"));
                }
            }
            Err(_) => return Ok(0),
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn header_of<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// The status code and the lowercase headers of a response head (without the blank line).
pub fn parse_head(head: &[u8]) -> Result<(u16, Vec<(String, String)>), String> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split('\n').map(|l| l.trim_end_matches('\r'));
    let status_line = lines.next().ok_or("empty response head")?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("bad status line `{status_line}`"))?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    Ok((status, headers))
}

/// Incremental decoder for `Transfer-Encoding: chunked` bodies. Trailers are not supported,
/// which is what the Python server sends.
#[derive(Debug, Default)]
pub struct ChunkDecoder {
    buf: Vec<u8>,
    done: bool,
}

impl ChunkDecoder {
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Add raw bytes; returns the body bytes that are complete so far.
    pub fn feed(&mut self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while !self.done {
            let Some(line_end) = find(&self.buf, b"\r\n") else {
                break;
            };
            let size_text = String::from_utf8_lossy(&self.buf[..line_end]).into_owned();
            let size_hex = size_text.split(';').next().unwrap_or("").trim();
            let size = usize::from_str_radix(size_hex, 16)
                .map_err(|e| format!("bad chunk size `{size_hex}`: {e}"))?;
            if size == 0 {
                self.done = true;
                self.buf.drain(..line_end + 2);
                break;
            }
            let need = line_end + 2 + size + 2;
            if self.buf.len() < need {
                break;
            }
            out.extend_from_slice(&self.buf[line_end + 2..line_end + 2 + size]);
            self.buf.drain(..need);
        }
        Ok(out)
    }
}

/// Incremental server-sent events parser. Each completed event becomes the object the
/// CF-04 recorder writes: `{"event": name or "message", "data": parsed JSON or text,
/// "id": when an id line was seen}`.
#[derive(Debug, Default)]
pub struct SseParser {
    pending: Vec<u8>,
    name: Option<String>,
    seq: Option<String>,
    data: Vec<String>,
}

impl SseParser {
    /// Add raw bytes; returns the events completed by them.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Value> {
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(pos) = self.pending.iter().position(|b| *b == b'\n') {
            let rest = self.pending.split_off(pos + 1);
            let mut line = std::mem::replace(&mut self.pending, rest);
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let text = String::from_utf8_lossy(&line).into_owned();
            if let Some(event) = self.line(&text) {
                events.push(event);
            }
        }
        events
    }

    fn line(&mut self, line: &str) -> Option<Value> {
        if line.is_empty() {
            let (name, seq, data) = (
                self.name.take(),
                self.seq.take(),
                std::mem::take(&mut self.data),
            );
            if name.is_none() && data.is_empty() {
                return None;
            }
            let raw = data.join("\n");
            let payload = serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw));
            let mut event = Map::new();
            event.insert(
                "event".to_owned(),
                Value::String(name.unwrap_or_else(|| "message".to_owned())),
            );
            event.insert("data".to_owned(), payload);
            if let Some(seq) = seq {
                event.insert("id".to_owned(), Value::String(seq));
            }
            return Some(Value::Object(event));
        }
        if let Some(rest) = line.strip_prefix("event:") {
            self.name = Some(rest.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("id:") {
            self.seq = Some(rest.trim().to_owned());
        } else if let Some(rest) = line.strip_prefix("data:") {
            self.data.push(rest.trim_start().to_owned());
        }
        None
    }
}

/// Replaces run-specific values with the placeholders of the CF-04 recordings.
/// Literals go first (longest first, as the recorder does), then UUIDs, then timestamps.
#[derive(Debug, Clone, Default)]
pub struct Scrubber {
    literals: Vec<(String, String)>,
}

impl Scrubber {
    /// Register a literal (empty literals are ignored).
    pub fn add(&mut self, literal: &str, placeholder: &str) {
        if literal.is_empty() {
            return;
        }
        self.literals
            .push((literal.to_owned(), placeholder.to_owned()));
        self.literals
            .sort_by_key(|(l, _)| std::cmp::Reverse(l.len()));
    }

    pub fn text(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for (literal, placeholder) in &self.literals {
            out = out.replace(literal.as_str(), placeholder);
        }
        timestamps(&uuids(&out))
    }

    /// Scrub every string inside a JSON value (keys are left alone, as the recorder does).
    pub fn value(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.text(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(x)).collect()),
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, x)| (k.clone(), self.value(x))).collect())
            }
            other => other.clone(),
        }
    }
}

/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` in lowercase hex becomes `<uuid>`.
fn uuids(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if i + 36 <= b.len() && is_uuid(&b[i..i + 36]) {
            out.extend_from_slice(b"<uuid>");
            i += 36;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_uuid(s: &[u8]) -> bool {
    s.iter().enumerate().all(|(k, c)| match k {
        8 | 13 | 18 | 23 => *c == b'-',
        _ => c.is_ascii_digit() || (b'a'..=b'f').contains(c),
    })
}

fn timestamps(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match timestamp_len(&b[i..]) {
            Some(n) => {
                out.extend_from_slice(b"<ts>");
                i += n;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Length of a timestamp at the start of `s`: `YYYY-MM-DDTHH:MM:SS`, optionally with
/// fraction digits and then `Z` or a `+HH:MM` / `-HH:MM` offset.
fn timestamp_len(s: &[u8]) -> Option<usize> {
    let digits = |from: usize, n: usize| {
        s.get(from..from + n)
            .is_some_and(|d| d.iter().all(u8::is_ascii_digit))
    };
    let shape = digits(0, 4)
        && s.get(4) == Some(&b'-')
        && digits(5, 2)
        && s.get(7) == Some(&b'-')
        && digits(8, 2)
        && s.get(10) == Some(&b'T')
        && digits(11, 2)
        && s.get(13) == Some(&b':')
        && digits(14, 2)
        && s.get(16) == Some(&b':')
        && digits(17, 2);
    if !shape {
        return None;
    }
    let mut end = 19;
    if s.get(end) == Some(&b'.') {
        let mut k = end + 1;
        while s.get(k).is_some_and(u8::is_ascii_digit) {
            k += 1;
        }
        if k > end + 1 {
            end = k;
        }
    }
    if s.get(end) == Some(&b'Z') {
        end += 1;
    } else if s.get(end).is_some_and(|c| *c == b'+' || *c == b'-')
        && digits(end + 1, 2)
        && s.get(end + 3) == Some(&b':')
        && digits(end + 4, 2)
    {
        end += 6;
    }
    Some(end)
}

/// A free loopback port: bind port 0, read the number, release it.
pub fn free_port() -> Result<u16, String> {
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).map_err(|e| format!("bind a free port: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local address: {e}"))?
        .port();
    Ok(port)
}

/// A running `serve --local --no-worker` process for one sandbox. Dropping it kills the
/// process, so the sandbox can be removed afterwards.
#[derive(Debug)]
pub struct Server {
    child: Child,
    pub port: u16,
    pub token: String,
}

impl Server {
    /// Start the implementation's `serve` on a free port in `sb`, wait for `/api/v1/health`,
    /// and read the API token from `<home>/config/api-token`.
    pub(crate) fn start(imp: &Implementation, sb: &Sandbox) -> Result<Self, String> {
        let port = free_port()?;
        let log_path = sb.root.join(format!("serve-{port}.log"));
        let log =
            File::create(&log_path).map_err(|e| format!("create {}: {e}", log_path.display()))?;
        let log_err = log
            .try_clone()
            .map_err(|e| format!("clone log handle: {e}"))?;
        let mut cmd = Command::new(&imp.program);
        cmd.args(&imp.args)
            .args(["serve", "--local", "--no-worker", "--port"])
            .arg(port.to_string())
            .current_dir(&sb.repos)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        for (k, _) in std::env::vars().filter(|(k, _)| k.starts_with("WORKGRAPH_")) {
            cmd.env_remove(k);
        }
        cmd.envs(env_for(imp, sb, &[]));
        let child = cmd.spawn().map_err(|e| format!("spawn serve: {e}"))?;
        let mut server = Self {
            child,
            port,
            token: String::new(),
        };
        let deadline = Instant::now() + crate::wait::STARTUP_TIMEOUT;
        loop {
            if crate::budget::cut() {
                return Err(crate::budget::cut_error());
            }
            if let Some(status) = server
                .child
                .try_wait()
                .map_err(|e| format!("poll serve: {e}"))?
            {
                return Err(format!(
                    "serve exited early ({status}); see {}",
                    log_path.display()
                ));
            }
            let health = Request {
                method: "GET",
                path: "/api/v1/health",
                headers: &[],
                body: None,
            };
            if send(port, &health, None).is_ok_and(|e| e.status == 200) {
                break;
            }
            if Instant::now() >= deadline {
                return Err(format!(
                    "serve on port {port} was not ready after {} s; see {}",
                    crate::wait::STARTUP_TIMEOUT.as_secs(),
                    log_path.display()
                ));
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let token_path = sb.home.join("config").join("api-token");
        server.token = std::fs::read_to_string(&token_path)
            .map_err(|e| format!("read {}: {e}", token_path.display()))?
            .trim()
            .to_owned();
        Ok(server)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // On Windows a plain kill leaves the serve process's children running, and each run left
        // orphans behind, so the whole tree goes (as the Python runner's taskkill /T does).
        if cfg!(windows) {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/PID", &self.child.id().to_string()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn head_lines_and_lowercase_names() {
        let (status, headers) =
            parse_head(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer\r\nContent-Type: application/json").unwrap();
        assert_eq!(status, 401);
        assert_eq!(
            headers,
            vec![
                ("www-authenticate".to_owned(), "Bearer".to_owned()),
                ("content-type".to_owned(), "application/json".to_owned()),
            ]
        );
        assert!(parse_head(b"nonsense").is_err());
    }

    #[test]
    fn chunked_body_decodes_across_feeds() {
        let mut d = ChunkDecoder::default();
        let mut out = Vec::new();
        for piece in [&b"5\r\nhel"[..], b"lo\r\n6;ext=1\r\n world\r\n0\r\n\r\n"] {
            out.extend(d.feed(piece).unwrap());
        }
        assert_eq!(out, b"hello world");
        assert!(d.is_done());
    }

    #[test]
    fn sse_events_with_crlf_multiline_data_and_ids() {
        let mut p = SseParser::default();
        let text = b"id: 1\r\nevent: domain\r\ndata: {\"seq\":1,\r\ndata: \"x\":2}\r\n\r\n:heartbeat\n\nevent: queue\ndata: {\"queued\": 2}\n\n";
        let events = p.feed(&text[..10]);
        assert!(events.is_empty());
        let mut events = p.feed(&text[10..]);
        assert_eq!(events.len(), 2);
        let second = events.pop().unwrap();
        let first = events.pop().unwrap();
        assert_eq!(
            first,
            json!({"event": "domain", "data": {"seq": 1, "x": 2}, "id": "1"})
        );
        assert_eq!(second, json!({"event": "queue", "data": {"queued": 2}}));
    }

    #[test]
    fn scrubber_uses_recording_placeholders() {
        let mut s = Scrubber::default();
        s.add("/tmp/home", "<home>");
        s.add("secret-token", "<token>");
        s.add(":8123", ":<port>");
        let text = "at /tmp/home with secret-token on 127.0.0.1:8123, id 3F2B8C1E-9A4D-4E6F-8B7A-1C2D3E4F5A6B, at 2026-10-25T13:56:00.123+00:00 and 2026-10-25T13:56:00Z";
        assert_eq!(
            s.text(text),
            "at <home> with <token> on 127.0.0.1:<port>, id 3F2B8C1E-9A4D-4E6F-8B7A-1C2D3E4F5A6B, at <ts> and <ts>"
        );
        let v = json!({"actor": "human:brice", "id": "3f2b8c1e-9a4d-4e6f-8b7a-1c2d3e4f5a6b", "list": ["/tmp/home"]});
        s.add("brice", "<user>");
        assert_eq!(
            s.value(&v),
            json!({"actor": "human:<user>", "id": "<uuid>", "list": ["<home>"]})
        );
    }

    #[test]
    fn request_bytes_add_host_and_length_only_when_needed() {
        let get = Request {
            method: "GET",
            path: "/x",
            headers: &[],
            body: None,
        };
        let text = String::from_utf8(request_bytes(9, &get)).unwrap();
        assert_eq!(
            text,
            "GET /x HTTP/1.1\r\nHost: 127.0.0.1:9\r\nConnection: close\r\nAccept: */*\r\n\r\n"
        );
        let headers = [("host".to_owned(), "evil.example".to_owned())];
        let post = Request {
            method: "POST",
            path: "/y",
            headers: &headers,
            body: None,
        };
        let text = String::from_utf8(request_bytes(9, &post)).unwrap();
        assert_eq!(
            text,
            "POST /y HTTP/1.1\r\nConnection: close\r\nAccept: */*\r\nhost: evil.example\r\nContent-Length: 0\r\n\r\n"
        );
    }
}
