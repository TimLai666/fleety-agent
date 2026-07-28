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

const USAGE: &str = "usage: fleety connection <add|use|list|show|current|rename|remove|set-url>\n\
     \x20 add <name> <ws-url> [--label <text>] [--use]\n\
     \x20 use <name>              switch the current profile\n\
     \x20 list                    all profiles (* = current)\n\
     \x20 show [<name>]           details for one profile (default: current)\n\
     \x20 current                 the current profile\n\
     \x20 rename <old> <new>      rename a profile\n\
     \x20 remove <name> [--force] delete a non-current profile\n\
     \x20 set-url <name> <ws-url> change a profile's url";

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
            let name = need(rest.first(), "profile name")?;
            let url = need(rest.get(1), "profile URL")?;
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
                            "unknown flag '{other}' for `connection add`\n{USAGE}"
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
        Some("use") if rest.len() == 1 => Ok(Cmd::Use(need(rest.first(), "profile name")?)),
        Some("use") => Err(CoreError::Message(format!(
            "`connection use` needs exactly one name\n{USAGE}"
        ))),
        Some("list") if rest.is_empty() => Ok(Cmd::List),
        Some("list") => Err(CoreError::Message(format!(
            "`connection list` takes no arguments\n{USAGE}"
        ))),
        Some("show") if rest.len() <= 1 => Ok(Cmd::Show(rest.first().cloned())),
        Some("show") => Err(CoreError::Message(format!(
            "`connection show` takes at most one name\n{USAGE}"
        ))),
        Some("current") if rest.is_empty() => Ok(Cmd::Current),
        Some("current") => Err(CoreError::Message(format!(
            "`connection current` takes no arguments\n{USAGE}"
        ))),
        Some("rename") if rest.len() == 2 => Ok(Cmd::Rename {
            old: need(rest.first(), "old name")?,
            new: need(rest.get(1), "new name")?,
        }),
        Some("rename") => Err(CoreError::Message(format!(
            "`connection rename` needs exactly two names\n{USAGE}"
        ))),
        Some("remove") if rest.len() == 1 || (rest.len() == 2 && rest[1] == "--force") => {
            let name = need(rest.first(), "profile name")?;
            let force = rest.get(1).is_some();
            Ok(Cmd::Remove { name, force })
        }
        Some("remove") => Err(CoreError::Message(format!(
            "`connection remove` accepts only <name> [--force]\n{USAGE}"
        ))),
        Some("set-url") if rest.len() == 2 => Ok(Cmd::SetUrl {
            name: need(rest.first(), "profile name")?,
            url: need(rest.get(1), "profile URL")?,
        }),
        Some("set-url") => Err(CoreError::Message(format!(
            "`connection set-url` needs exactly a name and URL\n{USAGE}"
        ))),
        Some("help" | "--help" | "-h") if rest.is_empty() => Ok(Cmd::Help),
        Some("help" | "--help" | "-h") => Err(CoreError::Message(format!(
            "connection help takes no arguments\n{USAGE}"
        ))),
        None => Err(CoreError::Message(USAGE.to_string())),
        Some(other) => Err(CoreError::Message(format!(
            "unknown `connection` subcommand '{other}'\n{USAGE}"
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
            check_display_field("profile name", &name)?;
            if let Some(label) = &label {
                check_display_field("profile label", label)?;
            }
            check_ws_url(&url)?;
            connection::mutate_at(path, |conns| {
                if conns.profiles.contains_key(&name) {
                    return Err(CoreError::Message(format!(
                        "profile '{name}' already exists — change it with `fleety connection set-url {name} <url>` or pick another name"
                    )));
                }
                let becomes_current = use_current || conns.current.is_none();
                conns.profiles.insert(
                    name.clone(),
                    Profile {
                        url,
                        endpoints: Vec::new(),
                        configured_url: None,
                        secure: false,
                        token: None,
                        label,
                        fingerprint: None,
                        generation: uuid::Uuid::new_v4().to_string(),
                    },
                );
                if becomes_current {
                    conns.current = Some(name.clone());
                }
                Ok(if becomes_current {
                    format!("added profile '{name}' and switched to it")
                } else {
                    format!("added profile '{name}' (use it with `fleety connection use {name}`)")
                })
            })
        }
        Cmd::Use(name) => {
            check_display_field("profile name", &name)?;
            connection::mutate_at(path, |conns| {
                if !conns.profiles.contains_key(&name) {
                    return Err(unknown_server(&name, conns));
                }
                conns.current = Some(name.clone());
                Ok(format!("now using profile '{name}'"))
            })
        }
        Cmd::List => Ok(render_list(&connection::load_at(path)?)),
        Cmd::Show(name) => {
            if let Some(name) = &name {
                check_display_field("profile name", name)?;
            }
            let conns = connection::load_at(path)?;
            let name = match name.or_else(|| conns.current.clone()) {
                Some(n) => n,
                None => {
                    return Ok(
                        "(no current profile — add one with `fleety connection add`)".to_string(),
                    )
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
                None => return Ok("(no current profile)".to_string()),
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
            check_display_field("profile name", &old)?;
            check_display_field("profile name", &new)?;
            connection::mutate_at(path, |conns| {
                if conns.profiles.contains_key(&new) {
                    return Err(CoreError::Message(format!(
                        "profile '{new}' already exists"
                    )));
                }
                let profile = conns
                    .profiles
                    .remove(&old)
                    .ok_or_else(|| unknown_server(&old, conns))?;
                conns.profiles.insert(new.clone(), profile);
                if conns.current.as_deref() == Some(&old) {
                    conns.current = Some(new.clone());
                }
                Ok(format!("renamed profile '{old}' → '{new}'"))
            })
        }
        Cmd::Remove {
            name,
            force: _force,
        } => {
            check_display_field("profile name", &name)?;
            connection::mutate_at(path, |conns| {
                if !conns.profiles.contains_key(&name) {
                    return Err(unknown_server(&name, conns));
                }
                if conns.current.as_deref() == Some(&name) {
                    return Err(CoreError::Message(format!(
                        "'{name}' is the current profile — explicitly switch with `fleety connection use <replacement>` before removing it; --force never chooses a profile for you"
                    )));
                }
                conns.profiles.remove(&name);
                Ok(format!("removed profile '{name}'"))
            })
        }
        Cmd::SetUrl { name, url } => {
            check_display_field("profile name", &name)?;
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
                    format!("set url for profile '{name}'")
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
            "no profile named '{name}' (none defined — add one with `fleety connection add`)"
        ))
    } else {
        CoreError::Message(format!(
            "no profile named '{name}' (have: {})",
            known.join(", ")
        ))
    }
}

fn render_list(conns: &Connections) -> String {
    let mut out = String::new();
    if conns.profiles.is_empty() {
        out.push_str("(no profiles — add one with `fleety connection add <name> <ws-url>`)");
        return out;
    }
    for (name, p) in &conns.profiles {
        let marker = if conns.current.as_deref() == Some(name) {
            "*"
        } else {
            " "
        };
        let url = if p.url.is_empty() {
            "(endpoint required — run `fleety init`)".to_string()
        } else {
            safe_endpoint(&p.url)
        };
        let auth = if p.token.is_some() {
            "paired"
        } else {
            "no token"
        };
        // Pad by display width, not character count: a CJK name is two columns
        // per character and `{:<16}` would shift every following column.
        let shown = safe_field(name);
        let pad = 16usize.saturating_sub(crate::workspace::display_width(&shown));
        // `--label` promises "a human-readable name shown in listings", so the
        // listing has to show it — it was only visible from `connection show`.
        let label = match p.label.as_deref().filter(|label| !label.is_empty()) {
            Some(label) => format!("  {}", safe_field(label)),
            None => String::new(),
        };
        out.push_str(&format!(
            "{marker} {shown}{:pad$} {url}  [{auth}]{label}\n",
            "",
            pad = pad
        ));
    }
    out.push_str("\n(* = current; switch with `fleety connection use <name>`)");
    out
}

fn render_show(name: &str, p: &Profile, is_current: bool) -> String {
    let mut out = format!(
        "profile '{}'{}\n",
        safe_field(name),
        if is_current { " (current)" } else { "" }
    );
    let endpoint = if p.url.is_empty() {
        "(none — select and pair with `fleety init`)".to_string()
    } else {
        safe_endpoint(&p.url)
    };
    out.push_str(&format!("  url:          {endpoint}\n"));
    // Roaming can move `url` to an address the user never typed, and the latch
    // decides which remediations work. Both have to be visible, or a refusal
    // that says "this profile requires an encrypted channel" cannot be checked.
    if let Some(configured) = &p.configured_url {
        out.push_str(&format!(
            "  configured:   {} (roamed; pairing still uses this)\n",
            safe_endpoint(configured)
        ));
    }
    if !p.endpoints.is_empty() {
        out.push_str(&format!(
            "  alternates:   {}\n",
            p.endpoints
                .iter()
                .map(|endpoint| safe_endpoint(endpoint))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out.push_str(&format!(
        "  channel:      {}\n",
        if p.secure {
            "encrypted (this Server has proven it; cleartext is refused)"
        } else {
            "cleartext until this Server proves it can encrypt"
        }
    ));
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
    let explicit_reconnect_profile = match &cmd {
        Cmd::Use(name)
        | Cmd::Add {
            name,
            use_current: true,
            ..
        } => Some(name.clone()),
        _ => None,
    };
    let path = connection::connections_path();
    let mutates_profiles = matches!(
        cmd,
        Cmd::Add { .. }
            | Cmd::Use(_)
            | Cmd::Rename { .. }
            | Cmd::Remove { .. }
            | Cmd::SetUrl { .. }
    );
    let before = mutates_profiles
        .then(|| connection::load_at(&path))
        .transpose()?;
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
    let out = apply_at(&path, cmd, None)?;
    let after = mutates_profiles
        .then(|| connection::load_at(&path))
        .transpose()?;
    let reconnect_profile = before
        .as_ref()
        .zip(after.as_ref())
        .and_then(|(before, after)| reconnect_profile_after_change(before, after))
        .or(explicit_reconnect_profile);
    let daemon_notice = match reconnect_profile.as_deref() {
        Some(profile) => Some(notify_daemon_reconnect(profile).map_err(|error| {
            CoreError::Message(format!(
                "profile '{profile}' was saved, but fleetyd was not notified: {}",
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

fn current_profile_generation(conns: &Connections) -> Option<(&str, &Profile)> {
    let name = conns.current.as_deref()?;
    conns.profiles.get(name).map(|profile| (name, profile))
}

fn reconnect_profile_after_change(before: &Connections, after: &Connections) -> Option<String> {
    (current_profile_generation(before) != current_profile_generation(after))
        .then(|| after.current.clone())
        .flatten()
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
            "wss://example.test/ws#fragment",
        ] {
            let error = check_ws_url(invalid)
                .expect_err("unsafe endpoint must be rejected")
                .to_string();
            assert!(
                error.contains("profile URL") || error.contains("endpoint"),
                "actionable error for {invalid:?}: {error}"
            );
        }
    }

    /// Roaming moves `url` to an address the user never typed and the latch
    /// decides which remediations work, so both have to be visible — a refusal
    /// that names the encrypted channel is unverifiable otherwise.
    #[test]
    fn show_surfaces_the_configured_address_alternates_and_channel_state() {
        let profile = Profile {
            url: "ws://100.64.0.8:8787".into(),
            configured_url: Some("ws://home.lan:8787".into()),
            endpoints: vec!["ws://192.168.1.20:8787".into()],
            secure: true,
            token: Some("tok".into()),
            ..Default::default()
        };

        let rendered = render_show("home", &profile, true);

        assert!(rendered.contains("ws://home.lan:8787"), "{rendered}");
        assert!(rendered.contains("roamed"), "{rendered}");
        assert!(rendered.contains("ws://192.168.1.20:8787"), "{rendered}");
        assert!(rendered.contains("encrypted"), "{rendered}");

        let plain = Profile {
            url: "ws://home.lan:8787".into(),
            ..Default::default()
        };
        let rendered = render_show("home", &plain, false);
        assert!(!rendered.contains("roamed"), "{rendered}");
        assert!(rendered.contains("cleartext until"), "{rendered}");
    }

    #[test]
    fn legacy_connection_output_redacts_endpoint_secrets_and_terminal_controls() {
        let name = "bad\n\u{1b}[31mname".to_string();
        let profile = Profile {
            url: "wss://user:password@example.test/ws?token=secret#fragment".into(),
            endpoints: Vec::new(),
            configured_url: None,
            secure: false,
            token: Some("stored-token".into()),
            label: Some("label\r\n\u{1b}[2Jclear".into()),
            fingerprint: Some("fp\n\u{1b}[1mvalue".into()),
            generation: "legacy-generation".into(),
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
    fn list_shows_the_label_because_that_is_what_it_is_for() {
        // `--label` is documented as "a human-readable name shown in listings",
        // but only `connection show` printed it.
        let labelled = Profile {
            url: "ws://10.0.0.4:8787".into(),
            label: Some("Lab box".into()),
            ..Profile::default()
        };
        let bare = Profile {
            url: "ws://10.0.0.5:8787".into(),
            ..Profile::default()
        };
        let mut conns = Connections {
            current: Some("lab".into()),
            ..Connections::default()
        };
        conns.profiles.insert("lab".into(), labelled);
        conns.profiles.insert("bare".into(), bare);

        let list = render_list(&conns);
        let lab_line = list
            .lines()
            .find(|line| line.contains("lab "))
            .unwrap_or_default();
        assert!(lab_line.contains("Lab box"), "label missing from {list:?}");
        // A profile with no label gains no stray column.
        let bare_line = list
            .lines()
            .find(|line| line.contains("bare"))
            .unwrap_or_default();
        assert!(
            bare_line.trim_end().ends_with("[no token]"),
            "unlabelled row changed shape: {bare_line:?}"
        );
    }

    #[test]
    fn urlless_profile_requires_explicit_init_in_list_and_show() {
        let profile = Profile::default();
        let mut conns = Connections {
            current: Some("legacy".into()),
            ..Connections::default()
        };
        conns.profiles.insert("legacy".into(), profile.clone());

        let list = render_list(&conns);
        let show = render_show("legacy", &profile, true);
        assert!(list.contains("endpoint required"), "{list}");
        assert!(
            show.contains("select and pair with `fleety init`"),
            "{show}"
        );
        assert!(!list.contains("(mDNS)"), "{list}");
        assert!(!show.contains("falls back to mDNS"), "{show}");
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
    fn remove_current_requires_an_explicit_switch() {
        let p = tmp();
        apply_at(
            &p,
            parse(&v(&["add", "home", "ws://home:8787"])).unwrap(),
            None,
        )
        .unwrap();
        // Removing the current server is rejected.
        let err = apply_at(&p, parse(&v(&["remove", "home"])).unwrap(), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("explicitly switch"), "{err}");
        // --force never guesses which replacement the user meant.
        let err = apply_at(&p, parse(&v(&["remove", "home", "--force"])).unwrap(), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("never chooses"), "{err}");
        apply_at(
            &p,
            parse(&v(&["add", "work", "ws://work:8787"])).unwrap(),
            None,
        )
        .unwrap();
        let err = apply_at(&p, parse(&v(&["remove", "home", "--force"])).unwrap(), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("explicitly switch"), "{err}");
        apply_at(&p, parse(&v(&["use", "work"])).unwrap(), None).unwrap();
        apply_at(&p, parse(&v(&["remove", "home"])).unwrap(), None).unwrap();
        assert!(apply_at(&p, Cmd::Current, None).unwrap().contains("work"));
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
    fn every_current_profile_generation_change_requests_daemon_reconnect() {
        let mut before = Connections {
            current: Some("home".to_string()),
            ..Default::default()
        };
        before.profiles.insert(
            "home".to_string(),
            Profile {
                url: "ws://home:8787".to_string(),
                ..Default::default()
            },
        );

        let mut changed_url = before.clone();
        changed_url.profiles.get_mut("home").unwrap().url = "ws://new:8787".to_string();
        assert_eq!(
            reconnect_profile_after_change(&before, &changed_url).as_deref(),
            Some("home")
        );

        let mut renamed = before.clone();
        let renamed_profile = renamed.profiles.remove("home").unwrap();
        renamed
            .profiles
            .insert("house".to_string(), renamed_profile);
        renamed.current = Some("house".to_string());
        assert_eq!(
            reconnect_profile_after_change(&before, &renamed).as_deref(),
            Some("house")
        );

        let mut unrelated = before.clone();
        unrelated.profiles.insert(
            "work".to_string(),
            Profile {
                url: "ws://work:8787".to_string(),
                ..Default::default()
            },
        );
        assert_eq!(reconnect_profile_after_change(&before, &unrelated), None);
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
