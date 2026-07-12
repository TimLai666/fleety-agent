//! Built-in skills shipped inside the `fleety-server` binary and seeded into the
//! builtin skills dir at startup, so `list_skills`/`use_skill` can serve them
//! with no extra files to deploy and no runtime download. User-installed skills
//! still override built-ins of the same name (see `skills::collect`).
//!
//! Each skill is embedded as a **whole directory** (via `include_dir`) — `SKILL.md`
//! plus whatever else it ships (`references/`, scripts, …) — so the file set isn't
//! hardcoded and new upstream files flow through automatically; only `SKILL.md` is
//! guaranteed. The Insyra skills are vendored from upstream and refreshed to the
//! shipped Insyra version by release CI. A skill dir may include a `HEADER.md`
//! (Fleety-authored, not from upstream): at seed time it is prepended to that
//! skill's `SKILL.md` and not emitted itself — that's how `fleety-use-insyra-dsl`
//! tells the agent to run the DSL via `insyra_exec` (no upstream `insyra` CLI here)
//! while keeping `SKILL.md` a pristine, CI-refreshable upstream copy.

use std::path::Path;

use agent_core::{CoreError, Result};
use include_dir::{include_dir, Dir, DirEntry};

/// `(skill_name, embedded_dir)`. The dir is embedded whole; if it contains a
/// `HEADER.md`, that is folded into `SKILL.md` at seed time (see [`seed`]).
const SKILLS: &[(&str, &Dir<'static>)] = &[
    (
        "fleety-use-insyra-dsl",
        &include_dir!("$CARGO_MANIFEST_DIR/builtin-skills/fleety-use-insyra-dsl"),
    ),
    (
        "fleety-skill-creator",
        &include_dir!("$CARGO_MANIFEST_DIR/builtin-skills/fleety-skill-creator"),
    ),
    (
        "insyra",
        &include_dir!("$CARGO_MANIFEST_DIR/builtin-skills/insyra"),
    ),
    (
        "fleety-real-video",
        &include_dir!("$CARGO_MANIFEST_DIR/builtin-skills/fleety-real-video"),
    ),
];

/// Recursively write every embedded file (root-relative paths preserved) under
/// `base`, except `HEADER.md` (handled by the caller).
fn write_entries(base: &Path, dir: &Dir, skill: &str) -> Result<()> {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(sub) => write_entries(base, sub, skill)?,
            DirEntry::File(file) => {
                if file.path().file_name().and_then(|s| s.to_str()) == Some("HEADER.md") {
                    continue;
                }
                let path = base.join(file.path());
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| {
                        CoreError::Message(format!(
                            "cannot create builtin skill dir '{skill}': {e}"
                        ))
                    })?;
                }
                std::fs::write(&path, file.contents()).map_err(|e| {
                    CoreError::Message(format!("cannot seed builtin skill '{skill}': {e}"))
                })?;
            }
        }
    }
    Ok(())
}

/// Write the embedded built-in skills (whole packages) into `builtin_dir`. The
/// builtin tier is owned entirely by the binary, so it is **wiped first** for a
/// clean whole-package replace: on upgrade, files (or whole skills) the new
/// binary no longer ships don't linger as stale leftovers. The `installed` /
/// `authored` tiers live in separate dirs and are untouched. A skill's
/// `HEADER.md` (if present) is prepended to its `SKILL.md` and not emitted itself.
pub fn seed(builtin_dir: &Path) -> Result<()> {
    // Clean replace: the builtin tier is regenerated from the binary every boot.
    let _ = std::fs::remove_dir_all(builtin_dir);
    for (name, dir) in SKILLS {
        let base = builtin_dir.join(name);
        write_entries(&base, dir, name)?;
        // Fold a Fleety HEADER.md into SKILL.md (so SKILL.md stays a pristine,
        // CI-refreshable upstream copy and the Fleety adaptation survives refresh).
        if let Some(header) = dir.get_file("HEADER.md") {
            let skill_md = base.join("SKILL.md");
            let body = std::fs::read_to_string(&skill_md).unwrap_or_default();
            let header = String::from_utf8_lossy(header.contents());
            std::fs::write(&skill_md, format!("{header}{body}")).map_err(|e| {
                CoreError::Message(format!("cannot seed builtin skill '{name}': {e}"))
            })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_writes_insyra_skill_with_fleety_header() {
        let dir = std::env::temp_dir().join(format!("fleety-bskill-{}", uuid::Uuid::new_v4()));
        seed(&dir).expect("seed");
        let content = std::fs::read_to_string(dir.join("fleety-use-insyra-dsl").join("SKILL.md"))
            .expect("read");
        // Opens with Agent Skills frontmatter (name + description)...
        assert!(content.starts_with("---"));
        assert!(content.contains("name: fleety-use-insyra-dsl"));
        assert!(content.contains("insyra_exec"));
        // ...followed by the upstream DSL reference.
        assert!(content.contains(".isr"));
        // The whole package is seeded — references/, not just SKILL.md — and the
        // Fleety HEADER.md is folded in, not emitted as its own file.
        let skill_dir = dir.join("fleety-use-insyra-dsl");
        assert!(skill_dir
            .join("references")
            .join("cli-commands.md")
            .is_file());
        assert!(!skill_dir.join("HEADER.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seed_writes_skill_creator() {
        let dir = std::env::temp_dir().join(format!("fleety-bskill-{}", uuid::Uuid::new_v4()));
        seed(&dir).expect("seed");
        let content = std::fs::read_to_string(dir.join("fleety-skill-creator").join("SKILL.md"))
            .expect("read");
        // Opens with Agent Skills frontmatter; body teaches the Fleety skill_*
        // workflow, not the upstream eval-viewer machinery.
        assert!(content.starts_with("---"));
        assert!(content.contains("name: fleety-skill-creator"));
        assert!(content.contains("skill_write_file"));
        assert!(content.contains("authored"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seed_writes_verbatim_insyra_skill() {
        let dir = std::env::temp_dir().join(format!("fleety-bskill-{}", uuid::Uuid::new_v4()));
        seed(&dir).expect("seed");
        let content = std::fs::read_to_string(dir.join("insyra").join("SKILL.md")).expect("read");
        // Vendored verbatim (no Fleety header): the upstream frontmatter name is kept.
        assert!(content.starts_with("---"));
        assert!(content.contains("name: insyra"));
        // Whole package, incl. its references/.
        assert!(dir
            .join("insyra")
            .join("references")
            .join("ccl-operators.md")
            .is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seed_is_a_clean_replace() {
        let dir = std::env::temp_dir().join(format!("fleety-bskill-{}", uuid::Uuid::new_v4()));
        seed(&dir).expect("seed 1");
        // Simulate leftovers an older binary seeded: a now-removed reference file
        // and a now-removed whole skill.
        let stale_ref = dir.join("insyra").join("references").join("stale.md");
        std::fs::write(&stale_ref, "old").expect("plant ref");
        let removed_skill = dir.join("removed-skill");
        std::fs::create_dir_all(&removed_skill).expect("mk");
        std::fs::write(removed_skill.join("SKILL.md"), "old").expect("plant skill");

        seed(&dir).expect("seed 2");

        // Clean replace: stale leftovers are gone, current skills intact.
        assert!(
            !stale_ref.exists(),
            "stale reference file should be removed"
        );
        assert!(!removed_skill.exists(), "removed skill should be gone");
        assert!(dir.join("insyra").join("SKILL.md").is_file());
        assert!(dir
            .join("fleety-use-insyra-dsl")
            .join("references")
            .join("cli-commands.md")
            .is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
