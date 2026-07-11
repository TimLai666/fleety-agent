//! Startup dependency self-check & ensure, shared by fleetyd and fleety-server.
//!
//! Some features need external runtimes/tools — the ddgs search MCP (a Python
//! package), node/python MCP servers and skill scripts, the insyra sidecar. On
//! startup each binary ensures its own subset: a present dependency is left
//! alone; a missing one is installed via its [`Strategy`]. Everything is
//! **best-effort and non-blocking** — any failure is logged with an actionable
//! message and never stops the service (the boot path spawns this, never awaits
//! it as a gate).
//!
//! Three strategies, all root-free and non-polluting:
//! - **ManagedBinary** — download a single binary into a fleety dir (insyra).
//! - **UserPackage** — user-level package install (ddgs: pipx / pip --user).
//! - **ManagedRuntime** — a portable language runtime into `~/.fleety/runtimes/`
//!   with its bin dir prepended to *this process's* PATH, so children inherit it
//!   (python via `uv`, node via the official portable build). See [`runtime`].
//!
//! The behavior carried by each dependency lives in injectable probe/install
//! closures, so the framework logic is unit-testable without touching the
//! network; the platform URL/path/PATH mapping is pure and tested too.

use std::sync::Arc;

use agent_core::Result;

pub mod insyra;
pub mod runtime;

/// node (managed portable runtime) as a framework dependency.
pub fn node_dependency() -> Dependency {
    Dependency::new(
        "node",
        Strategy::ManagedRuntime,
        None,
        || async { runtime::has_node().await },
        || async { runtime::ensure_node().await },
    )
}

/// python (managed via uv) as a framework dependency.
pub fn python_dependency() -> Dependency {
    Dependency::new(
        "python",
        Strategy::ManagedRuntime,
        None,
        || async { runtime::has_python().await },
        || async { runtime::ensure_python().await },
    )
}

pub use insyra::insyra_dependency;

/// The release asset target triple for this build, or None if unsupported.
/// Shared by the insyra sidecar URL logic and the binary updater's multi-target
/// manifest resolution — one table, so the two can't drift.
///
/// ARM/RISC-V Linux entries have no Rust release target yet — fleetyd is built
/// from source there — but the release still ships a cross-built sidecar for
/// them (the `sidecar-cross` job in release.yml; keep both lists in sync). The
/// static pure-Go binary runs on glibc and musl systems alike, so one asset per
/// arch/os is enough. No 32-bit ARM: Insyra's thrift dependency doesn't build
/// where Go's `int` is 32-bit.
pub fn target_triple() -> Option<&'static str> {
    match (std::env::consts::ARCH, std::env::consts::OS) {
        ("x86_64", "linux") => Some("x86_64-unknown-linux-gnu"),
        ("aarch64", "linux") => Some("aarch64-unknown-linux-gnu"),
        ("riscv64", "linux") => Some("riscv64gc-unknown-linux-gnu"),
        ("aarch64", "macos") => Some("aarch64-apple-darwin"),
        ("x86_64", "macos") => Some("x86_64-apple-darwin"),
        ("x86_64", "windows") => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

/// How a missing dependency gets installed. Informational (the actual work is in
/// the dependency's `install` closure) but documents intent and shapes messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    ManagedBinary,
    UserPackage,
    ManagedRuntime,
}

type BoxFut<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send>>;
/// Returns true when the dependency is already usable.
pub type ProbeFn = Arc<dyn Fn() -> BoxFut<bool> + Send + Sync>;
/// Installs the dependency (best-effort); `Err` is reported, not fatal.
pub type InstallFn = Arc<dyn Fn() -> BoxFut<Result<()>> + Send + Sync>;

/// One dependency the framework drives: how to detect it and how to install it.
#[derive(Clone)]
pub struct Dependency {
    pub name: String,
    pub strategy: Strategy,
    /// Optional per-dependency opt-out env var (e.g. `FLEETY_DDGS_AUTO_INSTALL`);
    /// set to `"0"` to disable installing just this one.
    pub env_key: Option<String>,
    pub probe: ProbeFn,
    pub install: InstallFn,
}

impl Dependency {
    pub fn new<P, PF, I, IF>(
        name: impl Into<String>,
        strategy: Strategy,
        env_key: Option<&str>,
        probe: P,
        install: I,
    ) -> Self
    where
        P: Fn() -> PF + Send + Sync + 'static,
        PF: std::future::Future<Output = bool> + Send + 'static,
        I: Fn() -> IF + Send + Sync + 'static,
        IF: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        Self {
            name: name.into(),
            strategy,
            env_key: env_key.map(String::from),
            probe: Arc::new(move || Box::pin(probe())),
            install: Arc::new(move || Box::pin(install())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnsureStatus {
    /// Already present; nothing done.
    Already,
    /// Was missing; installed successfully.
    Installed,
    /// Missing but auto-install disabled (globally or per-dep).
    Skipped,
    /// Missing; install attempted and failed (best-effort — service still runs).
    Failed,
}

#[derive(Debug, Clone)]
pub struct EnsureOutcome {
    pub name: String,
    pub status: EnsureStatus,
    pub message: String,
}

/// Global auto-install switch. On unless `FLEETY_AUTO_INSTALL_DEPS=0`.
pub fn auto_install_enabled() -> bool {
    std::env::var("FLEETY_AUTO_INSTALL_DEPS").as_deref() != Ok("0")
}

/// Whether a specific dependency's per-dep env opt-out allows installing it.
fn dep_enabled(dep: &Dependency) -> bool {
    match &dep.env_key {
        Some(k) => std::env::var(k).as_deref() != Ok("0"),
        None => true,
    }
}

/// Pure: the dependency names a binary should ensure — the `FLEETY_DEPS`
/// comma-list override if non-empty, else the binary's default subset.
pub fn selected_dep_names(env_val: Option<&str>, default: &[&str]) -> Vec<String> {
    if let Some(raw) = env_val {
        let names: Vec<String> = raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !names.is_empty() {
            return names;
        }
    }
    default.iter().map(|s| s.to_string()).collect()
}

/// fleety-server's default dependency subset.
pub fn server_default_deps() -> &'static [&'static str] {
    &["python", "ddgs", "node", "insyra"]
}

/// fleetyd's default dependency subset.
pub fn daemon_default_deps() -> &'static [&'static str] {
    &["insyra"]
}

/// Drive a dependency list: probe each, install the missing ones per policy.
/// Best-effort and total — every entry yields an outcome; nothing panics or
/// propagates an error. The caller spawns this so boot is never blocked.
pub async fn ensure_all(deps: &[Dependency]) -> Vec<EnsureOutcome> {
    let global = auto_install_enabled();
    let mut out = Vec::with_capacity(deps.len());
    for dep in deps {
        if (dep.probe)().await {
            out.push(EnsureOutcome {
                name: dep.name.clone(),
                status: EnsureStatus::Already,
                message: format!("{} already present", dep.name),
            });
            continue;
        }
        if !global || !dep_enabled(dep) {
            out.push(EnsureOutcome {
                name: dep.name.clone(),
                status: EnsureStatus::Skipped,
                message: format!(
                    "{} missing; auto-install disabled (set FLEETY_AUTO_INSTALL_DEPS / {} to enable)",
                    dep.name,
                    dep.env_key.as_deref().unwrap_or("FLEETY_AUTO_INSTALL_DEPS")
                ),
            });
            continue;
        }
        match (dep.install)().await {
            Ok(()) => out.push(EnsureOutcome {
                name: dep.name.clone(),
                status: EnsureStatus::Installed,
                message: format!("{} installed", dep.name),
            }),
            Err(e) => out.push(EnsureOutcome {
                name: dep.name.clone(),
                status: EnsureStatus::Failed,
                message: format!(
                    "{} install failed (service still running): {}",
                    dep.name,
                    e.report().message
                ),
            }),
        }
    }
    out
}

/// Log a set of outcomes at appropriate levels (failures as actionable warns).
pub fn log_outcomes(outcomes: &[EnsureOutcome]) {
    for o in outcomes {
        match o.status {
            EnsureStatus::Failed => tracing::warn!(dep = %o.name, "{}", o.message),
            EnsureStatus::Skipped => tracing::info!(dep = %o.name, "{}", o.message),
            _ => tracing::info!(dep = %o.name, "{}", o.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn dep_with(
        name: &str,
        present: bool,
        install_ok: bool,
        counter: Arc<AtomicUsize>,
    ) -> Dependency {
        let c = Arc::clone(&counter);
        Dependency::new(
            name,
            Strategy::UserPackage,
            None,
            move || {
                let present = present;
                async move { present }
            },
            move || {
                let c = Arc::clone(&c);
                async move {
                    c.fetch_add(1, Ordering::SeqCst);
                    if install_ok {
                        Ok(())
                    } else {
                        Err(agent_core::CoreError::Message("boom".to_string()))
                    }
                }
            },
        )
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn present_is_not_installed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let deps = [dep_with("x", true, true, Arc::clone(&calls))];
        let out = ensure_all(&deps).await;
        assert_eq!(out[0].status, EnsureStatus::Already);
        assert_eq!(calls.load(Ordering::SeqCst), 0, "install must not run");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn missing_then_installed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let deps = [dep_with("x", false, true, Arc::clone(&calls))];
        let out = ensure_all(&deps).await;
        assert_eq!(out[0].status, EnsureStatus::Installed);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn failed_install_does_not_block() {
        let calls = Arc::new(AtomicUsize::new(0));
        let deps = [dep_with("x", false, false, Arc::clone(&calls))];
        // The point: ensure_all returns normally (no panic / no Err) with Failed.
        let out = ensure_all(&deps).await;
        assert_eq!(out[0].status, EnsureStatus::Failed);
        assert!(out[0].message.contains("still running"));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn global_off_only_detects() {
        std::env::set_var("FLEETY_AUTO_INSTALL_DEPS", "0");
        let calls = Arc::new(AtomicUsize::new(0));
        let deps = [dep_with("x", false, true, Arc::clone(&calls))];
        let out = ensure_all(&deps).await;
        std::env::remove_var("FLEETY_AUTO_INSTALL_DEPS");
        assert_eq!(out[0].status, EnsureStatus::Skipped);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn deps_filter_override_then_default() {
        assert_eq!(
            selected_dep_names(Some("node, python"), &["a", "b"]),
            vec!["node".to_string(), "python".to_string()]
        );
        // Empty / whitespace override falls back to default.
        assert_eq!(
            selected_dep_names(Some("   "), &["a", "b"]),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(selected_dep_names(None, &["a"]), vec!["a".to_string()]);
    }

    #[test]
    fn default_subsets() {
        assert_eq!(server_default_deps(), &["python", "ddgs", "node", "insyra"]);
        assert_eq!(daemon_default_deps(), &["insyra"]);
    }
}
