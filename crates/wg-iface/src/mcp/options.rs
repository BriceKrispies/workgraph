//! The command line of `workgraph mcp`: `--transport`, `--agent-name`, `--grant`, `--host`,
//! `--port` (`cli/commands/servers.py`).
//!
//! The Rust `workgraph` binary's clap tree wires this command up (IF-23); the standalone `wg-mcp`
//! binary and the conformance replay use this parser, so both read the flags the same way.

use std::collections::BTreeSet;

use wg_types::{Permission, WgError};

use crate::mcp::permissions::agent_permissions;

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

/// The request header that names the agent an HTTP call is attributed to, as `--agent-name` does
/// over stdio (WG-5702). Header names are case-insensitive; this is the lowercase form.
pub const AGENT_HEADER: &str = "x-workgraph-agent";

/// The longest agent name a request header may carry.
const AGENT_NAME_LIMIT: usize = 64;

/// The name in an agent header: one to 64 characters from ASCII letters, digits, `-`, `_` and `.`,
/// surrounding white space trimmed. Anything else is not a name.
#[must_use]
pub fn valid_agent_name(text: &str) -> Option<String> {
    let name = text.trim();
    let valid = !name.is_empty()
        && name.len() <= AGENT_NAME_LIMIT
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    valid.then(|| name.to_owned())
}

/// The agent a call runs under: `agent`, renamed to the `X-Workgraph-Agent` header when the header
/// names a valid agent, with the same permissions. A missing or invalid header leaves `agent` as
/// the flags set it, so a call without the header behaves as it did before.
#[must_use]
pub fn agent_named(agent: &Agent, header: Option<&str>) -> Agent {
    match header.and_then(valid_agent_name) {
        Some(name) => Agent {
            name,
            permissions: agent.permissions.clone(),
        },
        None => agent.clone(),
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
    fn agent_header_renames_the_agent_and_keeps_its_permissions() {
        let flags = ServeOptions::parse(&args(&[
            "--agent-name",
            "mcp-client",
            "--grant",
            "run:start",
        ]));
        let Ok(flags) = flags else {
            panic!("the flags parse");
        };
        let base = flags.agent();
        let named = agent_named(&base, Some(" claude-code "));
        assert_eq!(named.name, "claude-code");
        assert_eq!(named.permissions, base.permissions);
        assert_eq!(agent_named(&base, None), base);
        assert_eq!(agent_named(&base, Some("bad name")), base);
        assert_eq!(agent_named(&base, Some("")), base);
    }

    #[test]
    fn agent_names_are_short_plain_tokens() {
        assert_eq!(
            valid_agent_name("claude-code"),
            Some("claude-code".to_owned())
        );
        assert_eq!(valid_agent_name("agent_1.2"), Some("agent_1.2".to_owned()));
        assert_eq!(valid_agent_name("a/b"), None);
        assert_eq!(valid_agent_name("a\nb"), None);
        assert_eq!(valid_agent_name(&"x".repeat(65)), None);
        assert_eq!(valid_agent_name(&"x".repeat(64)).map(|s| s.len()), Some(64));
    }

    #[test]
    fn http_refuses_a_public_host() {
        assert!(ServeOptions::parse(&args(&["--transport", "http", "--host", "0.0.0.0"])).is_err());
    }
}
