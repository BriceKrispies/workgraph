//! A minimal in-process HTTP/1.1 server for the wg-net tests. It binds 127.0.0.1 on a free port,
//! answers every request with what its handler returns, and records each request. Connections
//! are closed after one exchange.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use serde_json::Value;
use wg_net::{Net, Sleeper};

/// One request as the server received it.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    /// The request target: path and query string.
    pub target: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    /// The path, without the query string.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }

    /// The decoded value of query parameter `name`, if present.
    pub fn query(&self, name: &str) -> Option<String> {
        let (_, query) = self.target.split_once('?')?;
        query
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == name)
            .map(|(_, value)| percent_decode(value))
    }

    /// The value of header `name`, matched without regard to case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The body as JSON.
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// What the server answers with.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    /// A reply with a JSON body.
    pub fn json(status: u16, value: &Value) -> Self {
        Self {
            status,
            headers: vec![("Content-Type".to_owned(), "application/json".to_owned())],
            body: value.to_string(),
        }
    }

    /// A reply with a plain body.
    pub fn text(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.to_owned(),
        }
    }

    /// The same reply with one more header.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

/// A running fake server.
pub struct Fake {
    /// `http://127.0.0.1:<port>`, with no trailing slash.
    pub url: String,
    seen: Arc<Mutex<Vec<Recorded>>>,
}

impl Fake {
    /// Starts a server that answers each request with `handler`.
    pub fn start<F>(handler: F) -> Self
    where
        F: Fn(&Recorded) -> Reply + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let address = listener.local_addr().expect("local address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<dyn Fn(&Recorded) -> Reply + Send + Sync> = Arc::new(handler);
        let shared = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let handler = Arc::clone(&handler);
                let shared = Arc::clone(&shared);
                thread::spawn(move || serve(stream, handler.as_ref(), &shared));
            }
        });
        Self {
            url: format!("http://{address}"),
            seen,
        }
    }

    /// Every request received so far, in arrival order.
    pub fn requests(&self) -> Vec<Recorded> {
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// A URL that refuses connections: a port that was just free.
pub fn refused_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
    let address = listener.local_addr().expect("local address");
    drop(listener);
    format!("http://{address}")
}

/// A sleeper that records each delay it is asked for.
pub fn recording_sleep() -> (Sleeper, Arc<Mutex<Vec<f64>>>) {
    let delays = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&delays);
    let sleep: Sleeper = Box::new(move |seconds| {
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(seconds);
    });
    (sleep, delays)
}

/// The process host, with retries that sleep through `sleep` instead of the clock.
pub fn net(sleep: Sleeper) -> Net {
    Net { sleep: Some(sleep) }
}

/// The delays recorded by [`recording_sleep`].
pub fn delays_of(delays: &Arc<Mutex<Vec<f64>>>) -> Vec<f64> {
    delays
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

fn serve(
    stream: TcpStream,
    handler: &(dyn Fn(&Recorded) -> Reply + Send + Sync),
    seen: &Mutex<Vec<Recorded>>,
) {
    let mut reader = BufReader::new(stream);
    let Some(request) = read_request(&mut reader) else {
        return;
    };
    let reply = handler(&request);
    seen.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(request);
    let mut response = format!("HTTP/1.1 {} Status\r\n", reply.status);
    for (name, value) in &reply.headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.body.len(),
        reply.body
    ));
    let _ = reader.get_mut().write_all(response.as_bytes());
    let _ = reader.get_mut().flush();
}

fn read_request(reader: &mut BufReader<TcpStream>) -> Option<Recorded> {
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        reader.read_line(&mut header).ok()?;
        let trimmed = header.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.push((name.trim().to_owned(), value.trim().to_owned()));
        }
    }
    let length = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(Recorded {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

/// Decodes `%XX` escapes; anything else is kept as it is.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%' && index + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[index + 1..index + 3]).ok())
            .flatten()
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match escaped {
            Some(byte) => {
                out.push(byte);
                index += 3;
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
