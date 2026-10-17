//! Network interfaces that have an IPv4 or IPv6 address (via getifaddrs).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    /// IFF_RUNNING: link is up.
    pub up: bool,
    /// IPv4 first, then global IPv6, then link-local IPv6.
    pub addrs: Vec<IpAddr>,
}

/// Interfaces with at least one address, sorted by name. Loopback is only
/// included when `loopback` is true.
pub fn list(loopback: bool) -> Vec<Iface> {
    let mut found: Vec<(String, bool, IpAddr)> = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Vec::new();
    }
    let mut p = head;
    while !p.is_null() {
        let ifa = unsafe { &*p };
        p = ifa.ifa_next;
        if ifa.ifa_addr.is_null() {
            continue;
        }
        let name = unsafe { std::ffi::CStr::from_ptr(ifa.ifa_name) }
            .to_string_lossy()
            .into_owned();
        let up = ifa.ifa_flags & libc::IFF_RUNNING as u32 != 0;
        let ip = match unsafe { (*ifa.ifa_addr).sa_family } as i32 {
            libc::AF_INET => {
                let sa = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
                IpAddr::V4(Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr)))
            }
            libc::AF_INET6 => {
                let sa = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
                IpAddr::V6(Ipv6Addr::from(sa.sin6_addr.s6_addr))
            }
            _ => continue,
        };
        found.push((name, up, ip));
    }
    unsafe { libc::freeifaddrs(head) };
    group(found, loopback)
}

fn group(found: Vec<(String, bool, IpAddr)>, loopback: bool) -> Vec<Iface> {
    let mut map: BTreeMap<String, Iface> = BTreeMap::new();
    for (name, up, ip) in found {
        if !loopback && ip.is_loopback() {
            continue;
        }
        let e = map.entry(name.clone()).or_insert_with(|| Iface {
            name,
            up,
            addrs: Vec::new(),
        });
        e.addrs.push(ip);
    }
    let rank = |a: &IpAddr| match a {
        IpAddr::V4(_) => 0,
        IpAddr::V6(v) if v.segments()[0] & 0xffc0 != 0xfe80 => 1,
        IpAddr::V6(_) => 2,
    };
    let mut v: Vec<Iface> = map.into_values().collect();
    for i in &mut v {
        i.addrs.sort_by_key(|a| (rank(a), *a));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn groups_sorts_and_skips_loopback() {
        let found = vec![
            ("eth0".into(), true, ip("fe80::1")),
            ("lo".into(), true, ip("127.0.0.1")),
            ("lo".into(), true, ip("::1")),
            ("eth0".into(), true, ip("2001:db8::5")),
            ("eth0".into(), true, ip("192.168.1.5")),
            ("br0".into(), false, ip("10.0.0.1")),
        ];
        let v = group(found.clone(), false);
        assert_eq!(v.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["br0", "eth0"]);
        assert_eq!(v[1].addrs, [ip("192.168.1.5"), ip("2001:db8::5"), ip("fe80::1")]);
        assert!(!v[0].up);
        assert_eq!(group(found, true).len(), 3);
    }

    #[test]
    fn getifaddrs_works() {
        let _ = list(true);
    }
}
