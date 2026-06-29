//! On-device tools fleetyd runs locally when the server dispatches a `RunTool`.
//!
//! These are the **shared** workspace tools from `fleety-tools` (read/list/
//! search-ripgrep/write/edit/run/git, with backups + unified diffs and the
//! critical-command guard) — the same implementations the server uses, rooted
//! at `FLEETY_DEVICE_ROOT` (default: the daemon's working dir). Keeping one
//! implementation means the agent gets the full toolset on every device, with
//! no drift; `fleetyd update` keeps the binary current.

use std::path::{Path, PathBuf};

use agent_core::ToolRegistry;

/// The device filesystem root the on-device tools operate within.
pub fn device_root() -> PathBuf {
    std::env::var("FLEETY_DEVICE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Build the on-device tool registry rooted at `root`.
pub fn build_local_registry(root: &Path) -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    let backups = root.join(".fleety-backups");
    fleety_tools::register_workspace(&mut registry, root, &backups);
    fleety_tools::register_insyra(&mut registry, root);
    // Browser (CDP) tools so the agent can drive this device's browser via
    // device_exec; Chrome is auto-provisioned on first use.
    fleety_tools::register_browser(&mut registry);
    // Native computer-use (desktop control) so device_exec can drive this
    // device's screen/mouse/keyboard. Errors actionably on headless hosts.
    fleety_tools::register_computer(&mut registry);
    // Interactive PTY terminal sessions on this device (driven via device_exec);
    // sessions live in this daemon process's registry across calls.
    fleety_tools::register_terminal(&mut registry);
    registry
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleety-ondev-{}", uuid_like()));
        std::fs::create_dir_all(&dir).expect("mk temp");
        dir
    }

    // Avoid a uuid dep in the daemon test: a process+counter-based unique name.
    fn uuid_like() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        format!(
            "{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[tokio::test]
    async fn shared_tools_present_on_device() {
        let root = temp();
        let reg = build_local_registry(&root);
        // The rich toolset is available on the device, not just the basics.
        reg.call(
            "write_file",
            json!({ "path": "note.txt", "content": "hi device" }),
        )
        .await
        .expect("write");
        let read = reg
            .call("read_file", json!({ "path": "note.txt" }))
            .await
            .expect("read");
        assert_eq!(read["content"], json!("hi device"));
        // search (ripgrep) now works on-device too
        let found = reg
            .call("search_files", json!({ "query": "device" }))
            .await
            .expect("search");
        assert_eq!(found["matches"].as_array().map(Vec::len).unwrap_or(0), 1);
        // escape + critical guard still enforced
        assert!(reg
            .call("read_file", json!({ "path": "../escape" }))
            .await
            .is_err());
        assert!(reg
            .call("run_command", json!({ "command": "rm -rf /" }))
            .await
            .is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
