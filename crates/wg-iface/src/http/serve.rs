//! The decisions of `workgraph serve` that are functions of values (`cli/commands/servers.py`):
//! the `server` configuration it reads and the refusal to bind a
//! non-loopback address.

use serde_json::{Map, Value};

use crate::http::lan::is_loopback_host;

/// The `[server]` table of the configuration, with the defaults of `ServerConfig`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub allow_non_loopback: bool,
    pub lan: bool,
    pub lan_address: Option<String>,
    pub lan_networks: Vec<String>,
}

impl Settings {
    /// Reads the `server` table of a loaded configuration (already validated and defaulted).
    pub fn from_config(config: &Map<String, Value>) -> Settings {
        let server = config.get("server").and_then(Value::as_object);
        let field = |name: &str| server.and_then(|table| table.get(name));
        Settings {
            host: field("host")
                .and_then(Value::as_str)
                .unwrap_or("127.0.0.1")
                .to_owned(),
            port: field("port")
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok())
                .unwrap_or(8765),
            allow_non_loopback: field("allow_non_loopback")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            lan: field("lan").and_then(Value::as_bool).unwrap_or(true),
            lan_address: field("lan_address")
                .and_then(Value::as_str)
                .map(str::to_owned),
            lan_networks: field("lan_networks")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

/// The refusal for a non-loopback bind that nothing allowed, else `None`.
pub fn bind_refusal(bind_host: &str, allowed: bool) -> Option<String> {
    (!is_loopback_host(bind_host) && !allowed).then(|| {
        format!(
            "refusing to bind to non-loopback address {bind_host}; pass --allow-remote to opt in"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn settings_read_the_server_table_or_the_defaults() {
        let empty = Settings::from_config(&Map::new());
        assert_eq!(empty.host, "127.0.0.1");
        assert_eq!(empty.port, 8765);
        assert!(empty.lan && !empty.allow_non_loopback);
        let config = json!({"server": {"host": "localhost", "port": 9000, "lan": false,
            "lan_address": "10.0.0.2", "lan_networks": ["10.0.0.0/8"]}});
        let read = Settings::from_config(config.as_object().unwrap_or(&Map::new()));
        assert_eq!(read.host, "localhost");
        assert_eq!(read.port, 9000);
        assert!(!read.lan);
        assert_eq!(read.lan_address.as_deref(), Some("10.0.0.2"));
        assert_eq!(read.lan_networks, vec!["10.0.0.0/8".to_owned()]);
    }

    #[test]
    fn refusals() {
        assert!(bind_refusal("0.0.0.0", false).is_some_and(|m| m.contains("--allow-remote")));
        assert_eq!(bind_refusal("0.0.0.0", true), None);
        assert_eq!(bind_refusal("127.0.0.1", false), None);
    }
}
