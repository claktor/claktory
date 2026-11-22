//! System DNS configuration on iOS, iPadOS, tvOS and watchOS, from libresolv.
//!
//! Claktory's copy only (vendor/README.md). Upstream reads SCDynamicStore on
//! every Apple platform (apple.rs), but SCDynamicStore is API on macOS only
//! (`API_UNAVAILABLE(ios, tvos, watchos)`), and the App Store takes an app
//! that links it on iOS for one using private API. libresolv is public on all
//! of them (`<resolv.h>`, `libresolv.tbd` in each SDK): `res_ninit` fills its
//! state from the system's live resolver configuration, the same default
//! resolver `State:/Network/Global/DNS` holds, and `res_getservers` copies its
//! nameservers out. Apple's libresolv (res_init.c) fills it from the system's
//! DNS configuration (`dns_configuration_copy`) first, and only then from
//! resolv.conf.
//!
//! Search domains aren't read: iroh looks up only fully qualified names (its
//! relays and DNS address lookup). When libresolv gives nothing, the caller
//! falls back exactly as it does when SystemConfiguration does
//! (`read_system`): iroh's public resolvers, never no DNS.

use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
};

use super::{Config, DnsProtocol, Hosts, Nameserver};

/// `struct __res_state` (`res_9_state`): about 550 bytes on 64-bit Apple
/// targets; kept far larger and 8-aligned, zeroed before `res_ninit`.
#[repr(C, align(8))]
struct ResState([u8; 4096]);

/// `union res_9_sockaddr_union`: a `sockaddr_in`, a `sockaddr_in6`, an
/// `int64_t` or 128 bytes.
#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct SockaddrUnion([u8; 128]);

#[link(name = "resolv")]
unsafe extern "C" {
    fn res_9_ninit(state: *mut ResState) -> i32;
    fn res_9_getservers(state: *mut ResState, set: *mut SockaddrUnion, count: i32) -> i32;
    fn res_9_ndestroy(state: *mut ResState);
}

/// BSD `sockaddr` families (sys/socket.h).
const AF_INET: u8 = 2;
const AF_INET6: u8 = 30;

/// At most this many nameservers, as `MAXNS` in resolv.h (3) with room to spare.
const MAX_SERVERS: usize = 8;

/// Reads the default resolver's nameservers through libresolv.
///
/// Also reads the hosts file.
pub(super) fn read_system_dns() -> Result<Config, io::Error> {
    let mut state = Box::new(ResState([0; 4096]));
    // SAFETY: `state` is zeroed and larger than `struct __res_state`.
    if unsafe { res_9_ninit(&mut *state) } != 0 {
        // SAFETY: a failed res_ninit may still have allocated its state's
        // extension; ndestroy frees what's there and nothing else.
        unsafe { res_9_ndestroy(&mut *state) };
        return Err(io::Error::other("res_ninit failed"));
    }
    let mut set = [SockaddrUnion([0; 128]); MAX_SERVERS];
    // SAFETY: `state` was initialized by res_ninit; `set` holds MAX_SERVERS
    // unions, the count given.
    let count = unsafe { res_9_getservers(&mut *state, set.as_mut_ptr(), MAX_SERVERS as i32) };
    // SAFETY: initialized above, destroyed once.
    unsafe { res_9_ndestroy(&mut *state) };
    let count = usize::try_from(count).unwrap_or(0).min(MAX_SERVERS);
    let nameservers: Vec<Nameserver> = set[..count]
        .iter()
        .filter_map(|addr| sockaddr(&addr.0))
        // res_ninit's default when the system has no configuration and no
        // resolv.conf: 0.0.0.0, no nameserver at all.
        .filter(|addr| !addr.ip().is_unspecified())
        .map(|addr| Nameserver::new(addr, DnsProtocol::Udp))
        .collect();
    if nameservers.is_empty() {
        return Err(io::Error::other("libresolv has no nameservers"));
    }
    Ok(Config {
        nameservers,
        search_domains: Vec::new(),
        ndots: None,
        hosts: Hosts::from_system(),
    })
}

/// A `sockaddr_in` or `sockaddr_in6` (BSD layout: length, family, port in
/// network order, then the address); port 0 means DNS's own.
fn sockaddr(raw: &[u8; 128]) -> Option<SocketAddr> {
    let port = match u16::from_be_bytes([raw[2], raw[3]]) {
        0 => DnsProtocol::Udp.port(),
        port => port,
    };
    let ip = match raw[1] {
        AF_INET => IpAddr::V4(Ipv4Addr::new(raw[4], raw[5], raw[6], raw[7])),
        AF_INET6 => {
            let mut octets = [0; 16];
            octets.copy_from_slice(&raw[8..24]);
            IpAddr::V6(Ipv6Addr::from(octets))
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_families() {
        let mut v4 = [0u8; 128];
        v4[0] = 16;
        v4[1] = AF_INET;
        v4[2..4].copy_from_slice(&53u16.to_be_bytes());
        v4[4..8].copy_from_slice(&[192, 168, 1, 1]);
        assert_eq!(sockaddr(&v4), Some("192.168.1.1:53".parse().unwrap()));

        let mut v6 = [0u8; 128];
        v6[0] = 28;
        v6[1] = AF_INET6;
        v6[8..24].copy_from_slice(&"fd00::1".parse::<Ipv6Addr>().unwrap().octets());
        assert_eq!(sockaddr(&v6), Some("[fd00::1]:53".parse().unwrap()));

        let other = [0u8; 128];
        assert_eq!(sockaddr(&other), None);
    }
}
