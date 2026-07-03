//! Presence inference: turn devices' co-location signals into a probabilistic
//! view of "is anyone at this site". Reachability is never treated as presence —
//! every answer carries a confidence and its reasons. See the `presence-inference`
//! spec. Tracking is gated by a per-device opt-in that defaults off.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

use crate::sites;
use crate::storage::Storage;

/// Caveat attached to every presence answer: reachability is not presence.
const PRESENCE_CAVEAT: &str =
    "This is a probabilistic estimate — a reachable or present device does not prove a person is there.";

/// A device currently at a site, for the site-level presence estimate.
#[derive(Debug, Clone)]
pub struct PresentDevice {
    pub device: String,
    /// `stationary` | `mobile` | `unknown`.
    pub mobility: String,
    /// Whether this site is the device's `home_site`.
    pub is_home_site: bool,
}

/// Probabilistic "is a person present at this site" from the devices there.
///
/// A stationary device only shows the site is reachable (weak); a mobile device
/// (phone/laptop) is a stronger personal-presence signal, strongest when the site
/// is its home site. The score is a pure function of the inputs, clamped to
/// `[0, 1]`, and always paired with the reasons behind it.
pub fn person_present_confidence(present: &[PresentDevice]) -> (f32, Vec<String>) {
    let mut score = 0.0f32;
    let mut reasons = Vec::new();
    for d in present {
        match d.mobility.as_str() {
            "mobile" if d.is_home_site => {
                score += 0.6;
                reasons.push(format!(
                    "{} is a mobile device at its home site — a stronger presence signal",
                    d.device
                ));
            }
            "mobile" => {
                score += 0.4;
                reasons.push(format!(
                    "{} is a mobile device present here — a personal-presence signal",
                    d.device
                ));
            }
            _ => {
                score += 0.1;
                reasons.push(format!(
                    "{} is a stationary/unknown device — shows the site is reachable, not that a person is here",
                    d.device
                ));
            }
        }
    }
    if present.is_empty() {
        reasons.push("no devices are reported at this site".to_string());
    }
    (score.clamp(0.0, 1.0), reasons)
}

/// Corroborate a site-level presence estimate with devices confirmed on the
/// server's own network segment (their reported subnet matches the server's). A
/// device physically on the server's LAN is a stronger co-location signal, so it
/// nudges confidence up and records why. Pure function of the corroborated set.
pub fn corroborate(base: f32, reasons: &mut Vec<String>, corroborated: &[String]) -> f32 {
    let mut score = base;
    for device in corroborated {
        score += 0.1;
        reasons.push(format!(
            "{device} is on the server's own network segment — corroborates co-location"
        ));
    }
    score.clamp(0.0, 1.0)
}

/// The server's own `/24` subnet (e.g. `192.168.1.0/24`), derived from a local
/// non-loopback address, or `None` when it cannot be determined. Used only to
/// corroborate that a device is on the server's segment.
pub fn server_subnet() -> Option<String> {
    // A connected UDP socket picks the OS's default-route source address without
    // sending anything; from that we take the /24. Best-effort, never panics.
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    match ip {
        std::net::IpAddr::V4(v4) if !v4.is_loopback() => {
            let o = v4.octets();
            Some(format!("{}.{}.{}.0/24", o[0], o[1], o[2]))
        }
        _ => None,
    }
}

/// Device-level departure signal: a mobile device whose home site is known but
/// whose current site is elsewhere leans toward "the person left". Never a
/// certainty — reachability is not presence.
pub fn departure_confidence(mobility: &str, home_site: &str, current_site: &str) -> (f32, Vec<String>) {
    if mobility == "mobile" && !home_site.is_empty() && current_site != home_site {
        (
            0.5,
            vec![format!(
                "a mobile device whose home site is '{home_site}' is currently at '{current_site}' — leans toward a departure"
            )],
        )
    } else {
        (
            0.0,
            vec!["no departure signal from this device".to_string()],
        )
    }
}

/// Outcome of applying one co-location report.
#[derive(Debug, Clone, PartialEq)]
pub enum ColocationOutcome {
    /// The device has not opted in; nothing was recorded.
    NotOptedIn,
    /// No fingerprint could be determined; the site was left unchanged.
    NoFingerprint,
    /// A report was processed. `changed` is whether the current site moved (and a
    /// timeline event was written); `unbound` marks a fingerprint no site claims.
    Applied {
        changed: bool,
        from: String,
        to: String,
        unbound: bool,
    },
}

/// Apply one co-location report for `device_id`: gate on opt-in, map the
/// fingerprint to a site, and — only when the current site actually changes —
/// update the record and append one timeline event. An unbound fingerprint maps
/// to `unknown` and is flagged so it can be bound explicitly. `now` is the event
/// timestamp (unix secs), passed in so the logic stays testable.
pub fn apply_colocation(
    storage: &Storage,
    sites_dir: &Path,
    device_id: &str,
    fingerprint: Option<&str>,
    subnet: Option<&str>,
    now: u64,
) -> ColocationOutcome {
    if !storage.device_presence_opt_in(device_id) {
        return ColocationOutcome::NotOptedIn;
    }
    // Remember the reported subnet (for same-network corroboration) even when the
    // fingerprint is absent. Best-effort.
    if let Some(sn) = subnet {
        if let Err(e) = storage.set_device_last_subnet(device_id, sn) {
            tracing::warn!(error = %e, device = device_id, "cannot record last subnet");
        }
    }
    let Some(fp) = fingerprint else {
        return ColocationOutcome::NoFingerprint;
    };
    // Remember the current network fingerprint so `site_bind_fingerprint` can
    // bind it to a site later. Best-effort; a failure only loses the binding hint.
    if let Err(e) = storage.set_device_last_fingerprint(device_id, fp) {
        tracing::warn!(error = %e, device = device_id, "cannot record last fingerprint");
    }
    let matched = sites::site_for_fingerprint(sites_dir, fp);
    let unbound = matched.is_none();
    let to = matched.unwrap_or_else(|| "unknown".to_string());
    let from = storage.device_site(device_id);
    if from == to {
        return ColocationOutcome::Applied {
            changed: false,
            from,
            to,
            unbound,
        };
    }
    let confidence = if unbound { 0.3f32 } else { 0.8f32 };
    // Best-effort persistence; a failure is logged, never fatal.
    if let Err(e) = storage.set_device_site(device_id, &to, now) {
        tracing::warn!(error = %e, device = device_id, "cannot update device site from co-location");
        return ColocationOutcome::Applied {
            changed: false,
            from,
            to,
            unbound,
        };
    }
    storage.append_presence_timeline(&json!({
        "ts_secs": now,
        "device": device_id,
        "from_site": from,
        "to_site": to,
        "signal": "colocation",
        "confidence": confidence,
    }));
    ColocationOutcome::Applied {
        changed: true,
        from,
        to,
        unbound,
    }
}

/// Devices currently at `site`, with the mobility and home-site facts the
/// confidence estimate needs. Scans the device records under `devices_dir`.
fn present_devices_at(devices_dir: &Path, site: &str) -> Vec<PresentDevice> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(devices_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path().join("device.json")) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if record.get("site").and_then(Value::as_str) != Some(site) {
            continue;
        }
        let device = record
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mobility = record
            .get("mobility")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let is_home_site = record.get("home_site").and_then(Value::as_str) == Some(site);
        out.push(PresentDevice {
            device,
            mobility,
            is_home_site,
        });
    }
    out
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

/// Register the five presence tools, anchored at the Agent `home`. `sites_dir`
/// and the storage-backed device records are read/written; nothing here runs
/// unless a device has opted in.
pub fn register_presence(registry: &mut ToolRegistry, home: PathBuf, sites_dir: PathBuf) {
    let storage = Arc::new(Storage::new(home));
    registry.register(Box::new(PresenceShow {
        storage: storage.clone(),
    }));
    registry.register(Box::new(DevicePresenceTool {
        storage: storage.clone(),
    }));
    registry.register(Box::new(SiteBindFingerprint {
        storage: storage.clone(),
        sites_dir: sites_dir.clone(),
    }));
    registry.register(Box::new(DeviceSetHomeSite {
        storage: storage.clone(),
        sites_dir,
    }));
    registry.register(Box::new(DeviceSetPresenceOptIn { storage }));
}

struct PresenceShow {
    storage: Arc<Storage>,
}

#[async_trait]
impl Tool for PresenceShow {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "presence_show".to_string(),
            description: "Estimate presence at a site: the devices currently there and a probabilistic 'is a person present' signal with reasons. Reachability is not presence.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "site": { "type": "string" } },
                "required": ["site"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let site = require_str(&args, "site")?;
        let present = present_devices_at(&self.storage.devices_dir(), site);
        let (base, mut reasons) = person_present_confidence(&present);
        // Secondary signal: devices whose last-reported subnet matches the
        // server's own segment corroborate co-location (server LAN only).
        let corroborated: Vec<String> = match server_subnet() {
            Some(server_sn) => present
                .iter()
                .filter(|d| self.storage.device_last_subnet(&d.device).as_deref() == Some(server_sn.as_str()))
                .map(|d| d.device.clone())
                .collect(),
            None => Vec::new(),
        };
        let confidence = corroborate(base, &mut reasons, &corroborated);
        let devices: Vec<Value> = present
            .iter()
            .map(|d| json!({ "device": d.device, "mobility": d.mobility, "is_home_site": d.is_home_site }))
            .collect();
        Ok(json!({
            "site": site,
            "devices": devices,
            "person_present": { "confidence": confidence, "reasons": reasons },
            "caveat": PRESENCE_CAVEAT,
        }))
    }
}

struct DevicePresenceTool {
    storage: Arc<Storage>,
}

#[async_trait]
impl Tool for DevicePresenceTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "device_presence".to_string(),
            description: "A device's inferred whereabouts: its current site, home site, how long it has been there, and a probabilistic departure signal with reasons.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "device": { "type": "string" } },
                "required": ["device"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let device = require_str(&args, "device")?;
        let site = self.storage.device_site(device);
        let home_site = self.storage.device_home_site(device);
        let mobility = self
            .storage
            .read_device_mobility(device)
            .unwrap_or_else(|| "unknown".to_string());
        let (confidence, reasons) = departure_confidence(&mobility, &home_site, &site);
        Ok(json!({
            "device": device,
            "site": site,
            "home_site": home_site,
            "mobility": mobility,
            "departure": { "confidence": confidence, "reasons": reasons },
            "caveat": PRESENCE_CAVEAT,
        }))
    }
}

struct SiteBindFingerprint {
    storage: Arc<Storage>,
    sites_dir: PathBuf,
}

#[async_trait]
impl Tool for SiteBindFingerprint {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "site_bind_fingerprint".to_string(),
            description: "Bind a device's currently reported network fingerprint to a site, so future reports of that network place the device at that site.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "device": { "type": "string" },
                    "site": { "type": "string", "description": "a registered site id" }
                },
                "required": ["device", "site"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let device = require_str(&args, "device")?;
        let site = require_str(&args, "site")?;
        let Some(fp) = self.storage.device_last_fingerprint(device) else {
            return Err(CoreError::Message(format!(
                "device '{device}' has reported no network fingerprint yet; it must connect with presence enabled before its network can be bound"
            )));
        };
        sites::add_fingerprint_to_site(&self.sites_dir, site, &fp)?;
        Ok(json!({ "site": site, "fingerprint": fp, "bound": true }))
    }
}

struct DeviceSetHomeSite {
    storage: Arc<Storage>,
    sites_dir: PathBuf,
}

#[async_trait]
impl Tool for DeviceSetHomeSite {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "device_set_home_site".to_string(),
            description: "Set a device's home site (its usual location), used as the baseline for presence and departure inference. Must be a registered site.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "device": { "type": "string" },
                    "home_site": { "type": "string" }
                },
                "required": ["device", "home_site"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let device = require_str(&args, "device")?;
        let home_site = require_str(&args, "home_site")?;
        if !self.sites_dir.join(format!("{home_site}.json")).exists() {
            return Err(CoreError::Message(format!(
                "unknown site '{home_site}'; register it with site_set first"
            )));
        }
        self.storage.set_device_home_site(device, home_site)?;
        Ok(json!({ "device": device, "home_site": home_site, "set": true }))
    }
}

struct DeviceSetPresenceOptIn {
    storage: Arc<Storage>,
}

#[async_trait]
impl Tool for DeviceSetPresenceOptIn {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "device_set_presence_opt_in".to_string(),
            description: "Enable or disable presence tracking for a device (server-side). Off by default; when off, the device's site changes and timeline are not recorded.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "device": { "type": "string" },
                    "enabled": { "type": "boolean" }
                },
                "required": ["device", "enabled"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let device = require_str(&args, "device")?;
        let enabled = args
            .get("enabled")
            .and_then(Value::as_bool)
            .ok_or_else(|| CoreError::Message("missing required boolean argument 'enabled'".to_string()))?;
        self.storage.set_device_presence_opt_in(device, enabled)?;
        Ok(json!({ "device": device, "presence_opt_in": enabled, "set": true }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(device: &str, mobility: &str, is_home_site: bool) -> PresentDevice {
        PresentDevice {
            device: device.to_string(),
            mobility: mobility.to_string(),
            is_home_site,
        }
    }

    #[test]
    fn confidence_matches_spec_examples() {
        // Only a stationary desktop present → low.
        let (low, _) = person_present_confidence(&[dev("desk", "stationary", true)]);
        // A mobile phone whose home_site is this site present → higher.
        let (higher, _) = person_present_confidence(&[dev("phone", "mobile", true)]);
        assert!(higher > low, "mobile-at-home ({higher}) should beat stationary-only ({low})");

        // A usually-home mobile device that is away → departure-leaning, not certain.
        let (dep, reasons) = departure_confidence("mobile", "home", "away");
        assert!(dep > 0.0 && dep < 1.0, "departure is probabilistic, got {dep}");
        assert!(reasons.iter().any(|r| r.contains("departure")));

        // A stationary device that is elsewhere gives no departure signal.
        let (none, _) = departure_confidence("stationary", "home", "away");
        assert_eq!(none, 0.0);
    }

    #[test]
    fn corroboration_raises_confidence_with_reasons() {
        // No corroborated devices → base unchanged, no extra reasons.
        let mut reasons = vec!["base reason".to_string()];
        let same = corroborate(0.4, &mut reasons, &[]);
        assert_eq!(same, 0.4);
        assert_eq!(reasons.len(), 1);

        // Corroborated devices bump confidence and record why (clamped to 1.0).
        let mut reasons = Vec::new();
        let raised = corroborate(0.4, &mut reasons, &["phone".to_string(), "laptop".to_string()]);
        assert!(raised > 0.4, "corroboration should raise confidence, got {raised}");
        assert!(reasons.iter().any(|r| r.contains("phone") && r.contains("server's own network")));
        assert!(corroborate(0.95, &mut Vec::new(), &["a".into(), "b".into()]) <= 1.0);
    }

    #[test]
    fn empty_site_has_zero_confidence_with_reason() {
        let (score, reasons) = person_present_confidence(&[]);
        assert_eq!(score, 0.0);
        assert!(reasons.iter().any(|r| r.contains("no devices")));
    }

    fn temp() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("fleety-presence-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).expect("mk temp");
        d
    }

    #[test]
    fn apply_colocation_gates_maps_and_debounces() {
        let home = temp();
        let storage = Storage::new(home.clone());
        let sites_dir = storage.sites_dir();
        std::fs::create_dir_all(&sites_dir).expect("mk sites");
        // Register a site and bind a fingerprint to it.
        std::fs::write(
            sites_dir.join("home.json"),
            json!({ "id": "home", "name": "Home", "fingerprints": ["fp-home"] }).to_string(),
        )
        .expect("site");

        // Not opted in → nothing recorded.
        assert_eq!(
            apply_colocation(&storage, &sites_dir, "pi", Some("fp-home"), None, 100),
            ColocationOutcome::NotOptedIn
        );
        assert_eq!(storage.device_site("pi"), "unknown");

        storage.set_device_presence_opt_in("pi", true).expect("opt in");

        // Absent fingerprint → site unchanged (but the subnet is still recorded).
        assert_eq!(
            apply_colocation(&storage, &sites_dir, "pi", None, Some("192.168.1.0/24"), 100),
            ColocationOutcome::NoFingerprint
        );
        assert_eq!(storage.device_last_subnet("pi").as_deref(), Some("192.168.1.0/24"));

        // Known fingerprint → site moves to `home`, one timeline event written.
        let out = apply_colocation(&storage, &sites_dir, "pi", Some("fp-home"), None, 200);
        assert_eq!(
            out,
            ColocationOutcome::Applied { changed: true, from: "unknown".into(), to: "home".into(), unbound: false }
        );
        assert_eq!(storage.device_site("pi"), "home");
        let timeline = std::fs::read_to_string(storage.presence_timeline_path()).expect("timeline");
        assert_eq!(timeline.lines().count(), 1);

        // Same site again → debounced, no new event.
        let out = apply_colocation(&storage, &sites_dir, "pi", Some("fp-home"), None, 300);
        assert_eq!(
            out,
            ColocationOutcome::Applied { changed: false, from: "home".into(), to: "home".into(), unbound: false }
        );
        let timeline = std::fs::read_to_string(storage.presence_timeline_path()).expect("timeline");
        assert_eq!(timeline.lines().count(), 1);

        // Unknown fingerprint → site becomes `unknown`, flagged unbound.
        let out = apply_colocation(&storage, &sites_dir, "pi", Some("fp-elsewhere"), None, 400);
        assert_eq!(
            out,
            ColocationOutcome::Applied { changed: true, from: "home".into(), to: "unknown".into(), unbound: true }
        );
        assert_eq!(storage.device_site("pi"), "unknown");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// End-to-end across the pieces (offline): a real wire `Colocation` frame →
    /// the exact handling the connection does (`apply_colocation`) → the
    /// `presence_show` tool. The socket transport itself is covered by the
    /// transport tests; this ties protocol + server state + tool query together.
    #[tokio::test]
    async fn colocation_wire_to_presence_show_e2e() {
        let home = temp();
        let storage = Arc::new(Storage::new(home.clone()));
        let sites_dir = storage.sites_dir();
        std::fs::create_dir_all(&sites_dir).expect("mk sites");
        // A registered site with a bound fingerprint, and an opted-in mobile phone.
        std::fs::write(
            sites_dir.join("home.json"),
            json!({ "id": "home", "name": "Home", "fingerprints": ["sha256:netA"] }).to_string(),
        )
        .expect("site");
        let mut reg = ToolRegistry::new();
        register_presence(&mut reg, storage.home(), sites_dir.clone());
        crate::sites::register(&mut reg, &sites_dir, &storage.devices_dir());
        reg.call("device_set_presence_opt_in", json!({ "device": "phone", "enabled": true }))
            .await
            .expect("opt in");
        reg.call("device_set_mobility", json!({ "device": "phone", "mobility": "mobile" }))
            .await
            .expect("mobility");
        reg.call("device_set_home_site", json!({ "device": "phone", "home_site": "home" }))
            .await
            .expect("home site");

        // A device sends this exact frame over the wire.
        let wire = json!({
            "type": "colocation",
            "fingerprint": "sha256:netA",
            "subnet": "192.168.1.0/24"
        })
        .to_string();
        let msg: fleety_protocol::ClientMsg = serde_json::from_str(&wire).expect("de wire");
        // The connection handler routes it exactly like this.
        let fleety_protocol::ClientMsg::Colocation { fingerprint, subnet, .. } = msg else {
            panic!("expected a colocation frame");
        };
        let outcome = apply_colocation(
            &storage,
            &sites_dir,
            "phone",
            fingerprint.as_deref(),
            subnet.as_deref(),
            1000,
        );
        assert!(matches!(outcome, ColocationOutcome::Applied { changed: true, .. }));

        // Server state moved: site updated + exactly one timeline event.
        assert_eq!(storage.device_site("phone"), "home");
        let timeline = std::fs::read_to_string(storage.presence_timeline_path()).expect("timeline");
        assert_eq!(timeline.lines().count(), 1);

        // The agent's presence query reflects the phone at home, with confidence.
        let shown = reg
            .call("presence_show", json!({ "site": "home" }))
            .await
            .expect("presence_show");
        let devices = shown["devices"].as_array().expect("devices");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0]["device"], json!("phone"));
        assert!(shown["person_present"]["confidence"].as_f64().unwrap_or(0.0) > 0.0);
        assert!(shown["caveat"].is_string());

        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn presence_tools_normal_and_error_paths() {
        let home = temp();
        let storage = Arc::new(Storage::new(home.clone()));
        let sites_dir = storage.sites_dir();
        std::fs::create_dir_all(&sites_dir).expect("mk sites");
        std::fs::write(
            sites_dir.join("home.json"),
            json!({ "id": "home", "name": "Home", "fingerprints": [] }).to_string(),
        )
        .expect("site");

        let mut reg = ToolRegistry::new();
        register_presence(&mut reg, storage.home(), sites_dir.clone());
        // Also register the site tools so the test can set mobility via the real
        // `device_set_mobility` path rather than a test-only setter.
        crate::sites::register(&mut reg, &sites_dir, &storage.devices_dir());

        // opt-in tool sets the flag.
        reg.call("device_set_presence_opt_in", json!({ "device": "phone", "enabled": true }))
            .await
            .expect("opt in");
        assert!(storage.device_presence_opt_in("phone"));

        // home-site tool: unknown site errors, registered site succeeds.
        assert!(reg
            .call("device_set_home_site", json!({ "device": "phone", "home_site": "nope" }))
            .await
            .is_err());
        reg.call("device_set_home_site", json!({ "device": "phone", "home_site": "home" }))
            .await
            .expect("home site");
        assert_eq!(storage.device_home_site("phone"), "home");

        // bind fingerprint: errors before any report, succeeds after one.
        assert!(reg
            .call("site_bind_fingerprint", json!({ "device": "phone", "site": "home" }))
            .await
            .is_err());
        storage
            .set_device_last_fingerprint("phone", "fp-home")
            .expect("record fp");
        reg.call("site_bind_fingerprint", json!({ "device": "phone", "site": "home" }))
            .await
            .expect("bind");
        assert_eq!(sites::site_for_fingerprint(&sites_dir, "fp-home").as_deref(), Some("home"));

        // Place the mobile phone at home, then presence_show reflects it with a caveat.
        storage.set_device_site("phone", "home", 10).expect("site");
        reg.call("device_set_mobility", json!({ "device": "phone", "mobility": "mobile" }))
            .await
            .expect("mobility");
        let shown = reg
            .call("presence_show", json!({ "site": "home" }))
            .await
            .expect("show");
        assert_eq!(shown["devices"].as_array().map(Vec::len).unwrap_or(0), 1);
        assert!(shown["person_present"]["confidence"].as_f64().unwrap_or(0.0) > 0.0);
        assert!(shown["caveat"].is_string());

        // device_presence: mobile phone whose home is `home` but currently `away`
        // leans toward a departure, never certain.
        storage.set_device_site("phone", "away", 20).expect("away");
        let dp = reg
            .call("device_presence", json!({ "device": "phone" }))
            .await
            .expect("device presence");
        assert_eq!(dp["site"], json!("away"));
        assert_eq!(dp["home_site"], json!("home"));
        let dep = dp["departure"]["confidence"].as_f64().unwrap_or(0.0);
        assert!(dep > 0.0 && dep < 1.0, "departure probabilistic, got {dep}");

        let _ = std::fs::remove_dir_all(&home);
    }
}
