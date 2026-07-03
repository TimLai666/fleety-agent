//! Co-location fingerprinting: derive a stable, hashed identifier for the LAN
//! this device is on, so the server can infer which site the device is at. The
//! fingerprint is a hash of the default gateway's MAC (or IP, if the MAC can't be
//! read) plus the subnet — stable per physical network, changes when the device
//! moves networks. Presence reporting is off unless `FLEETY_PRESENCE=on`; the raw
//! network attributes are never sent, only their hash.

use fleety_protocol::ClientMsg;
use sha2::{Digest, Sha256};

/// Whether presence reporting is enabled (`FLEETY_PRESENCE=on`). Off by default.
pub fn presence_enabled() -> bool {
    std::env::var("FLEETY_PRESENCE")
        .map(|v| v.eq_ignore_ascii_case("on"))
        .unwrap_or(false)
}

const DEFAULT_INTERVAL_SECS: u64 = 300;
const MIN_INTERVAL_SECS: u64 = 60;

/// Reporting period from `FLEETY_PRESENCE_INTERVAL_SECS` (default 300), clamped to
/// a 60-second floor so a misconfiguration can't hammer the server.
pub fn interval_secs() -> u64 {
    std::env::var("FLEETY_PRESENCE_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECS)
        .max(MIN_INTERVAL_SECS)
}

/// Hash the LAN's stable attributes into a fingerprint. Pure and testable: the
/// gateway id (MAC preferred, else IP) is the anchor; the subnet is folded in when
/// present. Returns `None` when there is no gateway anchor at all (nothing stable
/// to hash), which the server treats as "location unknown".
pub fn fingerprint_from(gateway_id: Option<&str>, subnet: Option<&str>) -> Option<String> {
    let gw = gateway_id?.trim();
    if gw.is_empty() {
        return None;
    }
    let mut hasher = Sha256::new();
    hasher.update(gw.to_ascii_lowercase().as_bytes());
    hasher.update(b"|");
    hasher.update(subnet.unwrap_or("").trim().as_bytes());
    let digest = hasher.finalize();
    Some(format!("sha256:{digest:x}"))
}

/// Build the wire frame for a co-location report. Pure so its shape is testable.
/// Always produces a frame when presence is enabled — an absent fingerprint is
/// reported honestly (the server then leaves the site unchanged).
pub fn build_colocation_frame(
    fingerprint: Option<String>,
    subnet: Option<String>,
) -> std::result::Result<String, serde_json::Error> {
    serde_json::to_string(&ClientMsg::Colocation {
        fingerprint,
        subnet,
        peers: Vec::new(),
    })
}

/// Compute the current co-location report frame, or `None` when presence is
/// disabled. Reads the OS network state best-effort; any failure degrades to an
/// absent fingerprint rather than an error.
pub fn report_frame() -> Option<String> {
    if !presence_enabled() {
        return None;
    }
    let (gateway_id, subnet) = read_gateway_and_subnet();
    let fingerprint = fingerprint_from(gateway_id.as_deref(), subnet.as_deref());
    match build_colocation_frame(fingerprint, subnet) {
        Ok(frame) => Some(frame),
        Err(e) => {
            tracing::warn!(%e, "could not serialize co-location report");
            None
        }
    }
}

/// Best-effort read of `(gateway_id, subnet)` for the current default route.
/// `gateway_id` is the gateway MAC when readable, else the gateway IP. Any OS
/// error or unsupported platform yields `(None, None)`; presence then reports an
/// absent fingerprint. Never panics.
fn read_gateway_and_subnet() -> (Option<String>, Option<String>) {
    #[cfg(target_os = "linux")]
    {
        linux_gateway_and_subnet()
    }
    #[cfg(not(target_os = "linux"))]
    {
        command_gateway_and_subnet()
    }
}

#[cfg(target_os = "linux")]
fn linux_gateway_and_subnet() -> (Option<String>, Option<String>) {
    // /proc/net/route: the default route (Destination 00000000) names the
    // interface and the gateway IP (little-endian hex). Then /proc/net/arp maps
    // the gateway IP to its MAC.
    let Ok(route) = std::fs::read_to_string("/proc/net/route") else {
        return (None, None);
    };
    let mut gateway_ip = None;
    for line in route.lines().skip(1) {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 3 && cols[1] == "00000000" {
            if let Ok(be) = u32::from_str_radix(cols[2], 16) {
                let ip = be.to_le_bytes();
                gateway_ip = Some(format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]));
                break;
            }
        }
    }
    let Some(gw_ip) = gateway_ip else {
        return (None, None);
    };
    let subnet = gw_ip
        .rsplit_once('.')
        .map(|(prefix, _)| format!("{prefix}.0/24"));
    let mac = std::fs::read_to_string("/proc/net/arp").ok().and_then(|arp| {
        arp.lines().skip(1).find_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.first() == Some(&gw_ip.as_str()) && cols.len() >= 4 && cols[3] != "00:00:00:00:00:00" {
                Some(cols[3].to_string())
            } else {
                None
            }
        })
    });
    (Some(mac.unwrap_or(gw_ip)), subnet)
}

#[cfg(not(target_os = "linux"))]
fn command_gateway_and_subnet() -> (Option<String>, Option<String>) {
    // Windows / macOS: derive the default gateway IP from the routing table, then
    // resolve its MAC via the ARP cache. All parsing is defensive; any failure
    // degrades to `(None, None)`.
    let gw_ip = default_gateway_ip();
    let Some(gw_ip) = gw_ip else {
        return (None, None);
    };
    let subnet = gw_ip
        .rsplit_once('.')
        .map(|(prefix, _)| format!("{prefix}.0/24"));
    let mac = arp_mac_for(&gw_ip);
    (Some(mac.unwrap_or(gw_ip)), subnet)
}

#[cfg(target_os = "windows")]
fn default_gateway_ip() -> Option<String> {
    // `route print -4 0.0.0.0` lists the default route; the gateway is the 3rd
    // column of the `0.0.0.0  0.0.0.0  <gateway>  ...` line.
    let out = std::process::Command::new("route")
        .args(["print", "-4", "0.0.0.0"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 3 && cols[0] == "0.0.0.0" && cols[1] == "0.0.0.0" {
            let gw = cols[2];
            if gw.chars().next().is_some_and(|c| c.is_ascii_digit()) && gw != "0.0.0.0" {
                return Some(gw.to_string());
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn default_gateway_ip() -> Option<String> {
    let out = std::process::Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("gateway:") {
            let gw = rest.trim();
            if !gw.is_empty() {
                return Some(gw.to_string());
            }
        }
    }
    None
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn default_gateway_ip() -> Option<String> {
    None
}

#[cfg(not(target_os = "linux"))]
fn arp_mac_for(gw_ip: &str) -> Option<String> {
    let out = std::process::Command::new("arp")
        .args(["-a", gw_ip])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        if line.contains(gw_ip) {
            // A MAC is 6 hex groups joined by ':' or '-'.
            for tok in line.split_whitespace() {
                let sep = if tok.contains('-') { '-' } else { ':' };
                let parts: Vec<&str> = tok.split(sep).collect();
                if parts.len() == 6 && parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit())) {
                    return Some(tok.replace('-', ":").to_ascii_lowercase());
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_hash_and_needs_a_gateway() {
        // No gateway anchor → no fingerprint.
        assert_eq!(fingerprint_from(None, Some("192.168.1.0/24")), None);
        assert_eq!(fingerprint_from(Some(""), Some("x")), None);

        // Same inputs hash identically; MAC case is normalized.
        let a = fingerprint_from(Some("AA:BB:CC:DD:EE:FF"), Some("192.168.1.0/24"));
        let b = fingerprint_from(Some("aa:bb:cc:dd:ee:ff"), Some("192.168.1.0/24"));
        assert!(a.is_some());
        assert_eq!(a, b);
        assert!(a.as_deref().unwrap().starts_with("sha256:"));

        // A different network hashes differently.
        let c = fingerprint_from(Some("aa:bb:cc:dd:ee:ff"), Some("10.0.0.0/24"));
        assert_ne!(a, c);
    }

    #[test]
    fn frame_shape_roundtrips_with_and_without_fingerprint() {
        let with = build_colocation_frame(Some("sha256:abcd".into()), Some("192.168.1.0/24".into()))
            .expect("ser");
        let parsed: ClientMsg = serde_json::from_str(&with).expect("de");
        assert_eq!(
            parsed,
            ClientMsg::Colocation {
                fingerprint: Some("sha256:abcd".into()),
                subnet: Some("192.168.1.0/24".into()),
                peers: vec![],
            }
        );

        // Absent fingerprint still produces a valid frame (reported honestly).
        let without = build_colocation_frame(None, None).expect("ser");
        let parsed: ClientMsg = serde_json::from_str(&without).expect("de");
        assert_eq!(
            parsed,
            ClientMsg::Colocation { fingerprint: None, subnet: None, peers: vec![] }
        );
    }

    #[test]
    fn interval_defaults_and_floors() {
        // Reads env, so isolate with explicit set/remove. Default when unset.
        std::env::remove_var("FLEETY_PRESENCE_INTERVAL_SECS");
        assert_eq!(interval_secs(), 300);
        std::env::set_var("FLEETY_PRESENCE_INTERVAL_SECS", "10");
        assert_eq!(interval_secs(), 60); // floored
        std::env::set_var("FLEETY_PRESENCE_INTERVAL_SECS", "900");
        assert_eq!(interval_secs(), 900);
        std::env::remove_var("FLEETY_PRESENCE_INTERVAL_SECS");
    }

    #[test]
    fn report_frame_is_none_when_disabled() {
        std::env::remove_var("FLEETY_PRESENCE");
        assert_eq!(report_frame(), None);
    }
}
