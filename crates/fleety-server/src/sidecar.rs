//! Sidecar binary lookup for the `fleety status` health probe.
//!
//! Mirrors the resolution order used by the consuming tool (`insyra_exec`
//! lives in `fleety-tools`): respect the env override, then look beside the
//! current executable, then `PATH`. We don't try to *run* the sidecar — just
//! confirm a file exists at one of those paths — because the status query
//! should be cheap.

use std::path::PathBuf;

const INSYRA_ENV: &str = "FLEETY_INSYRA_BIN";

fn insyra_filename() -> &'static str {
    if cfg!(windows) {
        "fleety-insyra.exe"
    } else {
        "fleety-insyra"
    }
}

/// Return the path to a usable `fleety-insyra` binary, or `None` if we can't
/// find one. An explicit environment override is authoritative; automatic
/// discovery soft-fails from beside the executable to `PATH`.
pub fn resolve_insyra() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(INSYRA_ENV) {
        let candidate = PathBuf::from(path);
        return candidate.is_file().then_some(candidate);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(insyra_filename());
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    which_on_path(insyra_filename())
}

/// Walk `PATH` looking for `name`. Pure to keep tests deterministic.
fn which_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_matches_platform() {
        if cfg!(windows) {
            assert!(insyra_filename().ends_with(".exe"));
        } else {
            assert_eq!(insyra_filename(), "fleety-insyra");
        }
    }

    #[test]
    #[serial_test::serial]
    fn env_override_is_used_when_file_exists() {
        // Point the env at a known-good file (our own Cargo.toml will do); the
        // resolver must surface exactly that path. Use a unique temp name to
        // avoid interfering with other parallel tests' env state.
        let cargo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        std::env::set_var(INSYRA_ENV, &cargo);
        let resolved = resolve_insyra();
        std::env::remove_var(INSYRA_ENV);
        assert_eq!(resolved.as_deref(), Some(cargo.as_path()));
    }

    #[test]
    #[serial_test::serial]
    fn missing_env_override_is_authoritative_and_reports_unavailable() {
        std::env::set_var(INSYRA_ENV, "/definitely/not/here/fleety-insyra-xxx");
        let resolved = resolve_insyra();
        std::env::remove_var(INSYRA_ENV);
        assert_eq!(resolved, None);
    }
}
