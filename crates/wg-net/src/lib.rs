//! wg-net: blocking HTTP clients for Ollama, OpenAI-compatible and Jira.
//!
//! So far this crate holds the plain-HTTP request used by `workgraph ui open` to see whether a
//! server answers. The provider clients (SH-18) add to it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// The longest status line read; anything longer is cut off.
const MAX_STATUS_LINE: u64 = 8192;

/// Sends `request` to `authority` (`host:port`) and returns the first line of the reply (the
/// status line). Connecting, writing and each read are bounded by `timeout`.
pub fn http_status_line(
    authority: &str,
    request: &str,
    timeout: Duration,
) -> Result<String, String> {
    let address = authority
        .to_socket_addrs()
        .map_err(|err| format!("cannot resolve {authority}: {err}"))?
        .next()
        .ok_or_else(|| format!("no address for {authority}"))?;
    let mut stream = TcpStream::connect_timeout(&address, timeout)
        .map_err(|err| format!("cannot connect to {authority}: {err}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|()| stream.set_write_timeout(Some(timeout)))
        .map_err(|err| format!("cannot set the timeout on {authority}: {err}"))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|err| format!("cannot write to {authority}: {err}"))?;
    let mut line = String::new();
    BufReader::new(Read::take(stream, MAX_STATUS_LINE))
        .read_line(&mut line)
        .map_err(|err| format!("cannot read from {authority}: {err}"))?;
    Ok(line.trim_end().to_owned())
}
