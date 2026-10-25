//! `ipaddress.ip_network(text, strict=False)` acceptance, for the `lan_networks` entries.
//!
//! Written here because `wg-config` is a Pure crate and may not name the standard library's
//! networking module. Accepts an IPv4 or IPv6 address with an optional prefix length or IPv4
//! netmask, as Python does; host bits are not checked because `strict=False` masks them away.

/// Whether `text` is an IPv4 or IPv6 network in Python's `ipaddress` notation.
pub(crate) fn is_network(text: &str) -> bool {
    let (address, prefix) = match text.split_once('/') {
        Some((address, prefix)) => (address, Some(prefix)),
        None => (text, None),
    };
    if address.contains(':') {
        is_v6(address) && prefix.is_none_or(|p| is_prefix(p, 128))
    } else {
        is_v4(address) && prefix.is_none_or(|p| is_prefix(p, 32) || is_netmask(p))
    }
}

fn is_prefix(text: &str, max: u32) -> bool {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if text.len() > 1 && text.starts_with('0') {
        return false;
    }
    text.parse::<u32>().is_ok_and(|value| value <= max)
}

/// A dotted IPv4 netmask whose bits are a run of ones followed by zeros.
fn is_netmask(text: &str) -> bool {
    match ipv4_octets(text) {
        Some(octets) => {
            let bits = u32::from_be_bytes(octets);
            let ones = bits.leading_ones();
            bits.checked_shl(ones).unwrap_or(0) == 0
        }
        None => false,
    }
}

fn is_v4(text: &str) -> bool {
    ipv4_octets(text).is_some()
}

fn ipv4_octets(text: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut octets = [0u8; 4];
    for (slot, part) in octets.iter_mut().zip(parts) {
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if part.len() > 1 && part.starts_with('0') {
            return None;
        }
        *slot = part.parse::<u8>().ok()?;
    }
    Some(octets)
}

fn is_v6(text: &str) -> bool {
    // An embedded IPv4 tail counts as two 16-bit groups.
    let (head, tail_groups) = match text.rfind(':') {
        Some(index) if text[index + 1..].contains('.') => {
            if !is_v4(&text[index + 1..]) {
                return false;
            }
            let head = &text[..=index];
            let head = if head.ends_with("::") {
                head
            } else {
                &head[..head.len() - 1]
            };
            (head, 2usize)
        }
        _ => (text, 0usize),
    };
    let count_groups = |part: &str| -> Option<usize> {
        if part.is_empty() {
            return Some(0);
        }
        let mut count = 0;
        for group in part.split(':') {
            if group.is_empty() || group.len() > 4 || !group.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return None;
            }
            count += 1;
        }
        Some(count)
    };
    let halves: Vec<&str> = head.split("::").collect();
    match halves.as_slice() {
        [whole] => count_groups(whole).map(|n| n + tail_groups) == Some(8),
        [left, right] => match (count_groups(left), count_groups(right)) {
            (Some(l), Some(r)) => l + r + tail_groups < 8,
            _ => false,
        },
        _ => false,
    }
}
