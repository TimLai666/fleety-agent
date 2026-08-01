//! Which tools the model is shown, and how it reaches the rest.
//!
//! The full tool surface is ~80 schemas — about 37 KB on every single request,
//! and a conversation typically uses fewer than ten of them. So a conversation
//! opens with a small **resident** set plus one search entry point, and the rest
//! arrive only when the model asks for them.
//!
//! This mirrors what the server already does elsewhere: a device's whole toolset
//! sits behind `device_exec`, an MCP server's behind `mcp_call`, and skills are
//! names-and-descriptions until `use_skill` loads one. This module applies the
//! same idea to the built-in tools themselves.
//!
//! Activation is a **context budget, not an authorization boundary** — see
//! `agent_core::ActiveTools`. Hiding a tool never decides whether it may run.

use serde_json::{json, Value};

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolSpec};

/// A named bundle of tools the model activates as a unit.
///
/// Group, not single tool: one piece of work normally needs several tools from
/// the same area (driving a browser means navigating *and* screenshotting *and*
/// closing), so per-tool activation would cost a round trip each.
pub struct ToolGroup {
    /// Stable identifier, also what the search reports.
    pub name: &'static str,
    /// What this group lets the agent do, in the words a model would search with.
    pub summary: &'static str,
    /// Every tool in the group.
    pub tools: &'static [&'static str],
}

/// The tools every conversation opens with, before any search.
///
/// Chosen so the common cases need no search at all: reading and changing files,
/// running commands, the two skills entry points, core memory, and the two
/// cross-device entry points. Everything else is one search away.
pub const RESIDENT: &[&str] = &[
    // Workspace files and command execution.
    "read_file",
    "edit_file",
    "write_file",
    "search_files",
    "list_dir",
    "make_dir",
    "move_file",
    "delete_file",
    "rollback",
    "run_command",
    // Skills are already progressive: these two are the entry points.
    "list_skills",
    "use_skill",
    // Core memory: the agent's own notes about the user and itself.
    "memory_read",
    "memory_write",
    // Cross-device: listing devices and running a tool on one.
    "device_list",
    "device_exec",
];

/// The name of the search entry point. Always offered, never deactivatable — it
/// is the only route from the resident set to everything else.
pub const SEARCH_TOOL: &str = "tool_search";

/// Every group. Together with [`RESIDENT`] this must cover the whole registry;
/// `groups_cover_every_registered_tool` in `conn.rs` proves it does.
pub const GROUPS: &[ToolGroup] = &[
    ToolGroup {
        name: "files-extra",
        summary: "read and write raw file bytes, and inspect git history",
        tools: &[
            "read_file_bytes",
            "write_file_bytes",
            "git_status",
            "git_diff",
            "git_log",
            "git_show",
        ],
    },
    ToolGroup {
        name: "skills-authoring",
        summary: "create, edit, validate, install and remove skills",
        tools: &[
            "skill_validate",
            "skill_install",
            "skill_remove",
            "skill_list_files",
            "skill_read_file",
            "skill_write_file",
            "skill_edit_file",
            "skill_delete_file",
        ],
    },
    ToolGroup {
        name: "web",
        summary: "fetch URLs and call HTTP, SSE and WebSocket endpoints",
        tools: &["fetch_url", "http_request", "sse_stream", "ws_call"],
    },
    ToolGroup {
        name: "browser",
        summary: "drive a Chrome browser: navigate, evaluate scripts, screenshot",
        tools: &[
            "browser_open",
            "browser_navigate",
            "browser_eval",
            "browser_screenshot",
            "browser_close",
        ],
    },
    ToolGroup {
        name: "computer",
        summary: "control the screen, keyboard and mouse of this machine",
        tools: &[
            "computer_screenshot",
            "computer_click",
            "computer_type",
            "computer_key",
            "computer_move",
            "computer_scroll",
        ],
    },
    ToolGroup {
        name: "terminal",
        summary: "open and drive an interactive terminal session (local or ssh)",
        tools: &[
            "terminal_open",
            "terminal_input",
            "terminal_read",
            "terminal_close",
        ],
    },
    ToolGroup {
        name: "wiki",
        summary: "read, write and search the knowledge wiki",
        tools: &[
            "wiki_read",
            "wiki_write",
            "wiki_list",
            "wiki_search",
            "wiki_semantic_search",
        ],
    },
    ToolGroup {
        name: "memory-extra",
        summary: "edit core memory surgically and review past tool history",
        tools: &["memory_edit", "history_list"],
    },
    ToolGroup {
        name: "fleet",
        summary: "inspect devices, pair them, transfer files, and run ssh commands",
        tools: &[
            "device_show",
            "device_presence",
            "device_set_site",
            "device_set_home_site",
            "device_set_mobility",
            "device_set_presence_opt_in",
            "presence_show",
            "pair_create",
            "transfer_file",
            "ssh_exec",
        ],
    },
    ToolGroup {
        name: "sites",
        summary: "define and bind named physical sites for the fleet",
        tools: &[
            "site_list",
            "site_show",
            "site_set",
            "site_delete",
            "site_bind_fingerprint",
        ],
    },
    ToolGroup {
        name: "schedule",
        summary: "create, list and delete the agent's own scheduled runs",
        tools: &["schedule_create", "schedule_list", "schedule_delete"],
    },
    ToolGroup {
        name: "mcp",
        summary: "add, list, remove and call external MCP servers",
        tools: &["mcp_add", "mcp_list", "mcp_remove", "mcp_call"],
    },
    ToolGroup {
        name: "data-analysis",
        summary: "run Insyra data-analysis scripts and extract video content",
        tools: &["insyra_exec", "video_extract"],
    },
];

/// The tools a conversation opens with: the resident set plus the search entry.
pub fn opening_set() -> Vec<String> {
    RESIDENT
        .iter()
        .map(|s| s.to_string())
        .chain(std::iter::once(SEARCH_TOOL.to_string()))
        .collect()
}

/// Put a conversation's offered set in effect: always the resident set and the
/// search entry, plus whatever that conversation has activated so far.
///
/// The resident set and the search entry are unioned in rather than trusted from
/// storage, so state written by another build — or state that somehow omits the
/// search entry — can never leave the model unable to discover anything. Names
/// in `stored` that this build does not register are harmless: `specs()` simply
/// never matches them.
pub fn apply_activation(
    active: &agent_core::ActiveTools,
    stored: Option<&std::collections::BTreeSet<String>>,
) {
    let mut set: std::collections::BTreeSet<String> = opening_set().into_iter().collect();
    if let Some(stored) = stored {
        set.extend(stored.iter().cloned());
    }
    active.restrict_to(set);
}

/// Score a group against a free-text capability query. Higher is better; `0`
/// means no match at all.
///
/// Deliberately a plain word-overlap score, not an embedding: the model already
/// describes what it wants in words the summaries use, and a dependency-free
/// match keeps this on the hot path of every conversation.
fn score(group: &ToolGroup, query: &str) -> usize {
    let query = query.to_lowercase();
    let words: Vec<&str> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .collect();
    if words.is_empty() {
        return 0;
    }
    let haystack = format!("{} {} {}", group.name, group.summary, group.tools.join(" "))
        .to_lowercase()
        .replace(['_', '-'], " ");
    words.iter().filter(|w| haystack.contains(**w)).count()
}

/// The groups matching `query`, best first. Empty when nothing matches.
pub fn search(query: &str) -> Vec<&'static ToolGroup> {
    let mut hits: Vec<(usize, &'static ToolGroup)> = GROUPS
        .iter()
        .map(|g| (score(g, query), g))
        .filter(|(s, _)| *s > 0)
        .collect();
    hits.sort_by_key(|(s, g)| (std::cmp::Reverse(*s), g.name));
    hits.into_iter().map(|(_, g)| g).collect()
}

/// The `tool_search` tool: the model's only route from the resident set to
/// everything else. Activating is a side effect of finding — a search that hits
/// makes those tools visible to the next model call of the same turn.
pub struct ToolSearch {
    active: agent_core::ActiveTools,
}

impl ToolSearch {
    pub fn new(active: agent_core::ActiveTools) -> Self {
        Self { active }
    }
}

#[async_trait::async_trait]
impl Tool for ToolSearch {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: SEARCH_TOOL.to_string(),
            description: "Find and activate tools for a capability you do not \
                          currently have. The tools you can see are not all the \
                          tools that exist — search here before concluding you \
                          cannot do something. Activated tools stay available."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "capability you need, in plain words" }
                },
                "required": ["query"]
            }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .ok_or_else(|| CoreError::Message("tool_search needs 'query'".to_string()))?;
        let hits = search(query);
        if hits.is_empty() {
            return Ok(json!({
                "query": query,
                "activated": [],
                "detail": "No tool group matches that capability. Nothing was activated.",
            }));
        }
        // Activating an already-active group is a harmless set union.
        for group in &hits {
            self.active
                .activate(group.tools.iter().map(|t| t.to_string()));
        }
        Ok(json!({
            "query": query,
            "activated": hits.iter().map(|g| json!({
                "group": g.name,
                "summary": g.summary,
                "tools": g.tools,
            })).collect::<Vec<_>>(),
            "detail": "These tools are now available to you.",
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn every_tool_belongs_to_exactly_one_group() {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for group in GROUPS {
            for tool in group.tools {
                assert!(
                    seen.insert(tool),
                    "tool '{tool}' appears in more than one group"
                );
                assert!(
                    !RESIDENT.contains(tool),
                    "tool '{tool}' is both resident and in group '{}'",
                    group.name
                );
            }
        }
    }

    #[test]
    fn opening_set_is_resident_plus_search() {
        let opening = opening_set();
        assert_eq!(opening.len(), RESIDENT.len() + 1);
        assert!(opening.contains(&SEARCH_TOOL.to_string()));
    }

    /// The spec's worked example: an empty activated set, a search for browser
    /// control, and the browser group's five tools become available.
    #[tokio::test]
    async fn search_activates_the_matching_group() {
        let active = agent_core::ActiveTools::default();
        active.restrict_to([SEARCH_TOOL.to_string()]);
        let search = ToolSearch::new(active.clone());

        let out = search
            .call(json!({ "query": "control a web browser" }))
            .await
            .expect("search runs");
        let groups: Vec<&str> = out["activated"]
            .as_array()
            .expect("activated array")
            .iter()
            .filter_map(|g| g["group"].as_str())
            .collect();
        assert!(
            groups.contains(&"browser"),
            "browser group named: {groups:?}"
        );

        for tool in [
            "browser_open",
            "browser_navigate",
            "browser_eval",
            "browser_screenshot",
            "browser_close",
        ] {
            assert!(active.offers(tool), "{tool} is now offered");
        }
    }

    #[tokio::test]
    async fn search_that_matches_nothing_changes_nothing() {
        let active = agent_core::ActiveTools::default();
        active.restrict_to([SEARCH_TOOL.to_string()]);
        let before = active.snapshot();

        let out = ToolSearch::new(active.clone())
            .call(json!({ "query": "zzzz nonexistent capability qqqq" }))
            .await
            .expect("search runs");

        assert_eq!(out["activated"], json!([]));
        assert!(out["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("No tool group matches"));
        assert_eq!(active.snapshot(), before, "the activated set is unchanged");
    }

    #[tokio::test]
    async fn re_activating_is_harmless() {
        let active = agent_core::ActiveTools::default();
        active.restrict_to([SEARCH_TOOL.to_string()]);
        let search = ToolSearch::new(active.clone());

        search
            .call(json!({ "query": "control a web browser" }))
            .await
            .expect("first search");
        let after_first = active.snapshot();
        search
            .call(json!({ "query": "control a web browser" }))
            .await
            .expect("second search returns normally");
        assert_eq!(active.snapshot(), after_first, "no duplication, no error");
    }

    /// Whatever a conversation carries, the resident set and the search entry are
    /// always offered — otherwise a conversation could load state that leaves the
    /// model with no way to discover anything.
    #[test]
    fn applying_stored_activation_always_keeps_resident_and_search() {
        let active = agent_core::ActiveTools::default();
        // State that mentions neither the search entry nor any resident tool.
        let stored: BTreeSet<String> = ["browser_open".to_string()].into_iter().collect();
        apply_activation(&active, Some(&stored));

        assert!(active.offers(SEARCH_TOOL), "search is never hidden");
        for tool in RESIDENT {
            assert!(active.offers(tool), "{tool} stays resident");
        }
        assert!(
            active.offers("browser_open"),
            "stored activation is honoured"
        );
        assert!(!active.offers("ssh_exec"), "unrelated tools stay hidden");
    }

    /// A conversation with no stored state opens on exactly the opening set.
    #[test]
    fn applying_no_stored_activation_yields_the_opening_set() {
        let active = agent_core::ActiveTools::default();
        apply_activation(&active, None);
        let offered = active.snapshot().expect("restricted");
        let expected: BTreeSet<String> = opening_set().into_iter().collect();
        assert_eq!(offered, expected);
    }

    #[test]
    fn search_ranks_the_most_relevant_group_first() {
        assert_eq!(
            search("navigate a web browser").first().map(|g| g.name),
            Some("browser")
        );
        assert_eq!(
            search("run an interactive terminal session")
                .first()
                .map(|g| g.name),
            Some("terminal")
        );
        assert!(search("").is_empty(), "an empty query matches nothing");
    }
}
