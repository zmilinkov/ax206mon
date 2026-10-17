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

/// All interfaces with at least one address (loopback included), sorted by name.
pub fn list() -> Vec<Iface> {
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
    group(found)
}

fn group(found: Vec<(String, bool, IpAddr)>) -> Vec<Iface> {
    let mut map: BTreeMap<String, Iface> = BTreeMap::new();
    for (name, up, ip) in found {
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

/// What a `network` widget shows.
pub struct Select<'a> {
    /// Names to show, in this order; a trailing `*` matches a prefix
    /// ("br-*"). None or empty = every interface.
    pub names: Option<&'a [String]>,
    pub ipv4: bool,
    pub ipv6: bool,
    /// Include loopback when no names are given (a name list is explicit).
    pub loopback: bool,
}

fn name_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => name.starts_with(prefix),
        None => pattern == name,
    }
}

/// Apply the widget's address-family and interface filters. Interfaces left
/// with no addresses are dropped.
pub fn select(all: &[Iface], sel: &Select) -> Vec<Iface> {
    let keep = |a: &IpAddr| if a.is_ipv4() { sel.ipv4 } else { sel.ipv6 };
    let narrow = |i: &Iface| {
        let addrs: Vec<IpAddr> = i.addrs.iter().copied().filter(keep).collect();
        (!addrs.is_empty()).then(|| Iface { addrs, ..i.clone() })
    };
    match sel.names.filter(|n| !n.is_empty()) {
        Some(names) => {
            let mut out: Vec<Iface> = Vec::new();
            for pat in names {
                for i in all.iter().filter(|i| name_matches(pat, &i.name)) {
                    if !out.iter().any(|o| o.name == i.name) {
                        out.extend(narrow(i));
                    }
                }
            }
            out
        }
        None => all
            .iter()
            .filter(|i| sel.loopback || !i.addrs.iter().all(IpAddr::is_loopback))
            .filter_map(narrow)
            .collect(),
    }
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
        let all = group(found);
        assert_eq!(all.len(), 3);
        let v = select(&all, &sel(None, true, true, false));
        assert_eq!(v.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["br0", "eth0"]);
        assert_eq!(v[1].addrs, [ip("192.168.1.5"), ip("2001:db8::5"), ip("fe80::1")]);
        assert!(!v[0].up);
        assert_eq!(select(&all, &sel(None, true, true, true)).len(), 3);
    }

    fn sel(names: Option<&[String]>, ipv4: bool, ipv6: bool, loopback: bool) -> Select<'_> {
        Select { names, ipv4, ipv6, loopback }
    }

    fn sample() -> Vec<Iface> {
        group(vec![
            ("eth0".into(), true, ip("192.168.1.5")),
            ("eth0".into(), true, ip("2001:db8::5")),
            ("br-abc".into(), true, ip("172.18.0.1")),
            ("br-def".into(), true, ip("172.19.0.1")),
            ("wg0".into(), true, ip("fd00::1")),
            ("lo".into(), true, ip("127.0.0.1")),
        ])
    }

    #[test]
    fn address_family_filters_drop_empty_interfaces() {
        let all = sample();
        let v4 = select(&all, &sel(None, true, false, false));
        assert_eq!(v4.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["br-abc", "br-def", "eth0"]);
        assert_eq!(v4[2].addrs, [ip("192.168.1.5")]);
        let v6 = select(&all, &sel(None, false, true, false));
        assert_eq!(v6.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["eth0", "wg0"]);
    }

    #[test]
    fn named_interfaces_keep_order_support_prefix_and_loopback() {
        let all = sample();
        let names: Vec<String> = ["wg0", "eth0", "br-*", "lo", "nope"].map(String::from).into();
        let v = select(&all, &sel(Some(&names), true, true, false));
        assert_eq!(
            v.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
            ["wg0", "eth0", "br-abc", "br-def", "lo"]
        );
        let empty: Vec<String> = Vec::new();
        assert_eq!(select(&all, &sel(Some(&empty), true, true, false)).len(), 4);
    }

    #[test]
    fn getifaddrs_works() {
        let _ = list();
    }
}
