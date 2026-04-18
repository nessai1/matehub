use std::net::IpAddr;

pub struct Config {
    pub http_port: u16,
    pub udp_port: u16,
    /// IPs to advertise as ICE host candidates.
    /// Multiple so clients can pick a reachable one — if only one is given
    /// and routing asymmetry forces replies via a different interface,
    /// ICE falls back to peer-reflexive and nomination stalls.
    pub public_ips: Vec<IpAddr>,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            http_port: env_or("HTTP_PORT", 4000),
            udp_port: env_or("UDP_PORT", 4001),
            public_ips: public_ips_from_env(),
        }
    }
}

/// Resolve the host candidate IPs.
///
/// - `PUBLIC_IP` (comma-separated) — explicit override, used verbatim.
/// - Otherwise: every non-loopback, non-link-local IPv4 we can see
///   (wifi, ethernet, docker, vpn). Lets the client pick whichever
///   actually routes back to us.
fn public_ips_from_env() -> Vec<IpAddr> {
    if let Ok(raw) = std::env::var("PUBLIC_IP") {
        let parsed: Vec<IpAddr> = raw
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        if !parsed.is_empty() {
            return parsed;
        }
    }
    discover_local_ips()
}

fn discover_local_ips() -> Vec<IpAddr> {
    let Ok(ifs) = if_addrs::get_if_addrs() else {
        return vec![[127, 0, 0, 1].into()];
    };
    let mut ips: Vec<IpAddr> = ifs
        .into_iter()
        .map(|i| i.addr.ip())
        .filter(|ip| !ip.is_loopback() && !is_link_local(ip) && !ip.is_unspecified())
        // IPv4 only for now — str0m host candidates work via the same UDP
        // socket bound on 0.0.0.0, and browsers generally prefer v4 on LAN.
        .filter(|ip| ip.is_ipv4())
        .collect();
    if ips.is_empty() {
        ips.push([127, 0, 0, 1].into());
    }
    ips
}

fn is_link_local(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
