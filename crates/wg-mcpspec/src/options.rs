//! The command line of `workgraph mcp`: `--transport`, `--agent-name`, `--grant`, `--host`,
//! `--port` (`cli/commands/servers.py`).
//!
//! The Rust `workgraph` binary's clap tree wires this command up (IF-23); the standalone `wg-mcp`
//! binary and the conformance replay use this parser, so both read the flags the same way.

use std::collections::BTreeSet;

use wg_types::{Permission, WgError};

use crate::permissions::agent_permissions;

/// Which transport to serve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Stdio,
    Http,
}

/// The parsed flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServeOptions {
    pub transport: Transport,
    pub agent_name: String,
    /// The default agent permissions plus every `--grant`.
    pub permissions: BTreeSet<Permission>,
    pub host: String,
    pub port: u16,
}

/// The identity every call of one server runs under (`RequestContext.agent(name, perms)`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Agent {
    pub name: String,
    pub permissions: BTreeSet<Permission>,
}

impl ServeOptions {
    /// The agent the flags describe; every transport takes its identity from here (WG-3734).
    pub fn agent(&self) -> Agent {
        Agent {
            name: self.agent_name.clone(),
            permissions: self.permissions.clone(),
        }
    }
}

/// Whether `host` is a loopback address, the only kind the HTTP transport binds.
pub fn is_loopback(host: &str) -> bool {
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

impl ServeOptions {
    /// Parses `args` (a leading `mcp` word is accepted and skipped).
    pub fn parse(args: &[String]) -> Result<Self, WgError> {
        let mut transport = Transport::Stdio;
        let mut agent_name = "mcp-client".to_string();
        let mut grants: Vec<String> = Vec::new();
        let mut host = "127.0.0.1".to_string();
        let mut port: u16 = 8766;
        let mut it = args.iter().peekable();
        if it.peek().map(|a| a.as_str()) == Some("mcp") {
            it.next();
        }
        while let Some(flag) = it.next() {
            let value = it
                .next()
                .ok_or_else(|| WgError::validation(format!("{flag} needs a value")))?;
            match flag.as_str() {
                "--transport" => {
                    transport = match value.as_str() {
                        "stdio" => Transport::Stdio,
                        "http" => Transport::Http,
                        _ => return Err(WgError::validation("transport must be stdio or http")),
                    }
                }
                "--agent-name" => agent_name = value.clone(),
                "--grant" => grants.push(value.clone()),
                "--host" => host = value.clone(),
                "--port" => {
                    port = value
                        .parse()
                        .map_err(|_| WgError::validation(format!("invalid --port {value:?}")))?
                }
                other => return Err(WgError::validation(format!("unknown option {other:?}"))),
            }
        }
        if transport == Transport::Http && !is_loopback(&host) {
            return Err(WgError::validation(
                "the MCP HTTP transport only binds to loopback addresses",
            ));
        }
        Ok(Self {
            transport,
            agent_name,
            permissions: agent_permissions(&grants)?,
            host,
            port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn replay_command_line() {
        let parsed = ServeOptions::parse(&args(&[
            "mcp",
            "--agent-name",
            "conformance",
            "--grant",
            "run:start",
        ]));
        let parsed = parsed.ok();
        assert!(
            parsed
                .as_ref()
                .is_some_and(|o| o.agent_name == "conformance")
        );
        assert!(
            parsed
                .as_ref()
                .is_some_and(|o| o.permissions.contains(&Permission::RunStart))
        );
    }

    #[test]
    fn http_refuses_a_public_host() {
        assert!(ServeOptions::parse(&args(&["--transport", "http", "--host", "0.0.0.0"])).is_err());
    }
}
