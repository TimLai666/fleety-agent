//! `fleety server` — manage the named server connection profiles that live in
//! `~/.fleety/connections.toml` (see [`fleety_tools::connection`]).
//!
//! These subcommands are the window onto "which server this device talks to":
//! `add`/`use`/`list`/`show`/`current`/`rename`/`remove`/`set-url`. They are
//! pure file operations (no network); enrollment (`--pair`, and the `init`/`pair`
//! sugar) lives in `main.rs` because it needs a connection. `use` changes only
//! the `current` field — the CLI and this host's daemon both follow it.

use std::path::Path;

use agent_core::{CoreError, Result};
use fleety_tools::connection::{self, Connections, Profile};

/// A parsed `fleety server` subcommand. Pure and unit-testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cmd {
    Add {
        name: String,
        url: String,
        label: Option<String>,
        use_current: bool,
    },
    Use(String),
    List,
    Show(Option<String>),
    Current,
    Rename {
        old: String,
        new: String,
    },
    Remove {
        name: String,
        force: bool,
    },
    SetUrl {
        name: String,
        url: String,
    },
    Help,
}

const USAGE: &str = "usage: fleety server <add|use|list|show|current|rename|remove|set-url>\n\
     \x20 add <name> <ws-url> [--label <text>] [--use]\n\
     \x20 use <name>              switch the current server\n\
     \x20 list                    all servers (* = current)\n\
     \x20 show [<name>]           details for one server (default: current)\n\
     \x20 current                 the current server\n\
     \x20 rename <old> <new>      rename a server\n\
     \x20 remove <name> [--force] delete a server (--force if it is current)\n\
     \x20 set-url <name> <ws-url> change a server's url";

/// Parse `server <args...>`. Pure; a missing required argument or unknown flag
/// is an error (so a typo never silently no-ops).
pub fn parse(args: &[String]) -> Result<Cmd> {
    let sub = args.first().map(String::as_str);
    let rest = args.get(1..).unwrap_or(&[]);
    let need = |o: Option<&String>, what: &str| -> Result<String> {
        o.cloned()
            .ok_or_else(|| CoreError::Message(format!("missing {what}\n{USAGE}")))
    };
    match sub {
        Some("add") => {
            let name = need(rest.first(), "server name")?;
            let url = need(rest.get(1), "server url")?;
            let mut label = None;
            let mut use_current = false;
            let mut i = 2;
            while i < rest.len() {
                match rest[i].as_str() {
                    "--use" => {
                        use_current = true;
                        i += 1;
                    }
                    "--label" => {
                        label = Some(need(rest.get(i + 1), "value for --label")?);
                        i += 2;
                    }
                    other => {
                        return Err(CoreError::Message(format!(
                            "unknown flag '{other}' for `server add`\n{USAGE}"
                        )))
                    }
                }
            }
            Ok(Cmd::Add {
                name,
                url,
                label,
                use_current,
            })
        }
        Some("use") => Ok(Cmd::Use(need(rest.first(), "server name")?)),
        Some("list") => Ok(Cmd::List),
        Some("show") => Ok(Cmd::Show(rest.first().cloned())),
        Some("current") => Ok(Cmd::Current),
        Some("rename") => Ok(Cmd::Rename {
            old: need(rest.first(), "old name")?,
            new: need(rest.get(1), "new name")?,
        }),
        Some("remove") => {
            let name = need(rest.first(), "server name")?;
            let force = rest.iter().skip(1).any(|a| a == "--force");
            Ok(Cmd::Remove { name, force })
        }
        Some("set-url") => Ok(Cmd::SetUrl {
            name: need(rest.first(), "server name")?,
            url: need(rest.get(1), "server url")?,
        }),
        Some("help") | None => Ok(Cmd::Help),
        Some(other) => Err(CoreError::Message(format!(
            "unknown `server` subcommand '{other}'\n{USAGE}"
        ))),
    }
}

/// Reject a url that is not a WebSocket url up front — the raw connect error a
/// bad scheme would later cause is much harder to act on.
fn check_ws_url(url: &str) -> Result<()> {
    if url.starts_with("ws://") || url.starts_with("wss://") {
        Ok(())
    } else if url.starts_with("http://") || url.starts_with("https://") {
        Err(CoreError::Message(format!(
            "'{url}' is an http(s) url — the server url uses the WebSocket scheme (ws:// or wss://)"
        )))
    } else {
        Err(CoreError::Message(format!(
            "'{url}' is not a ws:// or wss:// url (e.g. ws://192.168.1.10:8787)"
        )))
    }
}

/// Execute a parsed command against the connections file at `path`, returning
/// the rendered output. Loads (empty when missing, error when corrupt), mutates,
/// and saves atomically. `env_url` is the current `FLEETY_AGENT_URL` (if any) so
/// `list` can warn that it overrides the current profile.
pub fn apply_at(path: &Path, cmd: Cmd, env_url: Option<String>) -> Result<String> {
    match cmd {
        Cmd::Help => Ok(USAGE.to_string()),
        Cmd::Add {
            name,
            url,
            label,
            use_current,
        } => {
            check_ws_url(&url)?;
            let mut conns = connection::load_at(path)?;
            if conns.profiles.contains_key(&name) {
                return Err(CoreError::Message(format!(
                    "server '{name}' already exists — change it with `fleety server set-url {name} <url>` or pick another name"
                )));
            }
            // Auto-select when this is the first server (so a fresh device gets a
            // usable target), or when --use is given.
            let becomes_current = use_current || conns.current.is_none();
            conns.profiles.insert(
                name.clone(),
                Profile {
                    url,
                    token: None,
                    label,
                    fingerprint: None,
                },
            );
            if becomes_current {
                conns.current = Some(name.clone());
            }
            connection::save_at(path, &conns)?;
            Ok(if becomes_current {
                format!("added server '{name}' and switched to it")
            } else {
                format!("added server '{name}' (use it with `fleety server use {name}`)")
            })
        }
        Cmd::Use(name) => {
            let mut conns = connection::load_at(path)?;
            if !conns.profiles.contains_key(&name) {
                return Err(unknown_server(&name, &conns));
            }
            conns.current = Some(name.clone());
            connection::save_at(path, &conns)?;
            Ok(format!("now using server '{name}'"))
        }
        Cmd::List => Ok(render_list(&connection::load_at(path)?, env_url)),
        Cmd::Show(name) => {
            let conns = connection::load_at(path)?;
            let name = match name.or_else(|| conns.current.clone()) {
                Some(n) => n,
                None => return Ok("(no current server — add one with `fleety server add`)".to_string()),
            };
            let p = conns
                .profiles
                .get(&name)
                .ok_or_else(|| unknown_server(&name, &conns))?;
            Ok(render_show(&name, p, conns.current.as_deref() == Some(&name)))
        }
        Cmd::Current => {
            let conns = connection::load_at(path)?;
            let name = match &conns.current {
                Some(n) => n.clone(),
                None => return Ok("(no current server)".to_string()),
            };
            let url = conns
                .profiles
                .get(&name)
                .map(|p| p.url.clone())
                .unwrap_or_default();
            Ok(if url.is_empty() {
                name
            } else {
                format!("{name}  {url}")
            })
        }
        Cmd::Rename { old, new } => {
            let mut conns = connection::load_at(path)?;
            let profile = conns
                .profiles
                .remove(&old)
                .ok_or_else(|| unknown_server(&old, &conns))?;
            if conns.profiles.contains_key(&new) {
                // Put it back so a failed rename doesn't drop the profile.
                conns.profiles.insert(old, profile);
                return Err(CoreError::Message(format!("server '{new}' already exists")));
            }
            conns.profiles.insert(new.clone(), profile);
            if conns.current.as_deref() == Some(&old) {
                conns.current = Some(new.clone());
            }
            connection::save_at(path, &conns)?;
            Ok(format!("renamed server '{old}' → '{new}'"))
        }
        Cmd::Remove { name, force } => {
            let mut conns = connection::load_at(path)?;
            if !conns.profiles.contains_key(&name) {
                return Err(unknown_server(&name, &conns));
            }
            if conns.current.as_deref() == Some(&name) && !force {
                return Err(CoreError::Message(format!(
                    "'{name}' is the current server — switch with `fleety server use <other>` first, or pass --force"
                )));
            }
            conns.profiles.remove(&name);
            if conns.current.as_deref() == Some(&name) {
                conns.current = None;
            }
            connection::save_at(path, &conns)?;
            Ok(format!("removed server '{name}'"))
        }
        Cmd::SetUrl { name, url } => {
            check_ws_url(&url)?;
            let mut conns = connection::load_at(path)?;
            if !conns.profiles.contains_key(&name) {
                return Err(unknown_server(&name, &conns));
            }
            if let Some(p) = conns.profiles.get_mut(&name) {
                p.url = url;
            }
            connection::save_at(path, &conns)?;
            Ok(format!("set url for server '{name}'"))
        }
    }
}

/// A "no such server" error that also lists the ones that do exist.
fn unknown_server(name: &str, conns: &Connections) -> CoreError {
    let known: Vec<&str> = conns.profiles.keys().map(String::as_str).collect();
    if known.is_empty() {
        CoreError::Message(format!(
            "no server named '{name}' (none defined — add one with `fleety server add`)"
        ))
    } else {
        CoreError::Message(format!(
            "no server named '{name}' (have: {})",
            known.join(", ")
        ))
    }
}

fn render_list(conns: &Connections, env_url: Option<String>) -> String {
    let mut out = String::new();
    if let Some(u) = env_url.filter(|s| !s.is_empty()) {
        out.push_str(&format!(
            "note: FLEETY_AGENT_URL={u} is overriding the current server for this shell \
             (`server use` takes effect once it is unset)\n\n"
        ));
    }
    if conns.profiles.is_empty() {
        out.push_str("(no servers — add one with `fleety server add <name> <ws-url>`)");
        return out;
    }
    for (name, p) in &conns.profiles {
        let marker = if conns.current.as_deref() == Some(name) {
            "*"
        } else {
            " "
        };
        let url = if p.url.is_empty() {
            "(mDNS)".to_string()
        } else {
            p.url.clone()
        };
        let auth = if p.token.is_some() { "paired" } else { "no token" };
        out.push_str(&format!("{marker} {name:<16} {url}  [{auth}]\n"));
    }
    out.push_str("\n(* = current; switch with `fleety server use <name>`)");
    out
}

fn render_show(name: &str, p: &Profile, is_current: bool) -> String {
    let mut out = format!("server '{name}'{}\n", if is_current { " (current)" } else { "" });
    out.push_str(&format!(
        "  url:          {}\n",
        if p.url.is_empty() {
            "(none — falls back to mDNS)"
        } else {
            &p.url
        }
    ));
    if let Some(label) = &p.label {
        out.push_str(&format!("  label:        {label}\n"));
    }
    out.push_str(&format!(
        "  auth:         {}\n",
        if p.token.is_some() {
            "paired (token stored)"
        } else {
            "no token"
        }
    ));
    if let Some(fp) = &p.fingerprint {
        out.push_str(&format!("  fingerprint:  {fp}\n"));
    }
    out
}

/// Parse + execute a `fleety server` subcommand against the default
/// connections.toml, printing the result.
pub fn run(args: &[String]) -> Result<()> {
    let cmd = parse(args)?;
    let env_url = std::env::var("FLEETY_AGENT_URL").ok();
    let out = apply_at(&connection::connections_path(), cmd, env_url)?;
    let out = out.trim_end_matches('\n');
    if !out.is_empty() {
        println!("{out}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(p: &[&str]) -> Vec<String> {
        p.iter().map(|s| s.to_string()).collect()
    }

    fn tmp() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("fleety-server-{}.toml", uuid::Uuid::new_v4()))
    }

    #[test]
    fn parse_add_flags_and_errors() {
        assert_eq!(
            parse(&v(&["add", "home", "ws://h:8787", "--use", "--label", "Home"])).unwrap(),
            Cmd::Add {
                name: "home".into(),
                url: "ws://h:8787".into(),
                label: Some("Home".into()),
                use_current: true,
            }
        );
        // Missing url is an error; unknown flag is an error.
        assert!(parse(&v(&["add", "home"])).is_err());
        assert!(parse(&v(&["add", "home", "ws://h", "--bogus"])).is_err());
        assert!(parse(&v(&["frobnicate"])).is_err());
    }

    #[test]
    fn add_first_becomes_current_and_list_marks_it() {
        let p = tmp();
        let out = apply_at(&p, parse(&v(&["add", "home", "ws://home:8787"])).unwrap(), None).unwrap();
        assert!(out.contains("switched to it"), "first add auto-selects: {out}");
        let list = apply_at(&p, Cmd::List, None).unwrap();
        assert!(list.contains("* home"), "current is starred: {list}");
        // `current` prints the name + url.
        let cur = apply_at(&p, Cmd::Current, None).unwrap();
        assert!(cur.contains("home") && cur.contains("ws://home:8787"), "{cur}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn add_second_without_use_keeps_current_then_use_switches() {
        let p = tmp();
        apply_at(&p, parse(&v(&["add", "home", "ws://home:8787"])).unwrap(), None).unwrap();
        let out = apply_at(&p, parse(&v(&["add", "work", "ws://work:8787"])).unwrap(), None).unwrap();
        assert!(out.contains("use it with"), "second add without --use stays: {out}");
        // home is still current.
        assert!(apply_at(&p, Cmd::List, None).unwrap().contains("* home"));
        // `use work` switches.
        apply_at(&p, parse(&v(&["use", "work"])).unwrap(), None).unwrap();
        assert!(apply_at(&p, Cmd::List, None).unwrap().contains("* work"));
        // A duplicate name is rejected.
        assert!(apply_at(&p, parse(&v(&["add", "home", "ws://x:1"])).unwrap(), None).is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn remove_current_requires_force() {
        let p = tmp();
        apply_at(&p, parse(&v(&["add", "home", "ws://home:8787"])).unwrap(), None).unwrap();
        // Removing the current server without --force is rejected.
        let err = apply_at(&p, parse(&v(&["remove", "home"])).unwrap(), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--force"), "must demand --force: {err}");
        // With --force it is removed and current is cleared.
        apply_at(&p, parse(&v(&["remove", "home", "--force"])).unwrap(), None).unwrap();
        assert!(apply_at(&p, Cmd::Current, None).unwrap().contains("no current"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn rename_moves_profile_and_follows_current() {
        let p = tmp();
        apply_at(&p, parse(&v(&["add", "home", "ws://home:8787"])).unwrap(), None).unwrap();
        apply_at(&p, parse(&v(&["rename", "home", "house"])).unwrap(), None).unwrap();
        // current followed the rename.
        assert!(apply_at(&p, Cmd::List, None).unwrap().contains("* house"));
        assert!(apply_at(&p, parse(&v(&["use", "home"])).unwrap(), None).is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn set_url_changes_target_and_show_reports_it() {
        let p = tmp();
        apply_at(&p, parse(&v(&["add", "home", "ws://old:8787"])).unwrap(), None).unwrap();
        apply_at(&p, parse(&v(&["set-url", "home", "ws://new:9000"])).unwrap(), None).unwrap();
        let show = apply_at(&p, Cmd::Show(Some("home".into())), None).unwrap();
        assert!(show.contains("ws://new:9000"), "{show}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn list_warns_when_env_override_is_active() {
        let p = tmp();
        apply_at(&p, parse(&v(&["add", "home", "ws://home:8787"])).unwrap(), None).unwrap();
        let list = apply_at(&p, Cmd::List, Some("ws://env:8787".to_string())).unwrap();
        assert!(list.contains("FLEETY_AGENT_URL"), "env override warned: {list}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn add_rejects_non_ws_url() {
        let p = tmp();
        assert!(apply_at(&p, parse(&v(&["add", "home", "http://h:8787"])).unwrap(), None).is_err());
        assert!(!p.exists(), "an invalid add must not create the file");
    }
}
