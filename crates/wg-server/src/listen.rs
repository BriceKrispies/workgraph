//! The sockets `serve` binds (IF-41): the loopback listener and, when LAN serving is on, a second
//! listener on the same port at the LAN address. Also the LAN address probe (`detect_lan_address`)
//! and the computer's name (`socket.gethostname()`).

use std::io;
use std::net::{IpAddr, UdpSocket};

use tokio::net::TcpListener;
use wg_iface::http::lan::lan_address_from_source;
use wg_iface::http::net::Ip;

/// `detect_lan_address()` with the probe injected: `probe` yields the source address the OS would
/// use towards the network, and the result is that address when it is private.
pub fn detect_lan_address_with(probe: impl FnOnce() -> io::Result<IpAddr>) -> Option<String> {
    let source = probe().ok().map(|address| match address {
        IpAddr::V4(v4) => Ip::V4(u32::from(v4)),
        IpAddr::V6(v6) => Ip::V6(u128::from(v6)),
    });
    lan_address_from_source(source)
}

/// `detect_lan_address()`: connecting a UDP socket sends no packets; it only asks the OS which
/// source address it would use (TEST-NET-1 is never routed).
pub fn detect_lan_address() -> Option<String> {
    detect_lan_address_with(default_route_source)
}

fn default_route_source() -> io::Result<IpAddr> {
    let probe = UdpSocket::bind(("0.0.0.0", 0))?;
    probe.connect(("192.0.2.1", 9))?;
    Ok(probe.local_addr()?.ip())
}

/// The listeners for `serve`: the bound host and, when `lan_address` is set, the same port on the
/// LAN address. A bind failure names the address, as `_bind` does.
pub async fn bind_listeners(
    host: &str,
    port: u16,
    lan_address: Option<&str>,
) -> io::Result<Vec<TcpListener>> {
    let mut listeners = vec![bind(host, port).await?];
    if let Some(address) = lan_address {
        listeners.push(bind(address, port).await?);
    }
    Ok(listeners)
}

async fn bind(host: &str, port: u16) -> io::Result<TcpListener> {
    let listener =
        std::net::TcpListener::bind((host.trim_matches(['[', ']']), port)).map_err(|err| {
            io::Error::new(err.kind(), format!("cannot listen on {host}:{port}: {err}"))
        })?;
    listener.set_nonblocking(true)?;
    TcpListener::from_std(listener)
}

/// `socket.gethostname()`: the computer's name from the environment, else `/etc/hostname`, else
/// `localhost`.
pub fn host_name() -> String {
    ["COMPUTERNAME", "HOSTNAME"]
        .iter()
        .filter_map(|var| std::env::var(var).ok())
        .chain(std::fs::read_to_string("/etc/hostname").ok())
        .map(|name| name.trim().to_owned())
        .find(|name| !name.is_empty())
        .unwrap_or_else(|| "localhost".to_owned())
}
