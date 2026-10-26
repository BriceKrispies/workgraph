//! LAN listener checks (IF-41): address detection with injected probes, the two listeners `serve`
//! binds, and requests over real sockets through each of them. No real LAN address is bound: the
//! "LAN" listener uses another loopback address, and every port is free.

use std::error::Error;
use std::io::{self, Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener as StdListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use wg_runtime::App;
use wg_server::{
    LanOptions, Server, ServerOptions, Ui, bind_listeners, detect_lan_address_with, serve,
};

type TestResult = Result<(), Box<dyn Error>>;

/// A loopback address other than 127.0.0.1, standing in for the LAN address.
const STAND_IN_LAN: &str = "127.0.0.2";

fn scratch(name: &str) -> Result<PathBuf, Box<dyn Error>> {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("if41")
        .join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn probe(address: IpAddr) -> impl FnOnce() -> io::Result<IpAddr> {
    move || Ok(address)
}

#[test]
fn detection_keeps_only_private_source_addresses() {
    let v4 = |a, b, c, d| IpAddr::V4(Ipv4Addr::new(a, b, c, d));
    assert_eq!(
        detect_lan_address_with(probe(v4(192, 168, 1, 20))),
        Some("192.168.1.20".to_owned())
    );
    assert_eq!(
        detect_lan_address_with(probe(v4(10, 8, 0, 5))),
        Some("10.8.0.5".to_owned())
    );
    assert_eq!(
        detect_lan_address_with(probe(v4(172, 31, 0, 9))),
        Some("172.31.0.9".to_owned())
    );
    assert_eq!(detect_lan_address_with(probe(v4(8, 8, 8, 8))), None);
    assert_eq!(detect_lan_address_with(probe(v4(127, 0, 0, 1))), None);
    assert_eq!(
        detect_lan_address_with(probe("fd12::7".parse().expect("ipv6"))),
        None
    );
    let failing = || Err(io::Error::other("no route"));
    assert_eq!(detect_lan_address_with(failing), None);
}

#[test]
fn real_probe_yields_nothing_or_a_private_address() {
    // Sends no packets; the result depends on this machine's network, so only its shape is checked.
    if let Some(address) = wg_server::detect_lan_address() {
        let octets: Vec<u8> = address
            .split('.')
            .map(|part| part.parse().expect("dotted quad"))
            .collect();
        let private = matches!(
            octets.as_slice(),
            [10, ..] | [172, 16..=31, ..] | [192, 168, ..]
        );
        assert!(private, "{address} is not a private address");
    }
}

#[tokio::test]
async fn binds_loopback_and_lan_listeners_on_one_port() -> TestResult {
    let free = StdListener::bind(("127.0.0.1", 0))?;
    let port = free.local_addr()?.port();
    drop(free);
    let listeners = bind_listeners("127.0.0.1", port, Some(STAND_IN_LAN)).await?;
    assert_eq!(listeners.len(), 2);
    assert_eq!(listeners[1].local_addr()?.port(), port);
    assert_eq!(listeners[1].local_addr()?.ip().to_string(), STAND_IN_LAN);
    drop(listeners);
    let only = bind_listeners("127.0.0.1", port, None).await?;
    assert_eq!(only.len(), 1);
    Ok(())
}

#[tokio::test]
async fn bind_failure_names_the_address() -> TestResult {
    let taken = StdListener::bind(("127.0.0.1", 0))?;
    let port = taken.local_addr()?.port();
    let error = match bind_listeners("127.0.0.1", port, None).await {
        Ok(_) => return Err("second bind on a taken port succeeded".into()),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .starts_with(&format!("cannot listen on 127.0.0.1:{port}: "))
    );
    Ok(())
}

/// One HTTP/1.1 request over a blocking socket; returns the status line and headers.
fn request_head(address: &str, port: u16, path: &str, host: &str) -> io::Result<String> {
    let mut stream = TcpStream::connect((address, port))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes())?;
    let mut text = String::new();
    stream.read_to_string(&mut text)?;
    Ok(text.lines().next().unwrap_or_default().to_owned())
}

#[tokio::test]
async fn lan_listener_serves_the_lan_name_and_loopback_serves_its_own() -> TestResult {
    let free = StdListener::bind(("127.0.0.1", 0))?;
    let port = free.local_addr()?.port();
    drop(free);
    let home = scratch("sockets")?;
    let ui = scratch("sockets-ui-missing")?;
    let options = ServerOptions {
        host: "127.0.0.1".to_owned(),
        port,
        lan: Some(LanOptions {
            address: STAND_IN_LAN.to_owned(),
            networks: Vec::new(),
            hostname: "testbox".to_owned(),
            pairing_code: Some("ABCD-EFGH".to_owned()),
        }),
        ui: Ui::Dir(ui),
        mcp: None,
        routes: Vec::new(),
    };
    let server = Arc::new(Server::new(options, App::open(Some(&home))?)?);
    let listeners = bind_listeners("127.0.0.1", port, Some(STAND_IN_LAN)).await?;
    let task = tokio::spawn(serve(server, listeners));

    let lan_host = format!("{STAND_IN_LAN}:{port}");
    let loopback_host = format!("127.0.0.1:{port}");
    let (lan, loopback, unknown) = tokio::task::spawn_blocking(move || -> io::Result<_> {
        Ok((
            request_head(STAND_IN_LAN, port, "/pair", &lan_host)?,
            request_head("127.0.0.1", port, "/pair", &loopback_host)?,
            request_head("127.0.0.1", port, "/pair", &format!("192.168.1.10:{port}"))?,
        ))
    })
    .await??;
    task.abort();

    // Loopback clients are trusted, so `/pair` redirects to `/` on both listeners.
    assert!(lan.starts_with("HTTP/1.1 303"), "{lan}");
    assert!(loopback.starts_with("HTTP/1.1 303"), "{loopback}");
    // A host that is neither loopback nor the LAN address is refused.
    assert!(unknown.starts_with("HTTP/1.1 421"), "{unknown}");
    Ok(())
}
