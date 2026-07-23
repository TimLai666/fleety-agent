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
        Some("use") if rest.len() == 1 => Ok(Cmd::Use(need(rest.first(), "server name")?)),
        Some("use") => Err(CoreError::Message(format!(
            "`server use` needs exactly one name\n{USAGE}"
        ))),
        Some("list") if rest.is_empty() => Ok(Cmd::List),
        Some("list") => Err(CoreError::Message(format!(
            "`server list` takes no arguments\n{USAGE}"
        ))),
        Some("show") if rest.len() <= 1 => Ok(Cmd::Show(rest.first().cloned())),
        Some("show") => Err(CoreError::Message(format!(
            "`server show` takes at most one name\n{USAGE}"
        ))),
        Some("current") if rest.is_empty() => Ok(Cmd::Current),
        Some("current") => Err(CoreError::Message(format!(
            "`server current` takes no arguments\n{USAGE}"
        ))),
        Some("rename") if rest.len() == 2 => Ok(Cmd::Rename {
            old: need(rest.first(), "old name")?,
            new: need(rest.get(1), "new name")?,
        }),
        Some("rename") => Err(CoreError::Message(format!(
            "`server rename` needs exactly two names\n{USAGE}"
        ))),
        Some("remove") if rest.len() == 1 || (rest.len() == 2 && rest[1] == "--force") => {
            let name = need(rest.first(), "server name")?;
            let force = rest.get(1).is_some();
            Ok(Cmd::Remove { name, force })
        }
        Some("remove") => Err(CoreError::Message(format!(
            "`server remove` accepts only <name> [--force]\n{USAGE}"
        ))),
        Some("set-url") if rest.len() == 2 => Ok(Cmd::SetUrl {
            name: need(rest.first(), "server name")?,
            url: need(rest.get(1), "server url")?,
        }),
        Some("set-url") => Err(CoreError::Message(format!(
            "`server set-url` needs exactly a name and URL\n{USAGE}"
        ))),
        Some("help" | "--help" | "-h") if rest.is_empty() => Ok(Cmd::Help),
        Some("help" | "--help" | "-h") => Err(CoreError::Message(format!(
            "server help takes no arguments\n{USAGE}"
        ))),
        None => Err(CoreError::Message(USAGE.to_string())),
        Some(other) => Err(CoreError::Message(format!(
            "unknown `server` subcommand '{other}'\n{USAGE}"
        ))),
    }
}

/// Reject a url that is not a WebSocket url up front — the raw connect error a
/// bad scheme would later cause is much harder to act on.
fn check_ws_url(url: &str) -> Result<()> {
    connection::validate_ws_url(url)
}

fn check_display_field(kind: &str, value: &str) -> Result<()> {
    if value.chars().any(char::is_control) {
        Err(CoreError::Message(format!(
            "{kind} cannot contain terminal control characters"
        )))
    } else {
        Ok(())
    }
}

fn safe_field(value: &str) -> String {
    crate::terminal_safe_field(value)
}

/// Connection commands do not need query parameter names to identify the
/// destination. Drop the complete query in addition to userinfo/fragment so a
/// legacy token-like key cannot leak through human or JSON domain output.
fn safe_endpoint(value: &str) -> String {
    let redacted = crate::redact_endpoint(value);
    redacted
        .split_once('?')
        .map(|(endpoint, _)| endpoint.to_string())
        .unwrap_or(redacted)
}

/// Execute a parsed command against the connections file at `path`, returning
/// the rendered output. Loads (empty when missing, error when corrupt), mutates,
/// and saves atomically. Context notices are deliberately excluded from this
/// domain result and emitted by [`run`] under the CLI output policy.
pub fn apply_at(path: &Path, cmd: Cmd, _env_url: Option<String>) -> Result<String> {
    match cmd {
        Cmd::Help => Ok(USAGE.to_string()),
        Cmd::Add {
            name,
            url,
            label,
            use_current,
        } => {
            check_display_field("server name", &name)?;
            if let Some(label) = &label {
                check_display_field("server label", label)?;
            }
            check_ws_url(&url)?;
            connection::mutate_at(path, |conns| {
                if conns.profiles.contains_key(&name) {
                    return Err(CoreError::Message(format!(
                        "server '{name}' already exists — change it with `fleety server set-url {name} <url>` or pick another name"
                    )));
                }
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
                Ok(if becomes_current {
                    format!("added server '{name}' and switched to it")
                } else {
                    format!("added server '{name}' (use it with `fleety server use {name}`)")
                })
            })
        }
        Cmd::Use(name) => {
            check_display_field("server name", &name)?;
            connection::mutate_at(path, |conns| {
                if !conns.profiles.contains_key(&name) {
                    return Err(unknown_server(&name, conns));
                }
                conns.current = Some(name.clone());
                Ok(format!("now using server '{name}'"))
            })
        }
        Cmd::List => Ok(render_list(&connection::load_at(path)?)),
        Cmd::Show(name) => {
            if let Some(name) = &name {
                check_display_field("server name", name)?;
            }
            let conns = connection::load_at(path)?;
            let name = match name.or_else(|| conns.current.clone()) {
                Some(n) => n,
                None => {
                    return Ok("(no current server — add one with `fleety server add`)".to_string())
                }
            };
            let p = conns
                .profiles
                .get(&name)
                .ok_or_else(|| unknown_server(&name, &conns))?;
            Ok(render_show(
                &name,
                p,
                conns.current.as_deref() == Some(&name),
            ))
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
                safe_field(&name)
            } else {
                format!("{}  {}", safe_field(&name), safe_endpoint(&url))
            })
        }
        Cmd::Rename { old, new } => {
            check_display_field("server name", &old)?;
            check_display_field("server name", &new)?;
            connection::mutate_at(path, |conns| {
                if conns.profiles.contains_key(&new) {
                    return Err(CoreError::Message(format!("server '{new}' already exists")));
                }
                let profile = conns
                    .profiles
                    .remove(&old)
                    .ok_or_else(|| unknown_server(&old, conns))?;
                conns.profiles.insert(new.clone(), profile);
                if conns.current.as_deref() == Some(&old) {
                    conns.current = Some(new.clone());
                }
                Ok(format!("renamed server '{old}' → '{new}'"))
            })
        }
        Cmd::Remove { name, force } => {
            check_display_field("server name", &name)?;
            connection::mutate_at(path, |conns| {
                if !conns.profiles.contains_key(&name) {
                    return Err(unknown_server(&name, conns));
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
                Ok(format!("removed server '{name}'"))
            })
        }
        Cmd::SetUrl { name, url } => {
            check_display_field("server name", &name)?;
            check_ws_url(&url)?;
            connection::mutate_at(path, |conns| {
                let Some(profile) = conns.profiles.get_mut(&name) else {
                    return Err(unknown_server(&name, conns));
                };
                let cleared = connection::reselect_profile_endpoint(profile, url);
                Ok(if cleared {
                    format!(
                        "set url for server '{name}'; cleared the old token and identity pin — \
                         re-pair with `fleety --profile <name> pair <code>` before using this profile"
                    )
                } else {
                    format!("set url for server '{name}'")
                })
            })
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

fn render_list(conns: &Connections) -> String {
    let mut out = String::new();
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
            safe_endpoint(&p.url)
        };
        let auth = if p.token.is_some() {
            "paired"
        } else {
            "no token"
        };
        out.push_str(&format!(
            "{marker} {:<16} {url}  [{auth}]\n",
            safe_field(name)
        ));
    }
    out.push_str("\n(* = current; switch with `fleety server use <name>`)");
    out
}

fn render_show(name: &str, p: &Profile, is_current: bool) -> String {
    let mut out = format!(
        "server '{}'{}\n",
        safe_field(name),
        if is_current { " (current)" } else { "" }
    );
    let endpoint = if p.url.is_empty() {
        "(none — falls back to mDNS)".to_string()
    } else {
        safe_endpoint(&p.url)
    };
    out.push_str(&format!("  url:          {endpoint}\n"));
    if let Some(label) = &p.label {
        out.push_str(&format!("  label:        {}\n", safe_field(label)));
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
        out.push_str(&format!("  fingerprint:  {}\n", safe_field(fp)));
    }
    out
}

/// Parse + execute a `fleety server` subcommand against the default
/// connections.toml, printing the result.
pub fn run(args: &[String]) -> Result<()> {
    let cmd = parse(args)?;
    let switched_profile = match &cmd {
        Cmd::Use(name) => Some(name.clone()),
        Cmd::Add {
            name,
            use_current: true,
            ..
        } => Some(name.clone()),
        _ => None,
    };
    let env_url = std::env::var("FLEETY_AGENT_URL").ok();
    if matches!(cmd, Cmd::List) && !crate::quiet_mode() && !crate::json_mode() {
        if let Some(url) = env_url.as_deref().filter(|url| !url.is_empty()) {
            eprintln!(
                "note: FLEETY_AGENT_URL={} overrides the current profile for this shell; \
                 `connection use` takes effect after it is unset",
                crate::terminal_safe_endpoint(url)
            );
        }
    }
    let out = apply_at(&connection::connections_path(), cmd, None)?;
    let daemon_notice = match switched_profile.as_deref() {
        Some(profile) => Some(notify_daemon_reconnect(profile).map_err(|error| {
            CoreError::Message(format!(
                "server profile '{profile}' was saved, but fleetyd was not notified: {}",
                error.report().message
            ))
        })?),
        None => None,
    };
    let out = out.trim_end_matches('\n');
    if !out.is_empty() {
        crate::output_stdout(crate::terminal_safe_multiline_redacted(out), true);
    }
    if let Some(notice) = daemon_notice.filter(|notice| !notice.is_empty()) {
        crate::output_stdout(crate::terminal_safe_multiline_redacted(&notice), true);
    }
    Ok(())
}

/// Ask the local Daemon owner to leave its current Server session and resolve
/// the newly selected profile. The CLI never edits daemon state directly: it
/// delegates to fleetyd's acknowledged local control command and propagates a
/// non-zero result instead of pretending the two processes are synchronized.
pub(crate) fn notify_daemon_reconnect(profile: &str) -> Result<String> {
    let program = fleety_tools::update::sibling_exe("fleetyd")
        .unwrap_or_else(|| std::path::PathBuf::from("fleetyd"));
    let output = std::process::Command::new(&program)
        .args(["reconnect", "--profile", profile])
        .output()
        .map_err(|error| {
            CoreError::Message(format!(
                "cannot run daemon owner command ({}): {error}; run `fleetyd reconnect --profile {profile}` manually",
                program.display()
            ))
        })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(CoreError::Message(if detail.is_empty() {
            format!(
                "daemon owner command exited with {}; run `fleetyd reconnect --profile {profile}` manually",
                output.status
            )
        } else {
            detail
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
            parse(&v(&[
                "add",
                "home",
                "ws://h:8787",
                "--use",
                "--label",
                "Home"
            ]))
            .unwrap(),
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
    fn websocket_urls_reject_credentials_missing_hosts_and_controls() {
        for invalid in [
            "wss://user:password@example.test/ws",
            "ws://",
            "ws://example.test/\nnext",
        ] {
            let error = check_ws_url(invalid)
                .expect_err("unsafe endpoint must be rejected")
                .to_string();
            assert!(
                error.contains("server url") || error.contains("endpoint"),
                "actionable error for {invalid:?}: {error}"
            );
        }
    }

    #[test]
    fn legacy_connection_output_redacts_endpoint_secrets_and_terminal_controls() {
        let name = "bad\n\u{1b}[31mname".to_string();
        let profile = Profile {
            url: "wss://user:password@example.test/ws?token=secret#fragment".into(),
            token: Some("stored-token".into()),
            label: Some("label\r\n\u{1b}[2Jclear".into()),
            fingerprint: Some("fp\n\u{1b}[1mvalue".into()),
        };
        let mut conns = Connections {
            current: Some(name.clone()),
            ..Connections::default()
        };
        conns.profiles.insert(name.clone(), profile.clone());
        let path = tmp();
        connection::save_at(&path, &conns).expect("seed legacy profile");

        let outputs = [
            render_list(&conns),
            render_show(&name, &profile, true),
            apply_at(&path, Cmd::Current, None).expect("render current profile"),
        ];
        for output in outputs {
            assert!(!output.contains("password"), "password leaked: {output:?}");
            assert!(!output.contains("secret"), "query leaked: {output:?}");
            assert!(!output.contains("user@"), "userinfo leaked: {output:?}");
            assert!(!output.contains('\u{1b}'), "ESC leaked: {output:?}");
            assert!(
                !output.contains("bad\n"),
                "name injected a line: {output:?}"
            );
            assert!(
                !output.contains("label\r\n"),
                "label injected lines: {output:?}"
            );
            assert!(
                output.contains("wss://example.test/ws"),
                "safe endpoint missing: {output:?}"
            );
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn add_first_becomes_current_and_list_marks_it() {
        let p = tmp();
        let out = apply_at(
            &p,
            parse(&v(&["add", "home", "ws://home:8787"])).unwrap(),
            None,
        )
        .unwrap();
        assert!(
            out.contains("switched to it"),
            "first add auto-selects: {out}"
        );
        let list = apply_at(&p, Cmd::List, None).unwrap();
        assert!(list.contains("* home"), "current is starred: {list}");
        // `current` prints the name + url.
        let cur = apply_at(&p, Cmd::Current, None).unwrap();
        assert!(
            cur.contains("home") && cur.contains("ws://home:8787"),
            "{cur}"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn add_second_without_use_keeps_current_then_use_switches() {
        let p = tmp();
        apply_at(
            &p,
            parse(&v(&["add", "home", "ws://home:8787"])).unwrap(),
            None,
        )
        .unwrap();
        let out = apply_at(
            &p,
            parse(&v(&["add", "work", "ws://work:8787"])).unwrap(),
            None,
        )
        .unwrap();
        assert!(
            out.contains("use it with"),
            "second add without --use stays: {out}"
        );
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
        apply_at(
            &p,
            parse(&v(&["add", "home", "ws://home:8787"])).unwrap(),
            None,
        )
        .unwrap();
        // Removing the current server without --force is rejected.
        let err = apply_at(&p, parse(&v(&["remove", "home"])).unwrap(), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("--force"), "must demand --force: {err}");
        // With --force it is removed and current is cleared.
        apply_at(&p, parse(&v(&["remove", "home", "--force"])).unwrap(), None).unwrap();
        assert!(apply_at(&p, Cmd::Current, None)
            .unwrap()
            .contains("no current"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn rename_moves_profile_and_follows_current() {
        let p = tmp();
        apply_at(
            &p,
            parse(&v(&["add", "home", "ws://home:8787"])).unwrap(),
            None,
        )
        .unwrap();
        apply_at(&p, parse(&v(&["rename", "home", "house"])).unwrap(), None).unwrap();
        // current followed the rename.
        assert!(apply_at(&p, Cmd::List, None).unwrap().contains("* house"));
        assert!(apply_at(&p, parse(&v(&["use", "home"])).unwrap(), None).is_err());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn set_url_changes_target_and_show_reports_it() {
        let p = tmp();
        apply_at(
            &p,
            parse(&v(&["add", "home", "ws://old:8787"])).unwrap(),
            None,
        )
        .unwrap();
        let mut paired = connection::load_at(&p).unwrap();
        paired.profiles.get_mut("home").unwrap().token = Some("old-token".into());
        paired.profiles.get_mut("home").unwrap().fingerprint = Some("old-pin".into());
        connection::save_at(&p, &paired).unwrap();
        let message = apply_at(
            &p,
            parse(&v(&["set-url", "home", "ws://new:9000"])).unwrap(),
            None,
        )
        .unwrap();
        assert!(message.contains("re-pair"), "{message}");
        let saved = connection::load_at(&p).unwrap();
        assert_eq!(saved.profiles["home"].token, None);
        assert_eq!(saved.profiles["home"].fingerprint, None);
        let show = apply_at(&p, Cmd::Show(Some("home".into())), None).unwrap();
        assert!(show.contains("ws://new:9000"), "{show}");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn list_domain_result_excludes_environment_context() {
        let p = tmp();
        apply_at(
            &p,
            parse(&v(&["add", "home", "ws://home:8787"])).unwrap(),
            None,
        )
        .unwrap();
        let list = apply_at(&p, Cmd::List, Some("ws://env:8787".to_string())).unwrap();
        assert!(
            !list.contains("FLEETY_AGENT_URL"),
            "context prose stays outside domain result: {list}"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn add_rejects_non_ws_url() {
        let p = tmp();
        assert!(apply_at(
            &p,
            parse(&v(&["add", "home", "http://h:8787"])).unwrap(),
            None
        )
        .is_err());
        assert!(!p.exists(), "an invalid add must not create the file");
    }
}
