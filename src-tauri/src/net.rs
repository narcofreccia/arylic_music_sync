//! Local network helpers shared by the poller (M2) and the subnet sweep (M3).
//!
//! Nothing here talks to a device — it only answers "which LAN am I on?", which
//! is the input the discovery sweep needs and the debug pane likes to show.

use std::net::{IpAddr, Ipv4Addr};

/// Best-effort discovery of this machine's LAN IPv4 (the interface serving the
/// default route), via a *connected* UDP socket — no packets are actually sent,
/// the kernel just resolves which local address it would use. This beats
/// enumerating interfaces: it picks the one that actually routes, on every OS.
pub fn local_ipv4() -> Option<Ipv4Addr> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    // 8.8.8.8 is never contacted; it is just a routable address to resolve
    // against, so this works with no internet connection (NFR-1: LAN-only).
    sock.connect("8.8.8.8:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_loopback() => Some(v4),
        _ => None,
    }
}

/// The /24 the given address sits in, in CIDR form (`192.168.10.0/24`).
///
/// M3's sweep defaults to this when `settings.subnet` is unset (FR-4). A /24 is
/// the pragmatic assumption for a home LAN: 254 probes is a fast sweep, whereas
/// honouring a real /16 netmask would be 65k.
pub fn derive_cidr24(ip: Ipv4Addr) -> String {
    let o = ip.octets();
    format!("{}.{}.{}.0/24", o[0], o[1], o[2])
}

/// Auto-detected sweep range for FR-4, or `None` when offline.
#[allow(dead_code)] // wired up by M3 (discovery)
pub fn local_cidr24() -> Option<String> {
    local_ipv4().map(derive_cidr24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr24_zeroes_the_host_octet() {
        assert_eq!(derive_cidr24(Ipv4Addr::new(192, 168, 10, 47)), "192.168.10.0/24");
        assert_eq!(derive_cidr24(Ipv4Addr::new(10, 0, 0, 1)), "10.0.0.0/24");
    }
}
