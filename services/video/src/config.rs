use std::net::{IpAddr, UdpSocket};

pub struct Config {
    pub http_port: u16,
    pub udp_port: u16,
    pub public_ip: IpAddr,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            http_port: env_or("HTTP_PORT", 4000),
            udp_port: env_or("UDP_PORT", 4001),
            public_ip: std::env::var("PUBLIC_IP")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(detect_local_ip),
        }
    }
}

/// Detect local network IP. Tries LAN gateway first (192.168.x.x),
/// falls back to internet-facing IP.
fn detect_local_ip() -> IpAddr {
    // Try LAN gateway first (most common for local dev)
    for target in &["192.168.1.1:80", "10.0.0.1:80", "8.8.8.8:80"] {
        if let Some(ip) = try_detect_ip(target) {
            // Prefer 192.168.x.x over 10.x.x.x (VPN usually uses 10.x)
            if matches!(ip, IpAddr::V4(v4) if v4.octets()[0] == 192) {
                return ip;
            }
        }
    }
    // Fallback: any non-loopback
    for target in &["192.168.1.1:80", "8.8.8.8:80"] {
        if let Some(ip) = try_detect_ip(target) {
            return ip;
        }
    }
    [127, 0, 0, 1].into()
}

fn try_detect_ip(target: &str) -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(target).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
