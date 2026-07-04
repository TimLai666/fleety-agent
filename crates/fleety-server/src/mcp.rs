//! MCP runtime: configure external MCP servers and call their tools.
//!
//! `mcp_add`/`mcp_list`/`mcp_remove` persist a JSON list of servers; `mcp_call`
//! spawns a server over stdio and speaks newline-delimited JSON-RPC
//! (initialize → tools/call). The JSON-RPC framing is unit-tested; the live
//! spawn follows the codebase's verification posture (logic tested, live I/O
//! exercised manually, like the OpenAI provider).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use agent_core::{CoreError, Result, RiskLevel, Tool, ToolRegistry, ToolSpec};

const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// Register the MCP tools. `builtin` (seeded by the runtime, read-only from the
/// user's PoV) and `installed` (user-managed) are merged: installed shadows
/// builtin by name. `mcp_add` writes only to `installed`; `mcp_remove` refuses
/// to delete a built-in (the operator overrides one by `mcp_add`-ing a server
/// of the same name).
pub fn register(
    registry: &mut ToolRegistry,
    builtin: &Path,
    installed: &Path,
    conversation: Vec<ServerCfg>,
) {
    registry.register(Box::new(McpList {
        builtin: builtin.to_path_buf(),
        installed: installed.to_path_buf(),
        conversation: conversation.clone(),
    }));
    registry.register(Box::new(McpAdd {
        installed: installed.to_path_buf(),
    }));
    registry.register(Box::new(McpRemove {
        builtin: builtin.to_path_buf(),
        installed: installed.to_path_buf(),
    }));
    registry.register(Box::new(McpCall {
        builtin: builtin.to_path_buf(),
        installed: installed.to_path_buf(),
        conversation,
    }));
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| CoreError::Message(format!("missing required string argument '{key}'")))
}

#[derive(Clone)]
pub(crate) struct ServerCfg {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    /// Where this entry came from. Not persisted — set at load time.
    pub builtin: bool,
}

/// Load one file (builtin or installed). Missing file → empty vec.
fn load_one(config: &Path, builtin: bool) -> Result<Vec<ServerCfg>> {
    let text = match std::fs::read_to_string(config) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CoreError::Message(format!("cannot read mcp config: {e}"))),
    };
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| CoreError::Message(format!("corrupt mcp config: {e}")))?;
    let mut out = Vec::new();
    if let Some(arr) = value.get("servers").and_then(Value::as_array) {
        for s in arr {
            let name = s
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let command = s
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args = s
                .get("args")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            if !name.is_empty() && !command.is_empty() {
                out.push(ServerCfg {
                    name,
                    command,
                    args,
                    builtin,
                });
            }
        }
    }
    Ok(out)
}

/// Merge builtin + installed; installed shadows builtin by name.
pub(crate) fn load_merged(
    builtin: &Path,
    installed: &Path,
    conversation: &[ServerCfg],
) -> Result<Vec<ServerCfg>> {
    let mut out = load_one(builtin, true)?;
    let user = load_one(installed, false)?;
    // Precedence by name: conversation (plugin) > installed > builtin.
    let user_names: std::collections::HashSet<String> =
        user.iter().map(|s| s.name.clone()).collect();
    out.retain(|s| !user_names.contains(&s.name));
    out.extend(user);
    let conv_names: std::collections::HashSet<String> =
        conversation.iter().map(|s| s.name.clone()).collect();
    out.retain(|s| !conv_names.contains(&s.name));
    out.extend(conversation.iter().cloned());
    Ok(out)
}

fn save_installed(config: &Path, servers: &[ServerCfg]) -> Result<()> {
    if let Some(parent) = config.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CoreError::Message(format!("cannot create mcp dir: {e}")))?;
    }
    let arr: Vec<Value> = servers
        .iter()
        .map(|s| json!({ "name": s.name, "command": s.command, "args": s.args }))
        .collect();
    let body = serde_json::to_string_pretty(&json!({ "servers": arr }))
        .map_err(|e| CoreError::Message(format!("serialize mcp config: {e}")))?;
    std::fs::write(config, body)
        .map_err(|e| CoreError::Message(format!("write mcp config: {e}")))?;
    Ok(())
}

// --- JSON-RPC framing (pure, unit-tested) ---

fn jsonrpc_request(id: u64, method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

fn jsonrpc_notification(method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
}

/// Parse a JSON-RPC line: `Some(Ok(result))` / `Some(Err)` if it is the response
/// for `id`, or `None` if it is an unrelated message (notification / other id).
fn parse_response_for(line: &str, id: u64) -> Option<Result<Value>> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v.get("id").and_then(Value::as_u64) != Some(id) {
        return None;
    }
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Some(Err(CoreError::Provider(format!("mcp error: {msg}"))));
    }
    Some(Ok(v.get("result").cloned().unwrap_or(Value::Null)))
}

async fn write_line<W: AsyncWriteExt + Unpin>(w: &mut W, line: &str) -> Result<()> {
    w.write_all(line.as_bytes())
        .await
        .map_err(|e| CoreError::Provider(format!("mcp write failed: {e}")))?;
    w.write_all(b"\n")
        .await
        .map_err(|e| CoreError::Provider(format!("mcp write failed: {e}")))?;
    w.flush()
        .await
        .map_err(|e| CoreError::Provider(format!("mcp flush failed: {e}")))?;
    Ok(())
}

async fn read_response<R: AsyncBufReadExt + Unpin>(
    reader: &mut tokio::io::Lines<R>,
    id: u64,
) -> Result<Value> {
    loop {
        match reader
            .next_line()
            .await
            .map_err(|e| CoreError::Provider(format!("mcp read failed: {e}")))?
        {
            Some(line) => {
                if line.trim().is_empty() {
                    continue;
                }
                if let Some(res) = parse_response_for(&line, id) {
                    return res;
                }
            }
            None => {
                return Err(CoreError::Provider(
                    "mcp server closed before responding".to_string(),
                ))
            }
        }
    }
}

/// The MCP stdio handshake + one `tools/call`, over any reader/writer pair
/// (a child's stdio in production; an in-memory duplex in tests).
async fn mcp_exchange<W, R>(
    stdin: &mut W,
    reader: &mut tokio::io::Lines<R>,
    tool: &str,
    arguments: &Value,
) -> Result<Value>
where
    W: AsyncWriteExt + Unpin,
    R: AsyncBufReadExt + Unpin,
{
    let init = jsonrpc_request(
        1,
        "initialize",
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "fleety", "version": env!("CARGO_PKG_VERSION") }
        }),
    );
    write_line(stdin, &init).await?;
    read_response(reader, 1).await?;
    write_line(
        stdin,
        &jsonrpc_notification("notifications/initialized", json!({})),
    )
    .await?;
    let call = jsonrpc_request(
        2,
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    );
    write_line(stdin, &call).await?;
    read_response(reader, 2).await
}

struct McpList {
    builtin: PathBuf,
    installed: PathBuf,
    /// Per-conversation servers (e.g. from enabled plugins); highest precedence.
    conversation: Vec<ServerCfg>,
}

#[async_trait]
impl Tool for McpList {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mcp_list".to_string(),
            description: "List configured MCP servers (built-in + user-installed). Each entry's `source` is `\"builtin\"` or `\"installed\"`; installed shadows builtin by name.".to_string(),
            parameters: json!({ "type": "object", "properties": {} }),
            risk: RiskLevel::Read,
        }
    }

    async fn call(&self, _args: Value) -> Result<Value> {
        let servers: Vec<Value> = load_merged(&self.builtin, &self.installed, &self.conversation)?
            .into_iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "command": s.command,
                    "args": s.args,
                    "source": if s.builtin { "builtin" } else { "installed" },
                })
            })
            .collect();
        Ok(json!({ "servers": servers }))
    }
}

struct McpAdd {
    installed: PathBuf,
}

#[async_trait]
impl Tool for McpAdd {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mcp_add".to_string(),
            description: "Add (or replace) a user-installed MCP server by name. Shadows a built-in of the same name.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string" },
                    "command": { "type": "string" },
                    "args": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["name", "command"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let name = require_str(&args, "name")?.to_string();
        let command = require_str(&args, "command")?.to_string();
        let server_args = args
            .get("args")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let mut servers = load_one(&self.installed, false)?;
        servers.retain(|s| s.name != name);
        servers.push(ServerCfg {
            name: name.clone(),
            command,
            args: server_args,
            builtin: false,
        });
        save_installed(&self.installed, &servers)?;
        Ok(json!({ "name": name, "added": true }))
    }
}

struct McpRemove {
    builtin: PathBuf,
    installed: PathBuf,
}

#[async_trait]
impl Tool for McpRemove {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mcp_remove".to_string(),
            description: "Remove a user-installed MCP server by name. Built-in servers cannot be removed; to override one, mcp_add a server of the same name (it shadows the built-in).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": { "name": { "type": "string" } },
                "required": ["name"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let name = require_str(&args, "name")?;
        let mut servers = load_one(&self.installed, false)?;
        let before = servers.len();
        servers.retain(|s| s.name != name);
        if servers.len() == before {
            // Was it a built-in?
            let is_builtin = load_one(&self.builtin, true)?
                .iter()
                .any(|s| s.name == name);
            if is_builtin {
                return Err(CoreError::Message(format!(
                    "cannot remove built-in mcp server '{name}'; use mcp_add to override it with a server of the same name"
                )));
            }
            return Err(CoreError::Message(format!("no such mcp server '{name}'")));
        }
        save_installed(&self.installed, &servers)?;
        Ok(json!({ "name": name, "removed": true }))
    }
}

struct McpCall {
    builtin: PathBuf,
    installed: PathBuf,
    /// Per-conversation servers (e.g. from enabled plugins); highest precedence.
    conversation: Vec<ServerCfg>,
}

#[async_trait]
impl Tool for McpCall {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "mcp_call".to_string(),
            description: "Call a tool on a configured MCP server (spawns it over stdio JSON-RPC)."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "server": { "type": "string" },
                    "tool": { "type": "string" },
                    "arguments": { "type": "object" }
                },
                "required": ["server", "tool"]
            }),
            risk: RiskLevel::Mutate,
        }
    }

    async fn call(&self, args: Value) -> Result<Value> {
        let server_name = require_str(&args, "server")?;
        let tool = require_str(&args, "tool")?.to_string();
        let arguments = args.get("arguments").cloned().unwrap_or_else(|| json!({}));

        let server = load_merged(&self.builtin, &self.installed, &self.conversation)?
            .into_iter()
            .find(|s| s.name == server_name)
            .ok_or_else(|| CoreError::ToolNotFound(format!("mcp server '{server_name}'")))?;

        let mut child = Command::new(&server.command)
            .args(&server.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                CoreError::Message(format!("cannot spawn mcp server '{server_name}': {e}"))
            })?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| CoreError::Message("mcp server has no stdin".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| CoreError::Message("mcp server has no stdout".to_string()))?;
        let mut reader = BufReader::new(stdout).lines();

        let exchange = mcp_exchange(&mut stdin, &mut reader, &tool, &arguments);
        let result = tokio::time::timeout(CALL_TIMEOUT, exchange).await;
        let _ = child.start_kill();
        match result {
            Ok(inner) => inner,
            Err(_) => Err(CoreError::Provider(format!(
                "mcp server '{server_name}' timed out after {}s",
                CALL_TIMEOUT.as_secs()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths() -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("fleety-mcp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).expect("mk temp");
        (base.join("builtin.json"), base.join("installed.json"))
    }

    #[test]
    fn load_merged_conversation_shadows() {
        let (builtin, installed) = temp_paths();
        std::fs::write(
            &installed,
            r#"{"servers":[{"name":"s","command":"old","args":[]}]}"#,
        )
        .expect("w installed");
        let conv = vec![ServerCfg {
            name: "s".to_string(),
            command: "new".to_string(),
            args: vec![],
            builtin: false,
        }];
        let merged = load_merged(&builtin, &installed, &conv).expect("merge");
        assert_eq!(
            merged.iter().filter(|x| x.name == "s").count(),
            1,
            "a conversation server shadows the installed one by name"
        );
        assert_eq!(
            merged.iter().find(|x| x.name == "s").unwrap().command,
            "new"
        );
    }

    #[tokio::test]
    async fn conversation_server_in_mcp_list() {
        let (builtin, installed) = temp_paths();
        let mut reg = ToolRegistry::new();
        register(
            &mut reg,
            &builtin,
            &installed,
            vec![ServerCfg {
                name: "ps".to_string(),
                command: "node".to_string(),
                args: vec![],
                builtin: false,
            }],
        );
        let listed = reg.call("mcp_list", json!({})).await.expect("list");
        assert!(
            listed["servers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["name"] == json!("ps")),
            "a per-conversation (plugin) server appears in mcp_list"
        );
    }

    #[tokio::test]
    async fn codex_mcp_in_conversation() {
        // A Codex config.toml server → collect_codex_mcp → ServerCfg → mcp_list.
        let home = std::env::temp_dir().join(format!("fleety-cx-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(home.join(".codex")).expect("mk codex dir");
        std::fs::write(
            home.join(".codex").join("config.toml"),
            "[mcp_servers.cx]\ncommand = \"node\"\nargs = [\"c.js\"]\n",
        )
        .expect("w config");
        let conv: Vec<ServerCfg> = crate::codex_sources::collect_codex_mcp(&home)
            .into_iter()
            .map(|m| ServerCfg {
                name: m.name,
                command: m.command,
                args: m.args,
                builtin: false,
            })
            .collect();
        let (builtin, installed) = temp_paths();
        let mut reg = ToolRegistry::new();
        register(&mut reg, &builtin, &installed, conv);
        let listed = reg.call("mcp_list", json!({})).await.expect("list");
        assert!(
            listed["servers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["name"] == json!("cx")),
            "a Codex config.toml server appears in mcp_list"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    #[tokio::test]
    async fn mcp_exchange_handshake_and_call() {
        use tokio::io::{AsyncWriteExt, BufReader};
        // Client <-> mock-server over an in-memory duplex (no subprocess).
        let (client, server) = tokio::io::duplex(8192);
        let (client_rd, mut client_wr) = tokio::io::split(client);
        let mut client_reader = BufReader::new(client_rd).lines();

        let server_task = tokio::spawn(async move {
            let (srd, mut swr) = tokio::io::split(server);
            let mut lines = BufReader::new(srd).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let v: Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                match v.get("id").and_then(Value::as_u64) {
                    Some(1) => {
                        let _ = swr
                            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n")
                            .await;
                    }
                    Some(2) => {
                        let name = v["params"]["name"].as_str().unwrap_or("").to_string();
                        let resp = format!(
                            "{{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"echo\":\"{name}\"}}}}\n"
                        );
                        let _ = swr.write_all(resp.as_bytes()).await;
                    }
                    _ => {} // the `initialized` notification needs no reply
                }
                let _ = swr.flush().await;
            }
        });

        let result = mcp_exchange(
            &mut client_wr,
            &mut client_reader,
            "do_thing",
            &json!({ "x": 1 }),
        )
        .await
        .expect("exchange");
        assert_eq!(result["echo"], json!("do_thing"));
        server_task.abort();
    }

    #[test]
    fn jsonrpc_framing() {
        let req = jsonrpc_request(2, "tools/call", json!({ "name": "x" }));
        let v: Value = serde_json::from_str(&req).expect("json");
        assert_eq!(v["jsonrpc"], json!("2.0"));
        assert_eq!(v["id"], json!(2));
        assert_eq!(v["method"], json!("tools/call"));

        // matching id -> result
        let line = r#"{"jsonrpc":"2.0","id":2,"result":{"ok":true}}"#;
        assert_eq!(
            parse_response_for(line, 2).expect("some").expect("ok"),
            json!({ "ok": true })
        );
        // error
        let err = r#"{"jsonrpc":"2.0","id":2,"error":{"message":"boom"}}"#;
        assert!(parse_response_for(err, 2).expect("some").is_err());
        // wrong id / notification -> None
        assert!(parse_response_for(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#, 2).is_none());
        assert!(parse_response_for(r#"{"jsonrpc":"2.0","method":"note"}"#, 2).is_none());
    }

    #[tokio::test]
    async fn config_crud() {
        let (builtin, installed) = temp_paths();
        let mut registry = ToolRegistry::new();
        register(&mut registry, &builtin, &installed, Vec::new());

        registry
            .call(
                "mcp_add",
                json!({ "name": "files", "command": "node", "args": ["server.js"] }),
            )
            .await
            .expect("add");
        let listed = registry.call("mcp_list", json!({})).await.expect("list");
        assert_eq!(listed["servers"].as_array().map(Vec::len).unwrap_or(0), 1);
        assert_eq!(listed["servers"][0]["command"], json!("node"));
        assert_eq!(listed["servers"][0]["source"], json!("installed"));

        registry
            .call("mcp_remove", json!({ "name": "files" }))
            .await
            .expect("remove");
        let after = registry.call("mcp_list", json!({})).await.expect("list2");
        assert_eq!(after["servers"].as_array().map(Vec::len).unwrap_or(0), 0);
        assert!(registry
            .call("mcp_remove", json!({ "name": "nope" }))
            .await
            .is_err());

        if let Some(dir) = builtin.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[tokio::test]
    async fn builtin_shadowing_and_remove_protection() {
        let (builtin, installed) = temp_paths();
        // Seed a builtin entry directly.
        std::fs::write(
            &builtin,
            r#"{"servers":[{"name":"ddgs","command":"ddgs","args":["mcp"]}]}"#,
        )
        .expect("seed");

        let mut registry = ToolRegistry::new();
        register(&mut registry, &builtin, &installed, Vec::new());

        // mcp_list shows the builtin with source flag.
        let listed = registry.call("mcp_list", json!({})).await.expect("list");
        let servers = listed["servers"].as_array().expect("arr");
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0]["name"], json!("ddgs"));
        assert_eq!(servers[0]["source"], json!("builtin"));

        // mcp_remove refuses to delete a builtin.
        let err = registry
            .call("mcp_remove", json!({ "name": "ddgs" }))
            .await
            .expect_err("must refuse");
        assert!(err.report().message.contains("built-in"));

        // mcp_add overrides the builtin → shadowed; entry now reports installed.
        registry
            .call(
                "mcp_add",
                json!({ "name": "ddgs", "command": "/usr/local/bin/ddgs", "args": ["mcp", "--debug"] }),
            )
            .await
            .expect("override");
        let shadowed = registry.call("mcp_list", json!({})).await.expect("list2");
        let arr = shadowed["servers"].as_array().expect("arr");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["source"], json!("installed"));
        assert_eq!(arr[0]["command"], json!("/usr/local/bin/ddgs"));

        // Removing the installed override exposes the builtin again.
        registry
            .call("mcp_remove", json!({ "name": "ddgs" }))
            .await
            .expect("remove installed");
        let back = registry.call("mcp_list", json!({})).await.expect("list3");
        assert_eq!(back["servers"][0]["source"], json!("builtin"));

        if let Some(dir) = builtin.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
