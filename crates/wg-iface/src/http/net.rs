//! IP addresses and networks as Python's `ipaddress` module reads them, for the LAN client rules
//! (`api/security.py`): `ip_address(text)`, `ip_network(text, strict=False)`, `is_loopback`,
//! `ipv4_mapped` and membership.
//!
//! The shell turns the socket's peer address into an [`Ip`] directly; only configuration text
//! (`server.lan_networks`, `server.lan_address`) is parsed here.

/// An IPv4 or IPv6 address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ip {
    V4(u32),
    V6(u128),
}

/// The IPv4-mapped prefix `::ffff:0:0/96`.
const MAPPED_PREFIX: u128 = 0xffff_0000_0000;

impl Ip {
    /// `ipaddress.ip_address(text)`: an IPv4 address in strict dotted decimal, else an IPv6
    /// address. A scope id (`%eth0`) is accepted on IPv6 and ignored.
    pub fn parse(text: &str) -> Option<Ip> {
        parse_v4(text)
            .map(Ip::V4)
            .or_else(|| parse_v6(text).map(Ip::V6))
    }

    /// `is_loopback` (Python 3.12): `127.0.0.0/8`, or exactly `::1`.
    pub fn is_loopback(self) -> bool {
        match self {
            Ip::V4(bits) => bits >> 24 == 127,
            Ip::V6(bits) => bits == 1,
        }
    }

    /// `ipv4_mapped`: the IPv4 address inside `::ffff:a.b.c.d`.
    pub fn ipv4_mapped(self) -> Option<Ip> {
        match self {
            Ip::V6(bits) if bits >> 32 == MAPPED_PREFIX >> 32 => {
                u32::try_from(bits & 0xffff_ffff).ok().map(Ip::V4)
            }
            _ => None,
        }
    }
}

/// An IP network: a masked address and a prefix length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Network {
    pub address: Ip,
    pub prefix: u8,
}

fn v4_mask(prefix: u8) -> u32 {
    match prefix {
        0 => 0,
        p => u32::MAX << (32 - u32::from(p.min(32))),
    }
}

fn v6_mask(prefix: u8) -> u128 {
    match prefix {
        0 => 0,
        p => u128::MAX << (128 - u32::from(p.min(128))),
    }
}

impl Network {
    /// `ipaddress.ip_network(text, strict=False)`: `address[/prefix]`, with host bits cleared. An
    /// IPv4 prefix may also be a dotted netmask (`/255.255.0.0`) or hostmask (`/0.0.255.255`).
    pub fn parse(text: &str) -> Option<Network> {
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text, None),
        };
        if let Some(bits) = parse_v4(address) {
            let prefix = match prefix {
                None => 32,
                Some(text) => v4_prefix(text)?,
            };
            return Some(Network {
                address: Ip::V4(bits & v4_mask(prefix)),
                prefix,
            });
        }
        if address.contains('%') {
            return None;
        }
        let bits = parse_v6(address)?;
        let prefix = match prefix {
            None => 128,
            Some(text) => decimal_prefix(text, 128)?,
        };
        Some(Network {
            address: Ip::V6(bits & v6_mask(prefix)),
            prefix,
        })
    }

    /// `address in network`; an address of the other version is never inside.
    pub fn contains(&self, ip: Ip) -> bool {
        match (self.address, ip) {
            (Ip::V4(net), Ip::V4(bits)) => bits & v4_mask(self.prefix) == net,
            (Ip::V6(net), Ip::V6(bits)) => bits & v6_mask(self.prefix) == net,
            _ => false,
        }
    }
}

/// `PRIVATE_NETWORKS`: RFC 1918 and the IPv6 unique-local range, the addresses a home or office
/// LAN hands out.
pub fn private_networks() -> Vec<Network> {
    ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "fc00::/7"]
        .iter()
        .filter_map(|text| Network::parse(text))
        .collect()
}

/// A decimal prefix length no larger than `max`, without sign or leading zeros beyond `0`.
fn decimal_prefix(text: &str, max: u8) -> Option<u8> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: u32 = text.parse().ok()?;
    u8::try_from(value).ok().filter(|prefix| *prefix <= max)
}

/// An IPv4 prefix: a length, a netmask (leading ones) or a hostmask (leading zeros).
fn v4_prefix(text: &str) -> Option<u8> {
    if let Some(prefix) = decimal_prefix(text, 32) {
        return Some(prefix);
    }
    let mask = parse_v4(text)?;
    let ones = mask.leading_ones();
    if mask.checked_shl(ones).unwrap_or(0) == 0 {
        return u8::try_from(ones).ok();
    }
    let zeros = mask.leading_zeros();
    let host = !mask;
    if host.checked_shl(zeros).unwrap_or(0) == 0 {
        return u8::try_from(zeros).ok();
    }
    None
}

/// Strict dotted decimal: four octets of one to three digits, no leading zero, at most 255.
fn parse_v4(text: &str) -> Option<u32> {
    let octets: Vec<&str> = text.split('.').collect();
    if octets.len() != 4 {
        return None;
    }
    octets.iter().try_fold(0u32, |acc, octet| {
        let digits_ok = !octet.is_empty()
            && octet.len() <= 3
            && octet.bytes().all(|b| b.is_ascii_digit())
            && !(octet.len() > 1 && octet.starts_with('0'));
        if !digits_ok {
            return None;
        }
        let value: u32 = octet.parse().ok()?;
        (value <= 255).then_some((acc << 8) | value)
    })
}

/// One group of an IPv6 address: one to four hex digits.
fn hex_group(text: &str) -> Option<u128> {
    if text.is_empty() || text.len() > 4 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u128::from_str_radix(text, 16).ok()
}

/// The groups of one side of `::`, as 16-bit values; a trailing dotted IPv4 counts as two.
fn v6_groups(text: &str, last_side: bool) -> Option<Vec<u128>> {
    if text.is_empty() {
        return Some(Vec::new());
    }
    let parts: Vec<&str> = text.split(':').collect();
    let count = parts.len();
    parts
        .iter()
        .enumerate()
        .try_fold(Vec::new(), |groups, (index, part)| {
            let tail = last_side && index + 1 == count && part.contains('.');
            let added: Vec<u128> = if tail {
                let bits = u128::from(parse_v4(part)?);
                vec![bits >> 16, bits & 0xffff]
            } else {
                vec![hex_group(part)?]
            };
            Some(groups.into_iter().chain(added).collect())
        })
}

/// An IPv6 address: up to eight groups, at most one `::`, an optional trailing dotted IPv4, and
/// an optional scope id.
fn parse_v6(text: &str) -> Option<u128> {
    let text = match text.split_once('%') {
        Some((address, scope)) if !scope.is_empty() => address,
        Some(_) => return None,
        None => text,
    };
    if !text.contains(':') {
        return None;
    }
    let groups = match text.split_once("::") {
        Some((head, tail)) => {
            if tail.contains("::") {
                return None;
            }
            let head = v6_groups(head, false)?;
            let tail = v6_groups(tail, true)?;
            if head.len() + tail.len() > 7 {
                return None;
            }
            let zeros = vec![0u128; 8 - head.len() - tail.len()];
            head.into_iter()
                .chain(zeros)
                .chain(tail)
                .collect::<Vec<u128>>()
        }
        None => v6_groups(text, true)?,
    };
    if groups.len() != 8 {
        return None;
    }
    Some(groups.iter().fold(0u128, |acc, group| (acc << 16) | group))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_addresses_like_ipaddress() {
        assert_eq!(Ip::parse("192.168.1.20"), Some(Ip::V4(0xc0a8_0114)));
        assert_eq!(Ip::parse("01.2.3.4"), None);
        assert_eq!(Ip::parse("256.1.1.1"), None);
        assert_eq!(Ip::parse("testclient"), None);
        assert_eq!(Ip::parse("::1"), Some(Ip::V6(1)));
        assert_eq!(Ip::parse("::"), Some(Ip::V6(0)));
        assert_eq!(
            Ip::parse("::ffff:127.0.0.1").and_then(Ip::ipv4_mapped),
            Some(Ip::V4(0x7f00_0001))
        );
        assert_eq!(Ip::parse("fe80::1%eth0"), Ip::parse("fe80::1"));
        assert_eq!(Ip::parse("1:2:3:4:5:6:7:8:9"), None);
        assert_eq!(Ip::parse("1::2::3"), None);
        assert!(Ip::parse("127.8.9.10").is_some_and(Ip::is_loopback));
        assert!(!Ip::parse("::ffff:127.0.0.1").is_some_and(Ip::is_loopback));
    }

    #[test]
    fn networks_clear_host_bits_and_contain() {
        let net = Network::parse("192.168.1.77/24").unwrap();
        assert_eq!(net.address, Ip::V4(0xc0a8_0100));
        assert!(net.contains(Ip::parse("192.168.1.7").unwrap()));
        assert!(!net.contains(Ip::parse("10.0.0.5").unwrap()));
        let mask = Network::parse("10.1.2.3/255.255.0.0").unwrap();
        assert_eq!(mask.prefix, 16);
        assert!(Network::parse("10.0.0.0/33").is_none());
        let ula = Network::parse("fc00::/7").unwrap();
        assert!(ula.contains(Ip::parse("fd12::1").unwrap()));
        assert!(!ula.contains(Ip::parse("fe80::1").unwrap()));
        assert_eq!(private_networks().len(), 4);
    }
}
