//! The local HTTP security rules of `api/security.py` (`LocalSecurityMiddleware`):
//!
//! * on a LAN-enabled server only loopback and private-network clients are served, and `/mcp`
//!   stays loopback-only (403 `forbidden_client`);
//! * the `Host` header must name an allowed host, defeating DNS rebinding (421 `forbidden_host`);
//! * every `/api/` and `/mcp` request except `/api/v1/health` needs the API token, as
//!   `Authorization: Bearer`, `X-Workgraph-Token`, or `?token=` on the event stream (401
//!   `unauthorized`), and a state-changing request with an `Origin` must come from an allowed
//!   origin (403 `forbidden_origin`);
//! * a browser on another device gets the token only once paired: the device cookie is an HMAC
//!   of the token, so rotating the token unpairs every device.
//!
//! Every comparison with a secret is constant-time.

use std::collections::BTreeSet;

use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

use wg_iface::http::form::{parse_qsl, query_get};
use wg_iface::http::net::{Ip, Network, private_networks};
use wg_iface::http::reply::{Reply, bare_error};
use wg_iface::http::text::py_repr;

/// The cookie a paired device carries.
pub const DEVICE_COOKIE: &str = "workgraph_device";
/// `Max-Age` of the device cookie: 400 days.
pub const DEVICE_COOKIE_MAX_AGE: u64 = 400 * 24 * 3600;
/// Paths under `/api/` served without the token.
pub const PUBLIC_PATHS: [&str; 1] = ["/api/v1/health"];
/// Methods that never change state; a foreign `Origin` is allowed on them.
pub const SAFE_METHODS: [&str; 3] = ["GET", "HEAD", "OPTIONS"];

/// Serving the UI to other devices on the local network (`LanAccess` without the pairing state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lan {
    /// The server's address on the LAN.
    pub address: String,
    /// The client networks served besides loopback.
    pub networks: Vec<Network>,
    /// The names the LAN listener answers to: the address, the host name and `<name>.local`.
    pub hosts: Vec<String>,
}

impl Lan {
    /// `LanAccess.build(address, networks)`: the configured networks, or the private ranges when
    /// none are configured. `hostname` is `socket.gethostname()`. A network that does not parse
    /// is an error naming it.
    pub fn build(address: &str, networks: &[String], hostname: &str) -> Result<Lan, String> {
        let parsed = networks
            .iter()
            .map(|text| Network::parse(text).ok_or_else(|| text.clone()))
            .collect::<Result<Vec<Network>, String>>()
            .map_err(|text| {
                format!(
                    "{} does not appear to be an IPv4 or IPv6 network",
                    py_repr(&text)
                )
            })?;
        let name = hostname.to_lowercase();
        Ok(Lan {
            address: address.to_owned(),
            networks: if parsed.is_empty() {
                private_networks()
            } else {
                parsed
            },
            hosts: vec![address.to_owned(), name.clone(), format!("{name}.local")],
        })
    }

    /// `LanAccess.client_allowed`: loopback, or inside one of the networks; an IPv4-mapped IPv6
    /// address counts as its IPv4 address.
    pub fn client_allowed(&self, client: Option<Ip>) -> bool {
        let Some(addr) = client else {
            return false;
        };
        let addr = addr.ipv4_mapped().unwrap_or(addr);
        addr.is_loopback() || self.networks.iter().any(|net| net.contains(addr))
    }
}

/// `allowed_hosts(host, port, lan)`: the loopback names, the bound host, the LAN names and
/// `WORKGRAPH_ALLOWED_HOSTS` (comma-separated), each bare and with `:port`.
pub fn allowed_hosts(
    host: &str,
    port: u16,
    lan: Option<&Lan>,
    extra: Option<&str>,
) -> BTreeSet<String> {
    let mut hosts: BTreeSet<String> = ["localhost", "127.0.0.1", "[::1]", host]
        .iter()
        .map(|h| (*h).to_owned())
        .collect();
    if let Some(lan) = lan {
        hosts.extend(lan.hosts.iter().cloned());
    }
    hosts.extend(
        extra
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|h| !h.is_empty())
            .map(str::to_owned),
    );
    let with_ports: Vec<String> = hosts.iter().map(|h| format!("{h}:{port}")).collect();
    hosts.extend(with_ports);
    hosts
}

/// `allowed_origins(host, port, lan)`: `http://<name>:<port>` for the same names, and
/// `WORKGRAPH_ALLOWED_ORIGINS` (comma-separated, trailing `/` removed).
pub fn allowed_origins(
    host: &str,
    port: u16,
    lan: Option<&Lan>,
    extra: Option<&str>,
) -> BTreeSet<String> {
    let names = ["localhost", "127.0.0.1", "[::1]", host]
        .into_iter()
        .map(str::to_owned)
        .chain(lan.map(|lan| lan.hosts.clone()).unwrap_or_default());
    let mut origins: BTreeSet<String> = names.map(|h| format!("http://{h}:{port}")).collect();
    origins.extend(
        extra
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|o| !o.is_empty())
            .map(|o| o.trim_end_matches('/').to_owned()),
    );
    origins
}

/// What the middleware checks requests against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub token: String,
    pub hosts: BTreeSet<String>,
    pub origins: BTreeSet<String>,
    pub lan: Option<Lan>,
}

impl Policy {
    /// The policy of a server bound to `host:port`, with the two environment overrides.
    pub fn new(
        token: &str,
        host: &str,
        port: u16,
        lan: Option<Lan>,
        extra_hosts: Option<&str>,
        extra_origins: Option<&str>,
    ) -> Policy {
        Policy {
            token: token.to_owned(),
            hosts: allowed_hosts(host, port, lan.as_ref(), extra_hosts),
            origins: allowed_origins(host, port, lan.as_ref(), extra_origins),
            lan,
        }
    }
}

/// The parts of a request the security rules read. `path` is the percent-decoded path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Incoming {
    pub method: String,
    pub path: String,
    /// The raw query string, without `?`.
    pub query: String,
    pub host: Option<String>,
    pub origin: Option<String>,
    pub authorization: Option<String>,
    pub workgraph_token: Option<String>,
    pub cookie: Option<String>,
    /// The peer address, when the transport has one.
    pub client: Option<Ip>,
}

impl Incoming {
    /// The parts of an HTTP request the rules read; `client` is the peer address. A header that
    /// is not visible ASCII counts as absent.
    pub fn from_parts(parts: &http::request::Parts, client: Option<Ip>) -> Incoming {
        let header = |name: &str| {
            parts
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        Incoming {
            method: parts.method.as_str().to_owned(),
            path: crate::spec::routes::decode_path(parts.uri.path()),
            query: parts.uri.query().unwrap_or("").to_owned(),
            host: header("host"),
            origin: header("origin"),
            authorization: header("authorization"),
            workgraph_token: header("x-workgraph-token"),
            cookie: header("cookie"),
            client,
        }
    }
}

/// Constant-time equality of two secrets (`hmac.compare_digest`).
pub fn secrets_equal(a: &str, b: &str) -> bool {
    bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// `is_loopback_client`.
pub fn is_loopback_client(client: Option<Ip>) -> bool {
    client.is_some_and(Ip::is_loopback)
}

/// The token a request supplies: `Authorization: Bearer <t>` (any case of `bearer `), else
/// `X-Workgraph-Token`, else `?token=` on a path ending in `/events/stream`.
pub fn supplied_token(req: &Incoming) -> Option<String> {
    let header = req.authorization.as_deref().unwrap_or("");
    let bearer = header
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer "));
    let supplied = if bearer {
        header.get(7..).map(|rest| rest.trim().to_owned())
    } else {
        req.workgraph_token.clone()
    };
    match supplied {
        Some(token) if !token.is_empty() => Some(token),
        _ if req.path.ends_with("/events/stream") => query_get(&parse_qsl(&req.query), "token"),
        other => other,
    }
}

/// `_authorized`: a non-empty supplied token equal to the API token.
pub fn authorized(policy: &Policy, req: &Incoming) -> bool {
    supplied_token(req)
        .is_some_and(|token| !token.is_empty() && secrets_equal(&token, &policy.token))
}

/// The middleware's verdict: `Some(reply)` rejects the request, `None` lets it through.
pub fn screen(policy: &Policy, req: &Incoming) -> Option<Reply> {
    if let Some(lan) = &policy.lan {
        if !lan.client_allowed(req.client) {
            return Some(bare_error(
                403,
                "forbidden_client",
                "client network not allowed",
            ));
        }
        if req.path.starts_with("/mcp") && !is_loopback_client(req.client) {
            return Some(bare_error(
                403,
                "forbidden_client",
                "MCP is only served on loopback",
            ));
        }
    }
    let host = req.host.as_deref().unwrap_or("").to_lowercase();
    if !policy.hosts.contains(&host) {
        return Some(bare_error(
            421,
            "forbidden_host",
            &format!("host {} not allowed", py_repr(&host)),
        ));
    }
    let protected = (req.path.starts_with("/api/") || req.path.starts_with("/mcp"))
        && !PUBLIC_PATHS.contains(&req.path.as_str());
    if !protected {
        return None;
    }
    if let Some(origin) = req.origin.as_deref().filter(|o| !o.is_empty())
        && !SAFE_METHODS.contains(&req.method.as_str())
        && !policy.origins.contains(origin.trim_end_matches('/'))
    {
        return Some(bare_error(
            403,
            "forbidden_origin",
            &format!("origin {origin} not allowed"),
        ));
    }
    if !authorized(policy, req) {
        return Some(
            bare_error(401, "unauthorized", "missing or invalid API token")
                .with_header("www-authenticate", "Bearer"),
        );
    }
    None
}

/// `device_cookie_value(token)`: HMAC-SHA256 of `workgraph-paired-device` keyed by the token, in
/// lower-case hex.
pub fn device_cookie_value(token: &str) -> String {
    let digest = <Hmac<Sha256> as Mac>::new_from_slice(token.as_bytes())
        .map(|mac| {
            mac.chain_update(b"workgraph-paired-device")
                .finalize()
                .into_bytes()
        })
        .map(|bytes| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>());
    // An HMAC key of any length is accepted, so the error arm cannot happen.
    digest.unwrap_or_default()
}

/// One cookie from a `Cookie` header, as Starlette's `cookie_parser` reads it: `;`-separated
/// `name=value` pairs, trimmed, the last one of a name winning, quotes removed.
pub fn cookie(header: Option<&str>, name: &str) -> Option<String> {
    header?.split(';').rev().find_map(|chunk| {
        let (key, value) = match chunk.split_once('=') {
            Some((key, value)) => (key.trim(), value.trim()),
            None => ("", chunk.trim()),
        };
        (key == name).then(|| unquote_cookie(value))
    })
}

/// `http.cookies._unquote` for the values Workgraph sets: a quoted value loses its quotes and
/// backslash escapes.
fn unquote_cookie(value: &str) -> String {
    match value
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
    {
        Some(inner) if value.len() >= 2 => {
            let chars: Vec<char> = inner.chars().collect();
            let mut out = String::new();
            let mut index = 0;
            while index < chars.len() {
                if chars[index] == '\\' && index + 1 < chars.len() {
                    out.push(chars[index + 1]);
                    index += 2;
                } else {
                    out.push(chars[index]);
                    index += 1;
                }
            }
            out
        }
        _ => value.to_owned(),
    }
}

/// `device_paired`: the request carries the device cookie of this token.
pub fn device_paired(req: &Incoming, token: &str) -> bool {
    cookie(req.cookie.as_deref(), DEVICE_COOKIE).is_some_and(|supplied| {
        !supplied.is_empty() && secrets_equal(&supplied, &device_cookie_value(token))
    })
}

/// `trusted_browser`: without LAN serving every browser is trusted; with it, loopback clients and
/// paired devices are.
pub fn trusted_browser(policy: &Policy, req: &Incoming) -> bool {
    policy.lan.is_none() || is_loopback_client(req.client) || device_paired(req, &policy.token)
}

/// The `Set-Cookie` value that pairs a device, as Starlette's `set_cookie` writes it.
pub fn device_set_cookie(token: &str) -> String {
    format!(
        "{DEVICE_COOKIE}={}; HttpOnly; Max-Age={DEVICE_COOKIE_MAX_AGE}; Path=/; SameSite=strict",
        device_cookie_value(token)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(lan: Option<Lan>) -> Policy {
        Policy::new("secret", "127.0.0.1", 8765, lan, None, None)
    }

    fn get(path: &str) -> Incoming {
        Incoming {
            method: "GET".into(),
            path: path.into(),
            host: Some("127.0.0.1:8765".into()),
            client: Ip::parse("127.0.0.1"),
            ..Incoming::default()
        }
    }

    fn status(reply: Option<Reply>) -> Option<u16> {
        reply.map(|r| r.status)
    }

    #[test]
    fn hosts_and_origins() {
        let hosts = allowed_hosts("127.0.0.1", 8765, None, Some(" a.test , ,b.test"));
        assert!(hosts.contains("localhost:8765") && hosts.contains("[::1]"));
        assert!(hosts.contains("a.test") && hosts.contains("b.test:8765"));
        let origins = allowed_origins("127.0.0.1", 8765, None, Some("http://dev.test:5173/"));
        assert!(origins.contains("http://localhost:8765"));
        assert!(origins.contains("http://dev.test:5173"));
    }

    #[test]
    fn screens_like_the_middleware() {
        let p = policy(None);
        assert_eq!(status(screen(&p, &get("/api/v1/health"))), None);
        assert_eq!(status(screen(&p, &get("/api/v1/health/"))), Some(401));
        assert_eq!(status(screen(&p, &get("/"))), None);
        assert_eq!(status(screen(&p, &get("/mcpx"))), Some(401));
        let foreign = Incoming {
            host: Some("Evil.Example".into()),
            ..get("/pair")
        };
        let reply = screen(&p, &foreign).unwrap();
        assert_eq!(reply.status, 421);
        assert_eq!(
            String::from_utf8(reply.body).unwrap(),
            r#"{"error":{"code":"forbidden_host","message":"host 'evil.example' not allowed"}}"#
        );
        let bearer = Incoming {
            authorization: Some("BEARER  secret ".into()),
            ..get("/api/x")
        };
        assert_eq!(status(screen(&p, &bearer)), None);
        let header = Incoming {
            workgraph_token: Some("secret".into()),
            ..get("/api/x")
        };
        assert_eq!(status(screen(&p, &header)), None);
        let query = Incoming {
            query: "token=nope&token=secret".into(),
            ..get("/api/v1/ws/a/events/stream")
        };
        assert_eq!(status(screen(&p, &query)), None);
        let elsewhere = Incoming {
            query: "token=secret".into(),
            ..get("/api/v1/ws/a/events")
        };
        assert_eq!(status(screen(&p, &elsewhere)), Some(401));
        let origin = Incoming {
            method: "POST".into(),
            origin: Some("http://evil.example".into()),
            ..bearer.clone()
        };
        assert_eq!(status(screen(&p, &origin)), Some(403));
        let own = Incoming {
            origin: Some("http://127.0.0.1:8765/".into()),
            ..origin.clone()
        };
        assert_eq!(status(screen(&p, &own)), None);
        let safe = Incoming {
            method: "GET".into(),
            ..origin
        };
        assert_eq!(status(screen(&p, &safe)), None);
    }

    #[test]
    fn lan_clients() {
        let lan = Lan::build("192.168.1.20", &["192.168.1.0/24".into()], "Box").unwrap();
        assert!(lan.client_allowed(Ip::parse("192.168.1.7")));
        assert!(lan.client_allowed(Ip::parse("127.0.0.1")));
        assert!(lan.client_allowed(Ip::parse("::ffff:192.168.1.7")));
        assert!(!lan.client_allowed(Ip::parse("10.0.0.5")));
        assert!(!lan.client_allowed(None));
        assert_eq!(lan.hosts, vec!["192.168.1.20", "box", "box.local"]);
        assert!(Lan::build("192.168.1.20", &["nope".into()], "box").is_err());
        let p = policy(Some(lan));
        let phone = Incoming {
            host: Some("192.168.1.20:8765".into()),
            client: Ip::parse("192.168.1.42"),
            ..get("/mcp/")
        };
        let reply = screen(&p, &phone).unwrap();
        assert_eq!(reply.status, 403);
        let outsider = Incoming {
            client: Ip::parse("203.0.113.9"),
            ..phone.clone()
        };
        assert_eq!(status(screen(&p, &outsider)), Some(403));
        assert!(!trusted_browser(&p, &phone));
        let paired = Incoming {
            cookie: Some(format!(
                "a=b; {DEVICE_COOKIE}={}",
                device_cookie_value("secret")
            )),
            ..phone
        };
        assert!(trusted_browser(&p, &paired));
    }

    #[test]
    fn device_cookie_is_the_token_hmac() {
        // hmac.new(b"secret", b"workgraph-paired-device", hashlib.sha256).hexdigest()
        assert_eq!(
            device_cookie_value("secret"),
            "0709f80b696bdcb7171888c267084a5247981f8553e07ce23a8658b26352c2f8"
        );
        assert_eq!(cookie(Some("x=1; x=\"2\""), "x").as_deref(), Some("2"));
    }
}
