use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

pub fn is_private_url(url: &str) -> bool {
    let host = match extract_host(url) {
        Some(h) => h,
        None => return false,
    };

    if let Ok(ip) = host.parse::<IpAddr>() {
        return is_private_ip(&ip);
    }

    let lower = host.to_lowercase();
    lower == "localhost"
        || lower == "ip6-localhost"
        || lower == "ip6-loopback"
        || lower.ends_with(".local")
        || lower.ends_with(".internal")
}

pub fn extract_host(url: &str) -> Option<String> {
    let after_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;

    if after_scheme.starts_with('[') {
        let end_bracket = after_scheme.find(']')?;
        let host = &after_scheme[1..end_bracket];
        return if host.is_empty() {
            None
        } else {
            Some(host.to_string())
        };
    }

    let host = after_scheme
        .split('/')
        .next()?
        .split(':')
        .next()?
        .to_string();

    if host.is_empty() {
        None
    } else {
        Some(host)
    }
}

fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

pub fn is_private_ipv4(ip: &Ipv4Addr) -> bool {
    if ip.is_loopback() {
        return true;
    }
    let octets = ip.octets();

    if octets[0] == 10 {
        return true;
    }

    if octets[0] == 172 && (16..=31).contains(&octets[1]) {
        return true;
    }

    if octets[0] == 192 && octets[1] == 168 {
        return true;
    }

    if octets[0] == 169 && octets[1] == 254 {
        return true;
    }

    if ip.is_broadcast() || ip.is_multicast() {
        return true;
    }

    if ip.is_unspecified() {
        return true;
    }
    false
}

fn is_private_ipv6(ip: &Ipv6Addr) -> bool {
    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        || (ip.segments()[0] & 0xffc0) == 0xfe80
        || (ip.segments()[0] & 0xfe00) == 0xfc00
}
