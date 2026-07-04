//! Conversation-scoped skill source directories: the project's per-layer
//! `.claude/skills` and `.agents/skills` (cwd upward) plus the originating
//! device's user-global skill dirs. Pure path collection; the skills runtime
//! overlays these onto the per-connection registry.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Collect the ordered, de-duplicated conversation-scoped skill source
/// directories from `cwd` and `user_home`: each layer from `cwd` upward
/// contributes `.claude/skills` and `.agents/skills` (deep → shallow, so a
/// deeper / more specific skill wins), then the user-global `~/.claude/skills`
/// and `~/.agents/skills`. No path appears twice.
pub fn skill_sources(cwd: &Path, user_home: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for dir in cwd.ancestors() {
        for (a, b) in [(".agents", "skills"), (".claude", "skills")] {
            let p = dir.join(a).join(b);
            if seen.insert(p.clone()) {
                out.push(p);
            }
        }
    }
    for (a, b) in [(".agents", "skills"), (".claude", "skills")] {
        let p = user_home.join(a).join(b);
        if seen.insert(p.clone()) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_sources_layers_and_dedupes() {
        let got = skill_sources(Path::new("/a/b"), Path::new("/home/u"));
        // cwd upward: /a/b, /a, / — each contributes .claude/skills + .agents/skills
        // (deep → shallow), then user-global.
        assert_eq!(got[0], PathBuf::from("/a/b/.agents/skills"));
        assert_eq!(got[1], PathBuf::from("/a/b/.claude/skills"));
        assert_eq!(got[2], PathBuf::from("/a/.agents/skills"));
        assert!(got.contains(&PathBuf::from("/home/u/.claude/skills")));
        assert!(got.contains(&PathBuf::from("/home/u/.agents/skills")));
        let mut uniq = got.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), got.len(), "no duplicate sources");
    }
}
