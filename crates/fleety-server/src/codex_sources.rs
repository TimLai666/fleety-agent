//! Reuse an originating device's Codex (`~/.codex`) declarative resources — the
//! MCP servers declared in `config.toml`. Pure parsing + best-effort I/O; conn
//! wires the servers into the conversation's per-conversation MCP list, and the
//! Codex `AGENTS.md` is picked up by the instruction-file user-global layer.

use std::path::Path;

use crate::plugin_sources::McpServer;

/// Parse Codex MCP servers from a parsed `config.toml`. Reads the `mcp_servers`
/// table; each entry's key is the server name, its value provides `command` /
/// `args`. Entries without a non-empty command are skipped.
pub fn parse_codex_mcp(config: &toml::Value) -> Vec<McpServer> {
    let Some(table) = config.get("mcp_servers").and_then(|v| v.as_table()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, spec) in table {
        let command = match spec.get("command").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => continue,
        };
        let args = spec
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        out.push(McpServer {
            name: name.clone(),
            command,
            args,
        });
    }
    out
}

/// Read `~/.codex/config.toml` and collect its MCP servers, best-effort: a
/// missing file or malformed TOML yields an empty list.
pub fn collect_codex_mcp(user_home: &Path) -> Vec<McpServer> {
    let path = user_home.join(".codex").join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(config) = toml::from_str::<toml::Value>(&text) else {
        return Vec::new();
    };
    parse_codex_mcp(&config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_codex_mcp_from_toml() {
        let cfg: toml::Value = toml::from_str(
            "[mcp_servers.srv]\ncommand = \"node\"\nargs = [\"s.js\"]\n\n[mcp_servers.nocmd]\nargs = [\"z\"]\n",
        )
        .expect("parse toml");
        let got = parse_codex_mcp(&cfg);
        assert_eq!(got.len(), 1, "an entry without a command is skipped");
        assert_eq!(got[0].name, "srv");
        assert_eq!(got[0].command, "node");
        assert_eq!(got[0].args, vec!["s.js".to_string()]);
    }

    #[test]
    fn collect_codex_mcp_is_best_effort() {
        assert!(collect_codex_mcp(Path::new("/no/such/home")).is_empty());
    }
}
