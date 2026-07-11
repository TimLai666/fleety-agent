//! Reuse an originating device's enabled Claude Code plugins' declarative
//! resources (skills + MCP servers). Pure parsing + best-effort discovery; conn
//! wires the results into the conversation's skill sources and MCP server list.
//! Everything here is best-effort: missing / malformed input yields empty or
//! partial results, never an error.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Where an enabled plugin was enabled — sets the scope its resources join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Project,
    User,
}

/// One MCP server declared by a plugin, in the runtime's server shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
}

/// Declarative resources collected from enabled plugins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginSources {
    pub skill_dirs: Vec<(Scope, PathBuf)>,
    pub mcp_servers: Vec<(Scope, McpServer)>,
}

/// Parse enabled plugin names from a settings JSON. Tolerates an object form
/// (`{"name": true}`, taking the entries whose value is true) and an array form
/// (`["name"]`). Missing / any other shape → empty.
pub fn parse_enabled_plugins(settings: &Value) -> Vec<String> {
    let Some(ep) = settings.get("enabledPlugins") else {
        return Vec::new();
    };
    if let Some(obj) = ep.as_object() {
        return obj
            .iter()
            .filter(|(_, v)| v.as_bool() == Some(true))
            .map(|(k, _)| k.clone())
            .collect();
    }
    if let Some(arr) = ep.as_array() {
        return arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
    }
    Vec::new()
}

/// Convert a Claude Code MCP object (`{ name: { command, args, env } }`) to the
/// runtime's server shape. Entries without a non-empty `command` are skipped.
/// `env` is not carried in the first release.
pub fn to_fleety_mcp(claude: &Value) -> Vec<McpServer> {
    let Some(obj) = claude.as_object() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, spec) in obj {
        let command = match spec.get("command").and_then(Value::as_str) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => continue,
        };
        let args = spec
            .get("args")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        out.push(McpServer {
            name: name.clone(),
            command,
            args,
        });
    }
    out
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
}

/// Locate an enabled plugin's directory under `plugins_root`. The name may be
/// bare or `name@marketplace`; try `plugins_root/<bare>` then one level down
/// (`plugins_root/<marketplace>/<bare>`). Best-effort.
fn locate_plugin(plugins_root: &Path, name: &str) -> Option<PathBuf> {
    let bare = name.split('@').next().unwrap_or(name);
    let direct = plugins_root.join(bare);
    if direct.is_dir() {
        return Some(direct);
    }
    if let Ok(entries) = std::fs::read_dir(plugins_root) {
        for e in entries.flatten() {
            let cand = e.path().join(bare);
            if cand.is_dir() {
                return Some(cand);
            }
        }
    }
    None
}

/// A plugin's MCP servers, from its `.mcp.json` or `plugin.json` `mcpServers`.
fn plugin_mcp_servers(plugin_dir: &Path) -> Vec<McpServer> {
    for file in [".mcp.json", "plugin.json"] {
        if let Some(v) = read_json(&plugin_dir.join(file)) {
            if let Some(mcp) = v.get("mcpServers") {
                return to_fleety_mcp(mcp);
            }
        }
    }
    Vec::new()
}

/// Read the project and user `.claude/settings.json`, locate each enabled plugin
/// under `user_home/.claude/plugins`, and collect its `skills/` dir + MCP
/// servers, tagged by scope. Best-effort throughout: any missing / malformed
/// step is skipped.
pub fn collect_plugin_sources(project_cwd: &Path, user_home: &Path) -> PluginSources {
    let plugins_root = user_home.join(".claude").join("plugins");
    let mut out = PluginSources::default();
    for (scope, settings_path) in [
        (
            Scope::Project,
            project_cwd.join(".claude").join("settings.json"),
        ),
        (Scope::User, user_home.join(".claude").join("settings.json")),
    ] {
        let Some(settings) = read_json(&settings_path) else {
            continue;
        };
        for name in parse_enabled_plugins(&settings) {
            let Some(dir) = locate_plugin(&plugins_root, &name) else {
                continue;
            };
            let skills = dir.join("skills");
            if skills.is_dir() {
                out.skill_dirs.push((scope, skills));
            }
            for server in plugin_mcp_servers(&dir) {
                out.mcp_servers.push((scope, server));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_enabled_plugins_handles_object_and_array() {
        let obj = json!({ "enabledPlugins": { "a": true, "b": false, "c": true } });
        let mut got = parse_enabled_plugins(&obj);
        got.sort();
        assert_eq!(got, vec!["a".to_string(), "c".to_string()]);
        let arr = json!({ "enabledPlugins": ["x", "y"] });
        assert_eq!(
            parse_enabled_plugins(&arr),
            vec!["x".to_string(), "y".to_string()]
        );
        assert!(parse_enabled_plugins(&json!({})).is_empty());
    }

    #[test]
    fn to_fleety_mcp_converts_shape() {
        let claude = json!({
            "srv": { "command": "node", "args": ["s.js"], "env": { "X": "1" } },
            "nocmd": { "args": ["z"] }
        });
        let got = to_fleety_mcp(&claude);
        assert_eq!(got.len(), 1, "an entry without a command is skipped");
        assert_eq!(got[0].name, "srv");
        assert_eq!(got[0].command, "node");
        assert_eq!(got[0].args, vec!["s.js".to_string()]);
    }

    #[test]
    fn collect_plugin_sources_is_best_effort() {
        let got = collect_plugin_sources(Path::new("/no/such/proj"), Path::new("/no/such/home"));
        assert!(got.skill_dirs.is_empty() && got.mcp_servers.is_empty());
    }

    #[test]
    fn collect_plugin_sources_tags_scope() {
        let home = std::env::temp_dir().join(format!("fleety-plug-{}", uuid::Uuid::new_v4()));
        let plug = home.join(".claude").join("plugins").join("up");
        std::fs::create_dir_all(plug.join("skills")).expect("mk plugin skills");
        std::fs::write(
            home.join(".claude").join("settings.json"),
            r#"{"enabledPlugins":{"up":true}}"#,
        )
        .expect("w settings");
        std::fs::write(
            plug.join(".mcp.json"),
            r#"{"mcpServers":{"s":{"command":"node","args":["a"]}}}"#,
        )
        .expect("w mcp");
        let proj = home.join("proj");
        std::fs::create_dir_all(&proj).expect("mk proj");
        let got = collect_plugin_sources(&proj, &home);
        assert_eq!(got.skill_dirs.len(), 1);
        assert_eq!(
            got.skill_dirs[0].0,
            Scope::User,
            "user-enabled → user scope"
        );
        assert!(got.skill_dirs[0].1.ends_with("skills"));
        assert_eq!(got.mcp_servers.len(), 1);
        assert_eq!(got.mcp_servers[0].0, Scope::User);
        assert_eq!(got.mcp_servers[0].1.name, "s");
        let _ = std::fs::remove_dir_all(&home);
    }
}
