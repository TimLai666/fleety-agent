//! mDNS service advertisement so devices on the same LAN can find the server
//! without anyone hand-typing `FLEETY_AGENT_URL`. The server registers a
//! `_fleety._tcp.local.` record with its `FLEETY_ADDR` port; daemons + the CLI
//! discover it with [`discover`].
//!
//! Set `FLEETY_MDNS_DISABLED=1` to skip registration (corporate networks often
//! block mDNS; fall back to an explicit URL).

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceInfo};

const SERVICE_TYPE: &str = "_fleety._tcp.local.";

fn disabled() -> bool {
    std::env::var("FLEETY_MDNS_DISABLED").is_ok()
}

/// Local IP address(es) other devices should reach the server on. Used by the
/// advertisement, since `127.0.0.1` would be useless to peers on the LAN.
///
/// `FLEETY_MDNS_HOST_IP` lets the operator pin the advertised IP when binding
/// to `0.0.0.0` (no other dep does interface enumeration here — mDNS just won't
/// work without a routable address).
fn local_ips(bind_addr: &str) -> Vec<IpAddr> {
    if let Ok(ip) = std::env::var("FLEETY_MDNS_HOST_IP") {
        if let Ok(parsed) = ip.parse::<IpAddr>() {
            return vec![parsed];
        }
    }
    if let Ok(addr) = bind_addr.parse::<SocketAddr>() {
        let ip = addr.ip();
        if !ip.is_unspecified() && !ip.is_loopback() {
            return vec![ip];
        }
    }
    Vec::new()
}

fn port_from(bind_addr: &str) -> u16 {
    bind_addr
        .parse::<SocketAddr>()
        .map(|a| a.port())
        .unwrap_or(8787)
}

/// Spawn the mDNS advertisement task. Holds the `ServiceDaemon` alive for the
/// life of the process via a leak (it's a one-per-process resource). Returns
/// immediately; failures are logged and don't stop the server.
pub fn spawn_advertise(bind_addr: &str) {
    if disabled() {
        tracing::info!("FLEETY_MDNS_DISABLED set; skipping mDNS announcement");
        return;
    }
    let port = port_from(bind_addr);
    let ips = local_ips(bind_addr);
    if ips.is_empty() {
        tracing::info!("no non-loopback IPs found; skipping mDNS announcement");
        return;
    }
    let daemon = match ServiceDaemon::new() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "could not start mDNS daemon");
            return;
        }
    };
    let hostname = std::env::var("FLEETY_MDNS_HOST")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "fleety".to_string());
    let host_label = format!("{hostname}.local.");
    let instance = format!("fleety-{hostname}");
    let txt: &[(&str, &str)] = &[("version", agent_core::VERSION)];
    let info = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &host_label,
        ips.as_slice(),
        port,
        txt,
    );
    let info = match info {
        Ok(info) => info,
        Err(e) => {
            tracing::warn!(error = %e, "could not build mDNS ServiceInfo");
            return;
        }
    };
    match daemon.register(info) {
        Ok(_) => tracing::info!(
            %SERVICE_TYPE,
            %instance,
            port,
            "advertising via mDNS",
        ),
        Err(e) => {
            tracing::warn!(error = %e, "could not register mDNS service");
            return;
        }
    }
    // Leak the daemon: it's a one-per-process resource that has to outlive
    // every connection so peers keep seeing us.
    Box::leak(Box::new(daemon));
}

/// Browse the LAN for a fleety server and return the first `ws://host:port` URL
/// we see, or `None` after `timeout`. The CLI and fleetyd each carry their own
/// in-binary copy of this logic so the dep stays self-contained per binary; this
/// is here as the canonical reference implementation + for tests.
#[allow(dead_code)]
pub fn discover(timeout: Duration) -> Option<String> {
    if disabled() {
        return None;
    }
    let daemon = ServiceDaemon::new().ok()?;
    let receiver = daemon.browse(SERVICE_TYPE).ok()?;
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let recv_timeout = remaining.min(Duration::from_millis(500));
        match receiver.recv_timeout(recv_timeout) {
            Ok(event) => {
                if let mdns_sd::ServiceEvent::ServiceResolved(info) = event {
                    let addrs = info.get_addresses_v4();
                    if let Some(ip) = addrs.iter().next() {
                        let url = format!("ws://{}:{}", ip, info.get_port());
                        let _ = daemon.shutdown();
                        return Some(url);
                    }
                }
            }
            // flume's RecvTimeoutError::Timeout means "no event yet", keep
            // looping until our overall deadline. Disconnected means break.
            Err(flume::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        }
    }
    let _ = daemon.shutdown();
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_parsing_picks_addr_port_then_falls_back() {
        assert_eq!(port_from("127.0.0.1:9999"), 9999);
        assert_eq!(port_from("garbage"), 8787);
    }

    #[test]
    fn disabled_short_circuits() {
        std::env::set_var("FLEETY_MDNS_DISABLED", "1");
        assert!(disabled());
        assert!(discover(Duration::from_millis(50)).is_none());
        std::env::remove_var("FLEETY_MDNS_DISABLED");
        assert!(!disabled());
    }
}
