//! Site (場域 / location) registry: group devices by physical location so the
//! agent understands which devices live where (e.g. devices at "home-office"
//! vs "warehouse"). Sites are first-class records under `fleet/sites/`; each
//! `device.json` carries a `site` field set via `device_set_site`.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

/// Register the site tools, persisting sites under `sites_dir` and reading
/// device records from `devices_dir`.
pub fn register(registry: &mut ToolRegistry, sites_dir: &Path, devices_dir: &Path) {
    registry.register(Box::new(SiteSet {
        dir: sites_dir.to_path_buf(),
    }));
    registry.register(Box::new(SiteList {
        dir: sites_dir.to_path_buf(),
    }));
    registry.register(Box::new(SiteDelete {
        dir: sites_dir.to_path_buf(),
    }));
    registry.register(Box::new(SiteShow {
        dir: sites_dir.to_path_buf(),
        devices_dir: devices_dir.to_path_buf(),
    }));
    registry.register(Box::new(DeviceSetSite {
        sites_dir: sites_dir.to_path_buf(),
        devices_dir: devices_dir.to_path_buf(),
    }));
    registry.register(Box::new(DeviceSetMobility {
        devices_dir: devices_dir.to_path_buf(),
    }));
}

/// Read a device record, set one field, and write it back.
fn update_device_field(devices_dir: &Path, device: &str, key: &str, value: Value) -> Result<()> {
    let record_path = devices_dir.join(device).join("device.json");
    let text = std::fs::read_to_string(&record_path).map_err(|e| {
        CoreError::Message(format!(
            "no such device '{device}' ({e}); use device_list to see devices"
        ))
    })?;
    let mut record: Value = serde_json::from_str(&text)
        .map_err(|e| CoreError::Message(format!("corrupt device record: {e}")))?;
    if let Some(obj) = record.as_object_mut() {
        obj.insert(key.to_string(), value);
    }
    let pretty = serde_json::to_string_pretty(&record)
        .map_err(|e| CoreError::Message(format!("serialize device record: {e}")))?;
    std::fs::write(&record_path, pretty)
        .map_err(|e| CoreError::Message(format!("write device record: {e}")))
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

fn safe_id(kind: &str, id: &str) -> Result<()> {
    if id.is_empty()
        || id.contains('/')
        || id.contains('\\')
        || id.contains("..")
        || id.contains(':')
    {
        return Err(CoreError::Message(format!(
            "invalid {kind} '{id}': no path separators, ':' or '..'"
        )));
    }
    Ok(())
}

/// Devices whose `device.json` `site` field equals `site`.
fn devices_at(devices_dir: &Path, site: &str) -> Vec<Value> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(devices_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(entry.path().join("device.json")) {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if value.get("site").and_then(Value::as_str) == Some(site) {
                    out.push(value);
                }
            }
        }
    }
    out
}

/// Add a fingerprint to a site's set (idempotent). Errors if the site is absent.
pub(crate) fn add_fingerprint_to_site(sites_dir: &Path, site: &str, fingerprint: &str) -> Result<()> {
    safe_id("site id", site)?;
    let path = sites_dir.join(format!("{site}.json"));
    let text = std::fs::read_to_string(&path).map_err(|e| {
        CoreError::Message(format!(
            "no such site '{site}' ({e}); register it with site_set first"
        ))
    })?;
    let mut record: Value = serde_json::from_str(&text)
        .map_err(|e| CoreError::Message(format!("corrupt site record: {e}")))?;
    let mut fps: Vec<String> = record
        .get("fingerprints")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    if !fps.iter().any(|f| f == fingerprint) {
        fps.push(fingerprint.to_string());
    }
    record["fingerprints"] = json!(fps);
    let pretty = serde_json::to_string_pretty(&record)
        .map_err(|e| CoreError::Message(format!("serialize site: {e}")))?;
    std::fs::write(&path, pretty).map_err(|e| CoreError::Message(format!("write site: {e}")))
}

/// The site whose fingerprint set contains `fingerprint`, if any.
pub(crate) fn site_for_fingerprint(sites_dir: &Path, fingerprint: &str) -> Option<String> {
    let entries = std::fs::read_dir(sites_dir).ok()?;
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let has = value
            .get("fingerprints")
            .and_then(Value::as_array)
            .map(|a| a.iter().any(|x| x.as_str() == Some(fingerprint)))
            .unwrap_or(false);
        if has {
            if let Some(id) = value.get("id").and_then(Value::as_str) {
                return Some(id.to_string());
            }
        }
    }
    None
}

struct SiteSet {
    dir: PathBuf,
}

#[async_trait]
impl Tool for SiteSet {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "site_set".to_string(),
            description: "Create or update a site (location): `id` plus optional `name` and `description`. Assign devices to it with device_set_site.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "stable site id, e.g. home-office" },
                    "name": { "type": "string" },
                    "description": { "type": "string" }
                },
                "required": ["id"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let id = require_str(&args, "id")?;
        safe_id("site id", id)?;
        let name = args.get("name").and_then(Value::as_str).unwrap_or(id);
        let description = args
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("");
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| CoreError::Message(format!("cannot create sites dir: {e}")))?;
        // Preserve fingerprints (and any other fields) already bound to this site
        // so re-running site_set to edit the name/description does not wipe them.
        let path = self.dir.join(format!("{id}.json"));
        let mut record = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),
            Err(_) => Value::Null,
        };
        if !record.is_object() {
            record = json!({ "fingerprints": [] });
        }
        record["id"] = json!(id);
        record["name"] = json!(name);
        record["description"] = json!(description);
        if record.get("fingerprints").and_then(Value::as_array).is_none() {
            record["fingerprints"] = json!([]);
        }
        let pretty = serde_json::to_string_pretty(&record)
            .map_err(|e| CoreError::Message(format!("serialize site: {e}")))?;
        std::fs::write(&path, pretty)
            .map_err(|e| CoreError::Message(format!("write site: {e}")))?;
        Ok(json!({ "id": id, "saved": true }))
    }
}

struct SiteList {
    dir: PathBuf,
}

#[async_trait]
impl Tool for SiteList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "site_list".to_string(),
            description: "List known sites (locations).".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let mut sites = Vec::new();
        match std::fs::read_dir(&self.dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    if entry.path().extension().and_then(|e| e.to_str()) == Some("json") {
                        if let Ok(text) = std::fs::read_to_string(entry.path()) {
                            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                                sites.push(value);
                            }
                        }
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(CoreError::Message(format!("cannot list sites: {e}"))),
        }
        Ok(json!({ "sites": sites }))
    }
}

struct SiteDelete {
    dir: PathBuf,
}

#[async_trait]
impl Tool for SiteDelete {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "site_delete".to_string(),
            description: "Delete a site by id (does not change device records).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let id = require_str(&args, "id")?;
        safe_id("site id", id)?;
        let path = self.dir.join(format!("{id}.json"));
        if !path.exists() {
            return Err(CoreError::Message(format!("no such site '{id}'")));
        }
        std::fs::remove_file(&path)
            .map_err(|e| CoreError::Message(format!("cannot delete site: {e}")))?;
        Ok(json!({ "id": id, "deleted": true }))
    }
}

struct SiteShow {
    dir: PathBuf,
    devices_dir: PathBuf,
}

#[async_trait]
impl Tool for SiteShow {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "site_show".to_string(),
            description: "Show a site and the devices located there.".to_string(),
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
        safe_id("site id", site)?;
        let record: Value = match std::fs::read_to_string(self.dir.join(format!("{site}.json"))) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| CoreError::Message(format!("corrupt site record: {e}")))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(CoreError::Message(format!(
                    "no such site '{site}'; use site_list to see known sites"
                )))
            }
            Err(e) => return Err(CoreError::Message(format!("cannot read site: {e}"))),
        };
        let devices = devices_at(&self.devices_dir, site);
        Ok(json!({ "site": record, "devices": devices }))
    }
}

struct DeviceSetSite {
    sites_dir: PathBuf,
    devices_dir: PathBuf,
}

#[async_trait]
impl Tool for DeviceSetSite {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "device_set_site".to_string(),
            description: "Set a device's current site. Use a registered site id, or the reserved `away` / `unknown` for a mobile device that's elsewhere or untracked. A device that relocates: just set its new site.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "device": { "type": "string" },
                    "site": { "type": "string", "description": "a registered site id, or 'away' / 'unknown'" }
                },
                "required": ["device", "site"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let device = require_str(&args, "device")?;
        let site = require_str(&args, "site")?;
        safe_id("device id", device)?;
        safe_id("site id", site)?;
        // `away`/`unknown` are reserved transient states (mobile / in-transit
        // devices); any other site must be registered first.
        let reserved = matches!(site, "away" | "unknown");
        if !reserved && !self.sites_dir.join(format!("{site}.json")).exists() {
            return Err(CoreError::Message(format!(
                "unknown site '{site}'; register it with site_set, or use 'away'/'unknown' for a device in transit"
            )));
        }
        update_device_field(&self.devices_dir, device, "site", json!(site))?;
        Ok(json!({ "device": device, "site": site, "set": true }))
    }
}

struct DeviceSetMobility {
    devices_dir: PathBuf,
}

#[async_trait]
impl Tool for DeviceSetMobility {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "device_set_mobility".to_string(),
            description: "Set a device's mobility: `stationary` (fixed in one place), `mobile` (moves around — its site is a current, changing value), or `unknown`.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "device": { "type": "string" },
                    "mobility": { "type": "string", "enum": ["stationary", "mobile", "unknown"] }
                },
                "required": ["device", "mobility"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let device = require_str(&args, "device")?;
        let mobility = require_str(&args, "mobility")?;
        safe_id("device id", device)?;
        if !matches!(mobility, "stationary" | "mobile" | "unknown") {
            return Err(CoreError::Message(format!(
                "mobility must be stationary|mobile|unknown, got '{mobility}'"
            )));
        }
        update_device_field(&self.devices_dir, device, "mobility", json!(mobility))?;
        Ok(json!({ "device": device, "mobility": mobility, "set": true }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleety-sites-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("mk temp");
        dir
    }

    fn write_device(devices_dir: &Path, id: &str, site: &str) {
        let dir = devices_dir.join(id);
        std::fs::create_dir_all(&dir).expect("mk device dir");
        std::fs::write(
            dir.join("device.json"),
            json!({ "id": id, "site": site }).to_string(),
        )
        .expect("write device.json");
    }

    #[tokio::test]
    async fn site_crud_and_device_assignment() {
        let root = temp();
        let sites_dir = root.join("sites");
        let devices_dir = root.join("devices");
        write_device(&devices_dir, "pi", "unknown");

        let mut registry = ToolRegistry::new();
        register(&mut registry, &sites_dir, &devices_dir);

        // Can't assign to an unregistered site.
        assert!(registry
            .call("device_set_site", json!({ "device": "pi", "site": "home" }))
            .await
            .is_err());

        // Register the site, then assign.
        registry
            .call("site_set", json!({ "id": "home", "name": "Home Office" }))
            .await
            .expect("site_set");
        let listed = registry.call("site_list", json!({})).await.expect("list");
        assert_eq!(listed["sites"].as_array().map(Vec::len).unwrap_or(0), 1);

        registry
            .call("device_set_site", json!({ "device": "pi", "site": "home" }))
            .await
            .expect("assign");

        // site_show lists the device now located there.
        let shown = registry
            .call("site_show", json!({ "site": "home" }))
            .await
            .expect("show");
        assert_eq!(shown["site"]["name"], json!("Home Office"));
        let devices = shown["devices"].as_array().expect("devices");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0]["id"], json!("pi"));

        // A mobile device elsewhere: reserved `away`/`unknown` need no registration.
        registry
            .call("device_set_site", json!({ "device": "pi", "site": "away" }))
            .await
            .expect("away");
        registry
            .call(
                "device_set_mobility",
                json!({ "device": "pi", "mobility": "mobile" }),
            )
            .await
            .expect("mobility");
        assert!(registry
            .call(
                "device_set_mobility",
                json!({ "device": "pi", "mobility": "teleport" })
            )
            .await
            .is_err());
        let raw =
            std::fs::read_to_string(devices_dir.join("pi").join("device.json")).expect("read");
        let rec: Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(rec["site"], json!("away"));
        assert_eq!(rec["mobility"], json!("mobile"));

        // Path-escape guard.
        assert!(registry
            .call("site_set", json!({ "id": "../evil" }))
            .await
            .is_err());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn site_fingerprints_default_empty_add_and_lookup() {
        let root = temp();
        let sites_dir = root.join("sites");
        let devices_dir = root.join("devices");
        let mut registry = ToolRegistry::new();
        register(&mut registry, &sites_dir, &devices_dir);

        // A legacy site record without a `fingerprints` field reads as empty.
        std::fs::create_dir_all(&sites_dir).expect("mk sites");
        std::fs::write(
            sites_dir.join("home.json"),
            json!({ "id": "home", "name": "Home" }).to_string(),
        )
        .expect("legacy site");
        assert_eq!(site_for_fingerprint(&sites_dir, "fp-1"), None);

        // Binding a fingerprint is idempotent and looks up by fingerprint.
        add_fingerprint_to_site(&sites_dir, "home", "fp-1").expect("bind");
        add_fingerprint_to_site(&sites_dir, "home", "fp-1").expect("bind again");
        assert_eq!(site_for_fingerprint(&sites_dir, "fp-1").as_deref(), Some("home"));
        // Idempotent: the fingerprint appears exactly once in the record.
        let raw = std::fs::read_to_string(sites_dir.join("home.json")).expect("read");
        let rec: Value = serde_json::from_str(&raw).expect("json");
        assert_eq!(rec["fingerprints"], json!(["fp-1"]));

        // Binding to an unregistered site errors.
        assert!(add_fingerprint_to_site(&sites_dir, "ghost", "fp-2").is_err());

        // Re-running site_set to edit the name preserves the bound fingerprints.
        registry
            .call("site_set", json!({ "id": "home", "name": "Home Office" }))
            .await
            .expect("edit name");
        assert_eq!(site_for_fingerprint(&sites_dir, "fp-1").as_deref(), Some("home"));

        let _ = std::fs::remove_dir_all(&root);
    }
}
