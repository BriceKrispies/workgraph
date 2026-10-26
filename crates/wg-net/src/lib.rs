//! wg-net: blocking HTTP clients for Ollama, OpenAI-compatible and Jira (SH-18).
//!
//! The requests, retry decisions, JQL and parsing are `wg-netspec`'s. This crate sends them: the
//! socket, the sleeping, the clock, the time zones and the log are [`Net`]. [`Net::perform`] is
//! also the `Http` effect's client in `wg-runtime`. The plain-HTTP status probe of
//! `workgraph ui open` is [`http_status_line`].

pub mod client;
pub mod host;
pub mod jira;
pub mod ollama;
pub mod openai;

pub use host::{Net, Sleeper};
pub use jira::JiraClient;
pub use ollama::Ollama;
pub use openai::OpenAiCompatible;
pub use wg_netspec::client::{Endpoint, Generated};
pub use wg_netspec::jira::Flavour;
pub use wg_netspec::{ProviderError, TrackerError};

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
