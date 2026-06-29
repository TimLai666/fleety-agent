//! Built-in skills shipped inside the `fleety-server` binary and seeded into the
//! builtin skills dir at startup, so `list_skills`/`use_skill` can serve them
//! with no extra files to deploy and no runtime download. User-installed skills
//! still override built-ins of the same name (see `skills::collect`).
//!
//! The Insyra skill is vendored from upstream (`SKILL.upstream.md`); release CI
//! refreshes it to match the shipped Insyra version. A Fleety-specific header is
//! prepended at seed time so the agent runs the DSL via `insyra_exec` rather than
//! the upstream `insyra` shell CLI (which isn't present here).

use std::path::Path;

use agent_core::{CoreError, Result};

/// A Fleety adapter note prepended to the upstream Insyra skill. It opens with
/// the Agent Skills YAML frontmatter (`name` + `description`), so `list_skills`
/// uses that `description`, followed by the Fleety usage note and the upstream
/// DSL reference.
const INSYRA_HEADER: &str = "---\n\
name: fleety-use-insyra-dsl\n\
description: Use the Insyra DSL (via the insyra_exec tool) for ALL statistics and data analysis — data cleaning, DataList/DataTable transforms, CSV/Excel/Parquet I/O, column formulas, statistical analysis, and charts. This is the default for any data-analysis or statistics task, regardless of language or stack.\n\
---\n\n\
# fleety-use-insyra-dsl\n\n\
> **In Fleety, run the Insyra DSL through the `insyra_exec` tool — there is no `insyra` shell command here.** \
Pass one DSL line as `command`, a multi-line `.isr` program as `script`, and a `session` name to keep variables/data across calls; \
`save <var> <file>` writes results into the workspace (read them back with `read_file`). \
The upstream reference below describes a CLI/REPL — ignore the install/REPL parts; the **`.isr` DSL command reference applies verbatim**.\n\n\
---\n\n";

/// `(skill_name, header, body)`. The `header` (if any) is prepended to the body
/// at seed time and its first line becomes the `list_skills` description; with an
/// empty header the body's own first line is used.
const SKILLS: &[(&str, &str, &str)] = &[
    (
        "fleety-use-insyra-dsl",
        INSYRA_HEADER,
        include_str!("../builtin-skills/fleety-use-insyra-dsl/SKILL.upstream.md"),
    ),
    (
        // Fleety-native adaptation of the upstream skill-creator: build/improve
        // authored skills via the `skill_*` tools (no eval-viewer / packaging).
        "fleety-skill-creator",
        "",
        include_str!("../builtin-skills/fleety-skill-creator/SKILL.md"),
    ),
    (
        // Upstream Insyra skill, vendored verbatim (no Fleety header / rename) —
        // the Go-oriented companion to fleety-use-insyra-dsl. Release CI refreshes
        // it from the same Insyra release version.
        "insyra",
        "",
        include_str!("../builtin-skills/insyra/SKILL.md"),
    ),
];

/// Write the embedded built-in skills into `builtin_dir` (overwriting built-ins
/// so an updated binary ships an updated skill).
pub fn seed(builtin_dir: &Path) -> Result<()> {
    for (name, header, body) in SKILLS {
        let dir = builtin_dir.join(name);
        std::fs::create_dir_all(&dir).map_err(|e| {
            CoreError::Message(format!("cannot create builtin skill dir '{name}': {e}"))
        })?;
        let mut content = String::with_capacity(header.len() + body.len());
        content.push_str(header);
        content.push_str(body);
        std::fs::write(dir.join("SKILL.md"), content)
            .map_err(|e| CoreError::Message(format!("cannot seed builtin skill '{name}': {e}")))?;
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
        let content =
            std::fs::read_to_string(dir.join("fleety-use-insyra-dsl").join("SKILL.md")).expect("read");
        // Opens with Agent Skills frontmatter (name + description)...
        assert!(content.starts_with("---"));
        assert!(content.contains("name: fleety-use-insyra-dsl"));
        assert!(content.contains("insyra_exec"));
        // ...followed by the upstream DSL reference.
        assert!(content.contains(".isr"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn seed_writes_skill_creator() {
        let dir = std::env::temp_dir().join(format!("fleety-bskill-{}", uuid::Uuid::new_v4()));
        seed(&dir).expect("seed");
        let content =
            std::fs::read_to_string(dir.join("fleety-skill-creator").join("SKILL.md")).expect("read");
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
        let content =
            std::fs::read_to_string(dir.join("insyra").join("SKILL.md")).expect("read");
        // Vendored verbatim (no Fleety header): the upstream frontmatter name is kept.
        assert!(content.starts_with("---"));
        assert!(content.contains("name: insyra"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
