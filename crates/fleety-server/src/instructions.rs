//! Per-conversation instruction-file collection: `AGENTS.md` / `CLAUDE.md` from
//! the project layers (project root down to the origin cwd) plus the
//! originating device's user-global files. This module is pure path/collection
//! logic and byte-budgeting; reading (local or via `device_exec`) and injection
//! into the turn live in `conn`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The instruction file names looked for at each project layer, in fixed order
/// (AGENTS.md before CLAUDE.md at the same layer).
const LAYER_FILES: [&str; 2] = ["CLAUDE.md", "AGENTS.md"];

/// Per-file byte cap for an injected instruction file. Overridable with
/// `FLEETY_INSTRUCTION_FILE_MAX_BYTES`.
pub fn per_file_cap() -> usize {
    env_usize("FLEETY_INSTRUCTION_FILE_MAX_BYTES", 8_000)
}

/// Total byte cap across all instruction files injected in one collection.
/// Overridable with `FLEETY_INSTRUCTION_TOTAL_MAX_BYTES`.
pub fn total_cap() -> usize {
    env_usize("FLEETY_INSTRUCTION_TOTAL_MAX_BYTES", 24_000)
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

/// Directory layers from `project_root` down to `cwd` (shallow → deep). If `cwd`
/// is not under `project_root`, the layers run up to the filesystem root.
fn layer_dirs(project_root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for anc in cwd.ancestors() {
        dirs.push(anc.to_path_buf());
        if anc == project_root {
            break;
        }
    }
    dirs.reverse();
    dirs
}

/// Collect the ordered, de-duplicated instruction-file paths for a conversation:
/// `AGENTS.md` + `CLAUDE.md` at each layer from `project_root` down to `cwd`
/// (shallow → deep, so deeper files refine shallower ones), then the originating
/// device's user-global files (`~/.claude/CLAUDE.md`, `~/.agents/AGENTS.md`).
/// The same path never appears twice.
pub fn collect_instruction_paths(
    project_root: &Path,
    cwd: &Path,
    user_home: &Path,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for dir in layer_dirs(project_root, cwd) {
        for name in LAYER_FILES {
            let p = dir.join(name);
            if seen.insert(p.clone()) {
                out.push(p);
            }
        }
    }
    for (sub, name) in [
        (".claude", "CLAUDE.md"),
        (".agents", "AGENTS.md"),
        (".codex", "AGENTS.md"),
    ] {
        let p = user_home.join(sub).join(name);
        if seen.insert(p.clone()) {
            out.push(p);
        }
    }
    out
}

/// One collected instruction file's content, ready to inject.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionFile {
    pub path: PathBuf,
    pub content: String,
    /// True when the content was cut by the per-file or total byte cap.
    pub truncated: bool,
}

/// Truncate `s` to at most `max` bytes on a UTF-8 char boundary.
fn truncate_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Read the collected `paths` with `reader` (returns `None` for a missing or
/// unreadable path — those are skipped), applying a per-file byte cap and a
/// running total cap. Content that hits either cap is truncated on a char
/// boundary and marked. Reading stops once the total cap is reached. The
/// `reader` abstracts the source, so the same logic serves local reads and
/// cross-device (`device_exec`) reads.
pub fn cap_instruction_contents(
    items: Vec<(PathBuf, String)>,
    per_file: usize,
    total: usize,
) -> Vec<InstructionFile> {
    let mut out = Vec::new();
    let mut used = 0usize;
    for (path, mut content) in items {
        if used >= total {
            break;
        }
        let mut truncated = false;
        if content.len() > per_file {
            content = truncate_bytes(&content, per_file);
            truncated = true;
        }
        let remaining = total - used;
        if content.len() > remaining {
            content = truncate_bytes(&content, remaining);
            truncated = true;
        }
        used += content.len();
        if truncated {
            content.push_str("\n…(truncated)");
        }
        out.push(InstructionFile {
            path,
            content,
            truncated,
        });
    }
    out
}

/// Read the collected `paths` with `reader` (`None` = missing/unreadable →
/// skipped), then apply the byte caps via [`cap_instruction_contents`]. The
/// `reader` abstracts the source: local reads pass `fs::read_to_string`;
/// cross-device reads pre-fetch content and call [`cap_instruction_contents`]
/// directly.
pub fn read_instruction_files(
    paths: &[PathBuf],
    per_file: usize,
    total: usize,
    reader: impl Fn(&Path) -> Option<String>,
) -> Vec<InstructionFile> {
    let items: Vec<(PathBuf, String)> = paths
        .iter()
        .filter_map(|p| reader(p).map(|c| (p.clone(), c)))
        .collect();
    cap_instruction_contents(items, per_file, total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_instruction_paths_layers_and_dedupes() {
        let got =
            collect_instruction_paths(Path::new("/a"), Path::new("/a/b/c"), Path::new("/home/u"));
        let expect: Vec<PathBuf> = [
            "/a/CLAUDE.md",
            "/a/AGENTS.md",
            "/a/b/CLAUDE.md",
            "/a/b/AGENTS.md",
            "/a/b/c/CLAUDE.md",
            "/a/b/c/AGENTS.md",
            "/home/u/.claude/CLAUDE.md",
            "/home/u/.agents/AGENTS.md",
            "/home/u/.codex/AGENTS.md",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(got, expect);
    }

    #[test]
    fn collect_single_layer_when_root_equals_cwd() {
        let got = collect_instruction_paths(Path::new("/a"), Path::new("/a"), Path::new("/home/u"));
        // One layer (/a) → 2 files, plus 3 user-global files (.claude/.agents/.codex).
        assert_eq!(got.len(), 5);
        assert_eq!(got[0], PathBuf::from("/a/CLAUDE.md"));
        assert_eq!(got[1], PathBuf::from("/a/AGENTS.md"));
    }

    #[test]
    fn collect_skips_missing_and_caps_size() {
        let paths: Vec<PathBuf> = ["/x/A.md", "/x/B.md", "/x/C.md"]
            .iter()
            .map(PathBuf::from)
            .collect();
        // A is missing, B and C are each 100 bytes.
        let reader = |p: &Path| -> Option<String> {
            let s = p.to_string_lossy();
            if s.ends_with("A.md") {
                None
            } else {
                Some("z".repeat(100))
            }
        };
        // per_file 50, total 80: A skipped, B truncated to 50, C limited by the
        // remaining 30.
        let got = read_instruction_files(&paths, 50, 80, reader);
        assert_eq!(got.len(), 2, "missing file skipped");
        assert!(got[0].path.ends_with("B.md"));
        assert!(got[0].truncated && got[0].content.contains("truncated"));
        assert!(got[1].path.ends_with("C.md"));
        assert!(got[1].truncated, "C is cut by the remaining total budget");
    }
}
