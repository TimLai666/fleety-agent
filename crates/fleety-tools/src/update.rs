//! Binary self-update + multi-binary update, shared by `fleety`,
//! `fleety-server`, and `fleetyd`.
//!
//! A binary's update is driven by a JSON manifest (`{version,url,sha256}`). The
//! manifest URL comes from `FLEETY_UPDATE_MANIFEST`; when it contains the literal
//! `{bin}`, that placeholder is replaced with the binary name so one base URL can
//! serve every binary (e.g. `https://host/dl/{bin}/latest.json`). A plain URL
//! (no `{bin}`) is treated as the *current* binary's manifest — back-compat for
//! the original `fleetyd update`.
//!
//! Binaries ship in lockstep (one workspace version), so the running process's
//! `agent_core::VERSION` is the baseline for every local binary. The manifest /
//! version / hash logic is pure and unit-tested; the download + swap is exercised
//! manually (same posture as the model provider's live calls).

use std::path::Path;

use agent_core::{CoreError, Result};
use serde_json::Value;

/// One downloadable binary: where it lives and what it must hash to.
struct Artifact {
    url: String,
    sha256: String,
}

/// A parsed update manifest. `artifact` is the entry for THIS platform — absent
/// when a multi-target manifest carries nothing for the local triple (version
/// probing still works; only installing needs an artifact).
struct Manifest {
    version: String,
    artifact: Option<Artifact>,
    versioned_manifest: Option<String>,
}

/// Parse an update manifest in either supported form and select `triple`'s
/// artifact. Flat form: `{version, url, sha256}` (one artifact, any platform).
/// Multi-target form: `{version, targets: {<triple>: {url, sha256}}}` plus an
/// optional `versioned_manifest` URL template. Unknown fields are ignored so
/// future manifests stay readable by old binaries.
fn parse_manifest_for(text: &str, triple: Option<&str>) -> Result<Manifest> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| CoreError::Message(format!("invalid update manifest: {e}")))?;
    let str_field = |obj: &Value, key: &str| -> Result<String> {
        obj.get(key)
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| CoreError::Message(format!("manifest missing string '{key}'")))
    };
    let version = str_field(&v, "version")?;
    let versioned_manifest = v
        .get("versioned_manifest")
        .and_then(Value::as_str)
        .map(String::from);
    let artifact = match v.get("targets") {
        Some(targets) => {
            let targets = targets.as_object().ok_or_else(|| {
                CoreError::Message("manifest 'targets' must be an object".to_string())
            })?;
            match triple.and_then(|t| targets.get(t)) {
                // An entry for our triple must be complete; a broken one is a
                // publisher error, not a silent "no artifact".
                Some(entry) => Some(Artifact {
                    url: str_field(entry, "url")?,
                    sha256: str_field(entry, "sha256")?.to_lowercase(),
                }),
                None => None,
            }
        }
        None => Some(Artifact {
            url: str_field(&v, "url")?,
            sha256: str_field(&v, "sha256")?.to_lowercase(),
        }),
    };
    Ok(Manifest {
        version,
        artifact,
        versioned_manifest,
    })
}

/// Human-readable name of the local platform for error messages: the release
/// target triple when known, else the raw arch/os pair.
fn local_platform_desc() -> String {
    crate::deps::target_triple()
        .map(String::from)
        .unwrap_or_else(|| format!("{}/{}", std::env::consts::ARCH, std::env::consts::OS))
}

/// Whether `latest` differs from `current` (ignoring a leading `v`).
fn needs_update(current: &str, latest: &str) -> bool {
    current.trim_start_matches('v') != latest.trim_start_matches('v')
}

/// Re-export of the version-diff predicate for the background poller.
pub fn needs_update_str(current: &str, latest: &str) -> bool {
    needs_update(current, latest)
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The file stem of the running binary (e.g. `fleetyd`), used as its `bin` name.
fn current_bin_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "fleety".to_string())
}

/// Whether `FLEETY_UPDATE_MANIFEST` is a per-binary template (`{bin}` form) —
/// required to safely resolve a *different* binary's manifest.
pub fn manifest_is_templated() -> bool {
    std::env::var("FLEETY_UPDATE_MANIFEST")
        .map(|b| b.contains("{bin}"))
        .unwrap_or(false)
}

/// Whether `FLEETY_UPDATE_MANIFEST` can resolve an exact `{version}` — required
/// for forward-only convergence to a specific (server) version.
pub fn manifest_supports_version() -> bool {
    std::env::var("FLEETY_UPDATE_MANIFEST")
        .map(|b| b.contains("{version}"))
        .unwrap_or(false)
}

/// The *latest*-manifest URL for `bin`, from `FLEETY_UPDATE_MANIFEST`. `{bin}`
/// is substituted with the binary name; `{version}` — a template kept for
/// pinned resolution — is substituted with the literal `latest`, so one env var
/// serves both modes (a self-hosted layout serves a `latest` alias directory).
pub fn manifest_url_for(bin: &str) -> Result<String> {
    let base = std::env::var("FLEETY_UPDATE_MANIFEST").map_err(|_| {
        CoreError::Message(
            "set FLEETY_UPDATE_MANIFEST to the update manifest URL (may contain {bin})".to_string(),
        )
    })?;
    Ok(base.replace("{bin}", bin).replace("{version}", "latest"))
}

async fn fetch_text(url: &str) -> Result<String> {
    reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("fetch manifest failed: {e}")))?
        .text()
        .await
        .map_err(|e| CoreError::Provider(format!("read manifest failed: {e}")))
}

/// Probe the latest published version for `bin` (background poller). Works even
/// on a multi-target manifest with no artifact for this platform.
pub async fn probe_latest_for(bin: &str) -> Result<String> {
    let url = manifest_url_for(bin)?;
    Ok(parse_manifest_for(&fetch_text(&url).await?, crate::deps::target_triple())?.version)
}

/// Probe the latest version for the *running* binary.
pub async fn probe_latest() -> Result<String> {
    probe_latest_for(&current_bin_name()).await
}

/// Move `exe_path` aside and install `bytes` in its place. Works on Windows,
/// where a running exe can be renamed but not overwritten; rolls back on failure
/// so the binary stays runnable.
fn swap_exe(exe_path: &Path, bytes: &[u8]) -> Result<()> {
    let staged = exe_path.with_extension("new");
    std::fs::write(&staged, bytes)
        .map_err(|e| CoreError::Message(format!("cannot write staged binary: {e}")))?;
    let backup = exe_path.with_extension("old");
    let _ = std::fs::remove_file(&backup);
    std::fs::rename(exe_path, &backup)
        .map_err(|e| CoreError::Message(format!("cannot move current exe aside: {e}")))?;
    if let Err(e) = std::fs::rename(&staged, exe_path) {
        let _ = std::fs::rename(&backup, exe_path); // roll back
        return Err(CoreError::Message(format!("cannot install new exe: {e}")));
    }
    Ok(())
}

/// Fetch `manifest_url`; if its version differs from `current_version`, download
/// the artifact, verify its SHA-256, and install it at `exe_path`. Returns `true`
/// when a new binary was installed (the caller restarts the service to apply).
pub async fn install(
    manifest_url: &str,
    exe_path: &Path,
    label: &str,
    current_version: &str,
) -> Result<bool> {
    install_expecting(manifest_url, exe_path, label, current_version, None).await
}

/// `install`, optionally pinned: when `expect_version` is set, a manifest that
/// declares any other version is rejected before anything downloads — a
/// publisher mixup must never silently install the wrong version.
async fn install_expecting(
    manifest_url: &str,
    exe_path: &Path,
    label: &str,
    current_version: &str,
    expect_version: Option<&str>,
) -> Result<bool> {
    let manifest = parse_manifest_for(
        &fetch_text(manifest_url).await?,
        crate::deps::target_triple(),
    )?;
    verify_pinned_version(expect_version, &manifest.version)?;
    if !needs_update(current_version, &manifest.version) {
        println!("{label} is already up to date (version {current_version}).");
        return Ok(false);
    }
    let Some(artifact) = manifest.artifact else {
        return Err(CoreError::Message(format!(
            "update manifest {} has no artifact for this platform ({}); update {label} from \
             source or publish an artifact for it",
            manifest.version,
            local_platform_desc()
        )));
    };
    println!(
        "Updating {label} {current_version} -> {} ...",
        manifest.version
    );
    let bytes = reqwest::Client::new()
        .get(&artifact.url)
        .send()
        .await
        .map_err(|e| CoreError::Provider(format!("download failed: {e}")))?
        .bytes()
        .await
        .map_err(|e| CoreError::Provider(format!("read artifact failed: {e}")))?;
    let actual = sha256_hex(&bytes);
    if actual != artifact.sha256 {
        return Err(CoreError::Message(format!(
            "sha256 mismatch: manifest {}, downloaded {actual}",
            artifact.sha256
        )));
    }
    swap_exe(exe_path, &bytes)?;
    println!("Updated {label} to {}.", manifest.version);
    Ok(true)
}

/// Self-update the running binary. Returns `true` if a new binary was installed.
pub async fn self_update() -> Result<bool> {
    let exe = std::env::current_exe()
        .map_err(|e| CoreError::Message(format!("cannot find current exe: {e}")))?;
    let bin = current_bin_name();
    let url = manifest_url_for(&bin)?;
    install(&url, &exe, &bin, agent_core::VERSION).await
}

/// Update the named binary at `exe_path` using `bin`'s manifest. The running
/// process's version is the baseline (binaries ship in lockstep).
pub async fn update_named(bin: &str, exe_path: &Path) -> Result<bool> {
    let url = manifest_url_for(bin)?;
    install(&url, exe_path, bin, agent_core::VERSION).await
}

// ---- version-pinned update (fleet convergence to the server's version) ----

fn parse_semver(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim_start_matches('v');
    let core = core.split(['-', '+']).next().unwrap_or(core); // drop pre-release/build
    let mut it = core.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next().unwrap_or("0").parse().ok()?;
    let patch = it.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// Whether `a` is a strictly newer semver than `b` (ignores a leading `v` and any
/// pre-release/build suffix). Unparseable versions are treated as *not* newer, so
/// a forward-only convergence never acts on a version it can't order.
pub fn is_newer(a: &str, b: &str) -> bool {
    match (parse_semver(a), parse_semver(b)) {
        (Some(x), Some(y)) => x > y,
        _ => false,
    }
}

/// The manifest URL for a *specific* `version` of `bin`. Requires a `{version}`
/// template (pinning to an exact version is the whole point), and substitutes
/// `{bin}` too. Errors if the base can't be made version-specific.
pub fn manifest_url_for_versioned(bin: &str, version: &str) -> Result<String> {
    let base = std::env::var("FLEETY_UPDATE_MANIFEST").map_err(|_| {
        CoreError::Message("set FLEETY_UPDATE_MANIFEST to the update manifest URL".to_string())
    })?;
    if !base.contains("{version}") {
        return Err(CoreError::Message(
            "FLEETY_UPDATE_MANIFEST must contain {version} (and {bin}) to pin a device to the \
             server's version — e.g. https://host/dl/{bin}/{version}/manifest.json"
                .to_string(),
        ));
    }
    Ok(base.replace("{bin}", bin).replace("{version}", version))
}

/// A sibling binary `bin` installed next to the running process, if present.
pub fn sibling_exe(bin: &str) -> Option<std::path::PathBuf> {
    let name = if cfg!(windows) {
        format!("{bin}.exe")
    } else {
        bin.to_string()
    };
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .filter(|p| p.exists())
}

/// Whether `template` can safely name `bin`'s manifest from a process running
/// as `current_bin`. The running binary always can (a plain URL IS its own
/// manifest); any other binary needs a `{bin}` placeholder.
fn manifest_covers_bin(template: &str, bin: &str, current_bin: &str) -> bool {
    bin == current_bin || template.contains("{bin}")
}

/// Pinned-version guard: a manifest fetched to pin `expected` must declare that
/// exact version (leading `v` ignored) — a publisher mixup is refused loudly.
fn verify_pinned_version(expected: Option<&str>, got: &str) -> Result<()> {
    match expected {
        Some(expected) if needs_update(expected, got) => Err(CoreError::Message(format!(
            "update manifest declares version {got}, expected {expected}; refusing to install"
        ))),
        _ => Ok(()),
    }
}

/// How to converge a binary to an exact target version, decided from its
/// *latest* manifest (used when the env template can't pin by itself).
#[derive(Debug, PartialEq, Eq)]
pub enum PinResolution {
    /// The latest manifest already declares the target version — install from it.
    UseLatest,
    /// Fetch this URL: the latest manifest's `versioned_manifest` template with
    /// `{bin}` and `{version}` resolved.
    FetchVersioned(String),
    /// No way to reach the exact version; human-actionable reason.
    CannotPin(String),
}

/// Pure decision for the convergence chain: given the latest manifest's version
/// and its optional `versioned_manifest` template, how does `bin` reach
/// `target_version` exactly?
pub fn resolve_pin(
    latest_version: &str,
    versioned_manifest: Option<&str>,
    bin: &str,
    target_version: &str,
) -> PinResolution {
    if !needs_update(target_version, latest_version) {
        return PinResolution::UseLatest;
    }
    match versioned_manifest {
        Some(template) => PinResolution::FetchVersioned(
            template
                .replace("{bin}", bin)
                .replace("{version}", target_version.trim_start_matches('v')),
        ),
        None => PinResolution::CannotPin(format!(
            "latest manifest is version {latest_version}, not {target_version}, and carries no \
             versioned_manifest template; publish manifests with one, or set \
             FLEETY_UPDATE_MANIFEST to a {{version}} template"
        )),
    }
}

/// Update `bin` at `exe_path` to an exact `version` (via its versioned manifest;
/// a manifest declaring any other version is refused).
pub async fn update_to_version(bin: &str, exe_path: &Path, version: &str) -> Result<bool> {
    let url = manifest_url_for_versioned(bin, version)?;
    install_expecting(&url, exe_path, bin, agent_core::VERSION, Some(version)).await
}

/// Self-update the running binary to an exact `version`.
pub async fn self_update_to_version(version: &str) -> Result<bool> {
    let exe = std::env::current_exe()
        .map_err(|e| CoreError::Message(format!("cannot find current exe: {e}")))?;
    update_to_version(&current_bin_name(), &exe, version).await
}

/// Converge `bin` at `exe_path` to exactly `version`, picking the resolution
/// path: an env `{version}` template pins directly; otherwise the binary's
/// latest manifest either already declares the version or names the pinned
/// manifest via its `versioned_manifest` template. Every manifest fetched to
/// pin is verified to declare the target version before anything installs.
pub async fn converge_to_version(bin: &str, exe_path: &Path, version: &str) -> Result<bool> {
    // Sibling guard: without {bin}, the template resolves to the RUNNING
    // binary's manifest — installing from it would write this binary's bytes
    // over `bin`'s executable. Refuse with the fix spelled out.
    let template = std::env::var("FLEETY_UPDATE_MANIFEST").unwrap_or_default();
    if !manifest_covers_bin(&template, bin, &current_bin_name()) {
        return Err(CoreError::Message(format!(
            "FLEETY_UPDATE_MANIFEST has no {{bin}} placeholder, so it cannot name {bin}'s \
             manifest from this process; add {{bin}} to the template (e.g. \
             https://host/dl/{{bin}}/latest.json) to update sibling binaries"
        )));
    }
    if manifest_supports_version() {
        return update_to_version(bin, exe_path, version).await;
    }
    let latest_url = manifest_url_for(bin)?;
    let latest = parse_manifest_for(
        &fetch_text(&latest_url).await?,
        crate::deps::target_triple(),
    )?;
    let url = match resolve_pin(
        &latest.version,
        latest.versioned_manifest.as_deref(),
        bin,
        version,
    ) {
        // Re-fetching the latest URL is TOCTOU-safe: the pinned-version check
        // inside install rejects a manifest that moved between the two fetches.
        PinResolution::UseLatest => latest_url,
        PinResolution::FetchVersioned(url) => url,
        PinResolution::CannotPin(reason) => return Err(CoreError::Message(reason)),
    };
    install_expecting(&url, exe_path, bin, agent_core::VERSION, Some(version)).await
}

/// Converge the running binary itself to exactly `version`.
pub async fn converge_self_to_version(version: &str) -> Result<bool> {
    let exe = std::env::current_exe()
        .map_err(|e| CoreError::Message(format!("cannot find current exe: {e}")))?;
    converge_to_version(&current_bin_name(), &exe, version).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_flat_manifest_reads_fields() {
        let m = parse_manifest_for(
            r#"{"version":"0.2.0","url":"https://x/y","sha256":"AABB"}"#,
            None,
        )
        .expect("parse");
        assert_eq!(m.version, "0.2.0");
        let a = m.artifact.expect("flat form always has an artifact");
        assert_eq!(a.url, "https://x/y");
        assert_eq!(a.sha256, "aabb"); // lowercased
        assert!(m.versioned_manifest.is_none());
        assert!(parse_manifest_for(r#"{"version":"1"}"#, None).is_err()); // missing fields
    }

    #[test]
    fn parse_flat_manifest_ignores_unknown_fields() {
        let m = parse_manifest_for(
            r#"{"version":"0.2.0","url":"https://x/y","sha256":"aa","future_field":{"x":1}}"#,
            Some("x86_64-unknown-linux-gnu"),
        )
        .expect("unknown fields are forward-compatible");
        assert_eq!(m.version, "0.2.0");
        assert!(m.artifact.is_some());
    }

    const MULTI: &str = r#"{
        "version": "0.2.0",
        "versioned_manifest": "https://h/dl/{bin}/{version}/m.json",
        "targets": {
            "x86_64-unknown-linux-gnu": {"url": "https://h/lin", "sha256": "AA11"},
            "aarch64-apple-darwin": {"url": "https://h/mac", "sha256": "bb22"}
        }
    }"#;

    #[test]
    fn parse_multi_target_selects_local_triple() {
        let m = parse_manifest_for(MULTI, Some("x86_64-unknown-linux-gnu")).expect("parse");
        assert_eq!(m.version, "0.2.0");
        let a = m.artifact.expect("entry for this triple");
        assert_eq!(a.url, "https://h/lin");
        assert_eq!(a.sha256, "aa11"); // lowercased
        assert_eq!(
            m.versioned_manifest.as_deref(),
            Some("https://h/dl/{bin}/{version}/m.json")
        );
    }

    #[test]
    fn parse_multi_target_without_local_triple_probes_but_has_no_artifact() {
        // Version stays readable (notify polling works); only installing needs
        // an artifact and errors later with a clear message.
        for triple in [Some("riscv64gc-unknown-linux-gnu"), None] {
            let m = parse_manifest_for(MULTI, triple).expect("version must parse");
            assert_eq!(m.version, "0.2.0");
            assert!(m.artifact.is_none());
        }
        // A malformed entry for OUR triple is an error, not a silent skip.
        let bad = r#"{"version":"1","targets":{"x86_64-unknown-linux-gnu":{"url":"https://h"}}}"#;
        assert!(parse_manifest_for(bad, Some("x86_64-unknown-linux-gnu")).is_err());
    }

    #[test]
    fn needs_update_ignores_v_prefix() {
        assert!(needs_update("0.1.0", "0.2.0"));
        assert!(!needs_update("0.1.0", "v0.1.0"));
        assert!(!needs_update("v0.1.0", "0.1.0"));
    }

    #[test]
    fn sha256_matches_known_vector() {
        // SHA-256("abc")
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn is_newer_orders_semver() {
        assert!(is_newer("0.2.0", "0.1.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("v0.1.1", "0.1.0")); // leading v ignored
        assert!(!is_newer("0.1.0", "0.1.0")); // equal → not newer
        assert!(!is_newer("0.1.0", "0.2.0")); // older → not newer
        assert!(!is_newer("garbage", "0.1.0")); // unparseable → not newer (safe)
    }

    #[test]
    fn sibling_needs_bin_placeholder_in_template() {
        // A {version}-only template resolves to the RUNNING binary's manifest:
        // fine for self-update, refused for siblings — installing from it would
        // write the running binary's bytes over the sibling's executable.
        let versioned_only = "https://h/dl/fleetyd/{version}/m.json";
        assert!(manifest_covers_bin(versioned_only, "fleetyd", "fleetyd"));
        assert!(!manifest_covers_bin(
            versioned_only,
            "fleety-server",
            "fleetyd"
        ));
        assert!(!manifest_covers_bin(
            "https://h/plain.json",
            "fleety",
            "fleetyd"
        ));
        // A {bin} template resolves any binary.
        let templated = "https://h/dl/{bin}/latest.json";
        assert!(manifest_covers_bin(templated, "fleety-server", "fleetyd"));
        assert!(manifest_covers_bin(templated, "fleetyd", "fleetyd"));
    }

    #[test]
    fn resolve_pin_three_branches() {
        // Latest already matches the target (v prefix ignored) → use it directly.
        assert_eq!(
            resolve_pin(
                "0.3.0",
                Some("https://h/{bin}/{version}"),
                "fleetyd",
                "v0.3.0"
            ),
            PinResolution::UseLatest
        );
        // Latest moved past the target → follow its versioned_manifest template.
        assert_eq!(
            resolve_pin(
                "0.4.0",
                Some("https://h/dl/{bin}/{version}/m.json"),
                "fleetyd",
                "0.3.0"
            ),
            PinResolution::FetchVersioned("https://h/dl/fleetyd/0.3.0/m.json".to_string())
        );
        // No template → cannot pin; the reason names both remedies.
        match resolve_pin("0.4.0", None, "fleetyd", "0.3.0") {
            PinResolution::CannotPin(reason) => {
                assert!(reason.contains("versioned_manifest"));
                assert!(reason.contains("{version}"));
            }
            other => panic!("expected CannotPin, got {other:?}"),
        }
    }

    #[test]
    fn pinned_manifest_version_mismatch_is_rejected() {
        // A manifest fetched to pin V must declare V — anything else is refused.
        let err = verify_pinned_version(Some("0.3.0"), "0.4.0").expect_err("mismatch");
        let msg = err.report().message;
        assert!(msg.contains("0.4.0") && msg.contains("0.3.0"));
        assert!(verify_pinned_version(Some("0.3.0"), "v0.3.0").is_ok()); // v prefix ignored
        assert!(verify_pinned_version(None, "9.9.9").is_ok()); // unpinned: anything goes
    }

    #[test]
    #[serial_test::serial]
    fn versioned_manifest_requires_version_placeholder() {
        std::env::set_var(
            "FLEETY_UPDATE_MANIFEST",
            "https://h/dl/{bin}/{version}/m.json",
        );
        assert_eq!(
            manifest_url_for_versioned("fleetyd", "0.3.0").unwrap(),
            "https://h/dl/fleetyd/0.3.0/m.json"
        );
        // No {version} → can't pin → error.
        std::env::set_var("FLEETY_UPDATE_MANIFEST", "https://h/dl/{bin}/latest.json");
        assert!(manifest_url_for_versioned("fleetyd", "0.3.0").is_err());
        std::env::remove_var("FLEETY_UPDATE_MANIFEST");
    }

    #[test]
    #[serial_test::serial]
    fn latest_resolution_substitutes_version_with_literal_latest() {
        // A {version} template must not break latest resolution: polling and the
        // update commands resolve it to a `latest` alias path, while pinned
        // resolution keeps substituting the exact version.
        std::env::set_var(
            "FLEETY_UPDATE_MANIFEST",
            "https://h/dl/{bin}/{version}/m.json",
        );
        assert_eq!(
            manifest_url_for("fleetyd").unwrap(),
            "https://h/dl/fleetyd/latest/m.json"
        );
        assert_eq!(
            manifest_url_for_versioned("fleetyd", "0.3.0").unwrap(),
            "https://h/dl/fleetyd/0.3.0/m.json"
        );
        std::env::remove_var("FLEETY_UPDATE_MANIFEST");
    }

    #[test]
    #[serial_test::serial]
    fn manifest_url_substitutes_bin_when_templated() {
        std::env::set_var("FLEETY_UPDATE_MANIFEST", "https://h/dl/{bin}/latest.json");
        assert!(manifest_is_templated());
        assert_eq!(
            manifest_url_for("fleety-server").unwrap(),
            "https://h/dl/fleety-server/latest.json"
        );
        // A plain URL is used as-is (current-binary back-compat).
        std::env::set_var("FLEETY_UPDATE_MANIFEST", "https://h/fleetyd.json");
        assert!(!manifest_is_templated());
        assert_eq!(
            manifest_url_for("fleetyd").unwrap(),
            "https://h/fleetyd.json"
        );
        std::env::remove_var("FLEETY_UPDATE_MANIFEST");
    }
}
