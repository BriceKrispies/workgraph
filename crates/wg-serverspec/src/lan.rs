//! Serving the UI on the local network (IF-41): whether `serve` also listens on a LAN address,
//! which address that is, and the lines it prints at start (`cli/commands/servers.py`).
//!
//! The shell supplies the facts: the probe's source address and the configuration. The decisions
//! and the text are here.

use crate::net::{Ip, private_networks};

/// `_is_loopback(host)`: `localhost`, or a loopback IP address.
pub fn is_loopback_host(host: &str) -> bool {
    host == "localhost" || Ip::parse(host).is_some_and(Ip::is_loopback)
}

/// The address `detect_lan_address` returns for the source address the OS picked for a probe:
/// the dotted IPv4 text when it is in a private range, else nothing. The probe sends no packets,
/// so `source` is the address of the interface holding the default route.
pub fn lan_address_from_source(source: Option<Ip>) -> Option<String> {
    match source {
        Some(Ip::V4(bits))
            if private_networks()
                .iter()
                .any(|net| net.contains(Ip::V4(bits))) =>
        {
            let [a, b, c, d] = bits.to_be_bytes();
            Some(format!("{a}.{b}.{c}.{d}"))
        }
        _ => None,
    }
}

/// What `serve` does about the local network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LanPlan {
    /// Loopback only, by configuration or because the server binds a non-loopback host.
    Off,
    /// LAN serving is on, but no private address was found or configured.
    NoAddress,
    /// Also listen on this address, on the same port.
    Serve(String),
}

/// `if (lan) and _is_loopback(bind_host)`, then `config.lan_address or detect_lan_address()`.
/// `detect` runs only when it is needed, as in Python.
pub fn lan_plan(
    enabled: bool,
    bind_host: &str,
    configured: Option<&str>,
    detect: impl FnOnce() -> Option<String>,
) -> LanPlan {
    if !enabled || !is_loopback_host(bind_host) {
        return LanPlan::Off;
    }
    match configured
        .filter(|address| !address.is_empty())
        .map(str::to_owned)
        .or_else(detect)
    {
        Some(address) => LanPlan::Serve(address),
        None => LanPlan::NoAddress,
    }
}

/// The warning printed when LAN serving is on but no address was found.
pub const LOOPBACK_ONLY_NOTICE: &str =
    "No private local-network address found; serving on loopback only.";

/// `Workgraph listening on http://<host>:<port>/  (API token: <token file>)`.
pub fn listening_notice(bind_host: &str, port: u16, token_file: &str) -> String {
    format!("Workgraph listening on http://{bind_host}:{port}/  (API token: {token_file})")
}

/// `Local network: http://<address>:<port>/  pairing code <code> (enter it once per device)`.
pub fn local_network_notice(address: &str, port: u16, code: &str) -> String {
    format!(
        "Local network: http://{address}:{port}/  pairing code {code} (enter it once per device)"
    )
}

/// [`local_network_notice`] as console markup: the code in bold, as `serve` prints it.
pub fn local_network_markup(address: &str, port: u16, code: &str) -> String {
    format!(
        "Local network: http://{address}:{port}/  pairing code [bold]{code}[/] (enter it once per device)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(text: &str) -> Option<Ip> {
        Ip::parse(text)
    }

    #[test]
    fn loopback_hosts() {
        assert!(is_loopback_host("127.0.0.1"));
        assert!(is_loopback_host("127.0.0.2"));
        assert!(is_loopback_host("::1"));
        assert!(is_loopback_host("localhost"));
        assert!(!is_loopback_host("0.0.0.0"));
        assert!(!is_loopback_host("192.168.1.10"));
        assert!(!is_loopback_host("example.org"));
    }

    #[test]
    fn detected_source_must_be_private_ipv4() {
        assert_eq!(
            lan_address_from_source(v4("192.168.1.20")),
            Some("192.168.1.20".to_owned())
        );
        assert_eq!(
            lan_address_from_source(v4("10.8.0.5")),
            Some("10.8.0.5".to_owned())
        );
        assert_eq!(
            lan_address_from_source(v4("172.20.1.1")),
            Some("172.20.1.1".to_owned())
        );
        assert_eq!(lan_address_from_source(v4("8.8.8.8")), None);
        assert_eq!(lan_address_from_source(v4("127.0.0.1")), None);
        assert_eq!(lan_address_from_source(v4("169.254.1.1")), None);
        assert_eq!(lan_address_from_source(v4("fd00::1")), None);
        assert_eq!(lan_address_from_source(None), None);
    }

    #[test]
    fn plan_is_off_without_lan_or_on_a_remote_bind() {
        let never = || panic!("detection must not run");
        assert_eq!(lan_plan(false, "127.0.0.1", None, never), LanPlan::Off);
        assert_eq!(lan_plan(true, "0.0.0.0", None, never), LanPlan::Off);
    }

    #[test]
    fn plan_prefers_configured_address_and_skips_detection() {
        let never = || panic!("detection must not run");
        assert_eq!(
            lan_plan(true, "127.0.0.1", Some("192.168.1.9"), never),
            LanPlan::Serve("192.168.1.9".to_owned())
        );
    }

    #[test]
    fn plan_uses_detection_when_unconfigured_or_empty() {
        let found = || Some("192.168.1.20".to_owned());
        assert_eq!(
            lan_plan(true, "localhost", None, found),
            LanPlan::Serve("192.168.1.20".to_owned())
        );
        assert_eq!(
            lan_plan(true, "127.0.0.1", Some(""), found),
            LanPlan::Serve("192.168.1.20".to_owned())
        );
        assert_eq!(
            lan_plan(true, "127.0.0.1", None, || None),
            LanPlan::NoAddress
        );
    }

    #[test]
    fn start_up_lines_match_python() {
        assert_eq!(
            listening_notice("127.0.0.1", 8765, "/home/u/.workgraph/api-token"),
            "Workgraph listening on http://127.0.0.1:8765/  (API token: /home/u/.workgraph/api-token)"
        );
        assert_eq!(
            local_network_notice("192.168.1.20", 8765, "ABCD-EFGH"),
            "Local network: http://192.168.1.20:8765/  pairing code ABCD-EFGH (enter it once per device)"
        );
        assert_eq!(
            LOOPBACK_ONLY_NOTICE,
            "No private local-network address found; serving on loopback only."
        );
    }
}
