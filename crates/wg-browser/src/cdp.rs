//! The DevTools WebSocket client: one page target, one request at a time.

use std::io::ErrorKind;
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::Value;
use tungstenite::protocol::{Message, WebSocketConfig};
use tungstenite::{Error as WsError, WebSocket};
use wg_browserspec::{
    CdpCall, Reply, evaluate_call, evaluated_true, io_failure, parse_reply, request_text,
    unavailable,
};
use wg_types::WgError;

/// A synchronous DevTools client for one page.
pub(crate) struct Cdp {
    socket: WebSocket<TcpStream>,
    next_id: u64,
}

impl Cdp {
    /// Opens the page's WebSocket on the loopback `port`.
    pub(crate) fn connect(port: u16, ws_url: &str, timeout: Duration) -> Result<Self, WgError> {
        let stream = std::net::TcpStream::connect(("127.0.0.1", port))
            .map_err(|err| io_failure("reach the browser DevTools", err))?;
        stream
            .set_read_timeout(Some(timeout))
            .and_then(|()| stream.set_write_timeout(Some(timeout)))
            .map_err(|err| io_failure("configure the DevTools socket", err))?;
        let config = WebSocketConfig::default().max_message_size(None);
        let (socket, _response) =
            tungstenite::client::client_with_config(ws_url, stream, Some(config)).map_err(
                |err| unavailable(format!("browser refused the DevTools socket: {err}")),
            )?;
        Ok(Self { socket, next_id: 0 })
    }

    /// Sends `call` and returns the `result` of its reply, waiting at most `timeout`.
    pub(crate) fn call(&mut self, call: &CdpCall, timeout: Duration) -> Result<Value, WgError> {
        self.next_id += 1;
        let id = self.next_id;
        self.socket
            .send(Message::text(request_text(id, call)))
            .map_err(|err| unavailable(format!("could not send {}: {err}", call.method)))?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|left| !left.is_zero())
                .ok_or_else(|| {
                    unavailable(format!(
                        "browser did not answer {} within {:.0}s",
                        call.method,
                        timeout.as_secs_f64()
                    ))
                })?;
            self.socket
                .get_ref()
                .set_read_timeout(Some(remaining))
                .map_err(|err| io_failure("set the DevTools read timeout", err))?;
            let message = match self.socket.read() {
                Ok(message) => message,
                Err(WsError::Io(err))
                    if matches!(err.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) =>
                {
                    continue;
                }
                Err(err) => return Err(unavailable(format!("DevTools socket failed: {err}"))),
            };
            let Message::Text(text) = message else {
                continue;
            };
            match parse_reply(text.as_str(), id) {
                Some(Reply::Result(result)) => return Ok(result),
                Some(Reply::Error(message)) => {
                    return Err(unavailable(format!(
                        "browser refused {}: {message}",
                        call.method
                    )));
                }
                None => continue,
            }
        }
    }

    /// Evaluates `expression` in the page and reports whether it is true.
    pub(crate) fn evaluate_true(
        &mut self,
        expression: &str,
        timeout: Duration,
    ) -> Result<bool, WgError> {
        let result = self.call(&evaluate_call(expression), timeout)?;
        Ok(evaluated_true(&result))
    }
}
