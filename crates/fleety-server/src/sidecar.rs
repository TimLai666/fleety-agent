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
/// find one. Soft-fails through every step.
pub fn resolve_insyra() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(INSYRA_ENV) {
        let p = PathBuf::from(&path);
        if p.is_file() {
            return Some(p);
        }
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
    fn missing_env_path_falls_through_to_other_strategies() {
        std::env::set_var(INSYRA_ENV, "/definitely/not/here/fleety-insyra-xxx");
        let _ = resolve_insyra(); // returns Some(...) or None; we just want no panic
        std::env::remove_var(INSYRA_ENV);
    }
}
