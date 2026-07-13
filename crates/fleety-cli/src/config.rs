//! `fleety config` — inspect and edit settings from the terminal.
//!
//! Backed by the shared typed registry in `fleety_tools::config`. `list/get`
//! show the resolved value and its source (env / config / default), secrets
//! masked; CLI-owned `set/unset` edit `~/.fleety/config.toml` after validating the key;
//! `edit` opens an interactive screen — a ratatui list when stdout is a TTY,
//! else a line-based loop. Read precedence stays env → config → default, so an
//! explicit env var always wins.

use std::io::IsTerminal;
use std::path::Path;

use agent_core::{CoreError, Result};
use fleety_protocol::{ClientMsg, ConfigTarget, ServerMsg};
use fleety_tools::config::{self, ConfigMap, Owner, Source};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Auto,
    Server,
    Daemon,
    Cli,
    Device(String),
}

/// Split a leading-or-embedded `--target <server|local|<device-id>>` out of the
/// config args, returning the target (default `Server`) and the remaining args.
/// Pure. `local` is handled by this CLI; `server`/`device` go over the wire.
pub fn split_target(args: &[String]) -> Result<(Target, Vec<String>)> {
    let mut target = Target::Auto;
    let mut seen = false;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--target" {
            if seen {
                return Err(CoreError::Message(
                    "--target may be specified only once".to_string(),
                ));
            }
            let v = args.get(i + 1).ok_or_else(|| {
                CoreError::Message(
                    "--target needs server, daemon, cli, local, or a device id".to_string(),
                )
            })?;
            target = match v.as_str() {
                "server" => Target::Server,
                "daemon" => Target::Daemon,
                "cli" | "local" => Target::Cli,
                other if other.starts_with('-') => {
                    return Err(CoreError::Message(format!(
                        "invalid config target '{other}'"
                    )))
                }
                other => Target::Device(other.to_string()),
            };
            seen = true;
            i += 2;
            continue;
        }
        rest.push(args[i].clone());
        i += 1;
    }
    Ok((target, rest))
}

fn command_owner(args: &[String]) -> Result<Option<Owner>> {
    match args.first().map(String::as_str) {
        Some("provider" | "model") => Ok(Some(Owner::Server)),
        Some("get" | "set" | "unset") => {
            let key = args.get(1).ok_or_else(|| {
                CoreError::Message(format!(
                    "config {} needs a setting key",
                    args.first().map(String::as_str).unwrap_or_default()
                ))
            })?;
            config::owner_for_key(key).map(Some)
        }
        Some("list") | None | Some("edit") => Ok(None),
        Some(other) => Err(CoreError::Message(format!(
            "unknown config command '{other}'"
        ))),
    }
}

/// Resolve automatic ownership and reject explicit target/owner mismatches
/// before any file or network I/O.
pub fn resolve_target(target: Target, args: &[String], device_id: &str) -> Result<Target> {
    let owner = command_owner(args)?;
    let resolved = match target {
        Target::Auto => match owner {
            Some(Owner::Server) => Target::Server,
            Some(Owner::Daemon) => Target::Device(device_id.to_string()),
            Some(Owner::Cli) => Target::Cli,
            None => Target::Auto,
        },
        other => other,
    };
    if let Some(owner) = owner {
        let matches = matches!(
            (&resolved, owner),
            (Target::Server, Owner::Server)
                | (Target::Cli, Owner::Cli)
                | (Target::Daemon | Target::Device(_), Owner::Daemon)
        );
        if !matches {
            let owner_name = match owner {
                Owner::Server => "server",
                Owner::Daemon => "daemon",
                Owner::Cli => "cli",
            };
            return Err(CoreError::Message(format!(
                "this setting is owned by {owner_name}; choose --target {owner_name}"
            )));
        }
    }
    Ok(resolved)
}

pub fn wire_target(target: &Target) -> Result<ConfigTarget> {
    match target {
        Target::Server => Ok(ConfigTarget::Server),
        Target::Daemon => Err(CoreError::Message(
            "daemon target must be resolved to a device id".to_string(),
        )),
        Target::Device(id) => Ok(ConfigTarget::Device(id.clone())),
        Target::Cli => Ok(ConfigTarget::Local),
        Target::Auto => Err(CoreError::Message(
            "config list/edit needs an explicit owner or the interactive panel".to_string(),
        )),
    }
}

/// Whether `args` is exactly the interactive provider editor invocation.
fn is_provider_edit(args: &[String]) -> bool {
    matches!(
        (
            args.first().map(String::as_str),
            args.get(1).map(String::as_str)
        ),
        (Some("provider"), Some("edit"))
    )
}

/// Pure routing: `provider edit` is available only against its server owner.
pub fn is_remote_provider_edit(args: &[String], target: &Target) -> bool {
    is_provider_edit(args) && matches!(target, Target::Server)
}

/// The version gate for remote provider editing: the full-config write-back
/// rides `ConfigApply.providers_json`, which servers before config protocol 2
/// silently ignore — refuse up front rather than losing the edit.
fn provider_edit_support_err(config_protocol: u32) -> Option<CoreError> {
    (config_protocol < 2).then(|| {
        CoreError::Message(
            "the connected server is too old for remote provider editing — update it first \
             (run `fleety update` on the server host); provider configuration is server-owned \
             and is never written directly by the CLI"
                .to_string(),
        )
    })
}

/// Interactive provider editing against the connected server: snapshot the
/// server's providers, edit them in memory, and apply the result under the
/// snapshot's optimistic-lock revision. A concurrent-edit conflict closes the
/// editor and reloads from a fresh snapshot instead of overwriting.
pub async fn provider_edit_remote() -> Result<()> {
    loop {
        let (mut tx, mut rx, config_protocol, _target) =
            crate::connect_hello_for_auth().await.map_err(|e| {
                CoreError::Message(format!(
                    "could not reach the server whose providers this would edit: {} — pair this \
                     device first (`fleety pair <code>`) or select the server with `fleety server \
                     use <name>`; provider configuration is never written locally by the CLI",
                    e.report().message
                ))
            })?;
        if let Some(err) = provider_edit_support_err(config_protocol) {
            return Err(err);
        }
        crate::send(
            &mut tx,
            &ClientMsg::ConfigSnapshot {
                target: ConfigTarget::Server,
            },
        )
        .await?;
        let (mut revision, providers_json) = match crate::recv(&mut rx).await? {
            Some(ServerMsg::ConfigSnapshotResult {
                revision,
                providers_json,
                ..
            }) => (revision, providers_json),
            Some(ServerMsg::ConfigResult { error: Some(e), .. }) => {
                return Err(CoreError::Message(match e.remediation {
                    Some(r) => format!("{} — {r}", e.message),
                    None => e.message,
                }))
            }
            other => {
                return Err(CoreError::Provider(format!(
                    "expected a config snapshot, got {other:?}"
                )))
            }
        };
        let cfg: fleety_tools::providers_config::ProvidersConfig =
            serde_json::from_str(&providers_json).map_err(|e| {
                CoreError::Message(format!(
                    "the server returned an unreadable provider snapshot: {e}"
                ))
            })?;

        // Credential status is server-owned. A status failure is deliberately
        // non-fatal so the editor never turns an unavailable query into a false
        // "not signed in" claim or blocks unrelated config edits.
        let mut auth_states = crate::provider_tui::ProviderAuthStates::new();
        for (provider_name, provider) in &cfg.providers {
            if provider.kind != "oauth:codex" {
                continue;
            }
            let state = if config_protocol < 3 {
                crate::provider_tui::ProviderAuthState::Unavailable
            } else {
                let query = async {
                    crate::send(
                        &mut tx,
                        &ClientMsg::CredentialStatus {
                            kind: "codex-oauth".to_string(),
                            provider: Some(provider_name.clone()),
                        },
                    )
                    .await?;
                    crate::recv(&mut rx).await
                }
                .await;
                match query {
                    Ok(Some(ServerMsg::CredentialStatusResult { present, error, .. }))
                        if error.is_none() =>
                    {
                        if present {
                            crate::provider_tui::ProviderAuthState::SignedIn
                        } else {
                            crate::provider_tui::ProviderAuthState::NotSignedIn
                        }
                    }
                    _ => crate::provider_tui::ProviderAuthState::Unavailable,
                }
            };
            auth_states.insert(provider_name.clone(), state);
        }

        // The editor loop is synchronous (crossterm events); each save runs the
        // async apply on the runtime from inside it.
        let handle = tokio::runtime::Handle::current();
        let io = std::rc::Rc::new(tokio::sync::Mutex::new((&mut tx, &mut rx)));
        let outcome = tokio::task::block_in_place(|| {
            crate::provider_tui::run_with_saver_and_fetcher(
                cfg,
                |edited| {
                    let json = serde_json::to_string(edited)
                        .map_err(|e| CoreError::Message(format!("serialize providers: {e}")))?;
                    handle.block_on(async {
                        let mut io = io.lock().await;
                        crate::send(
                            io.0,
                            &ClientMsg::ConfigApply {
                                target: ConfigTarget::Server,
                                base_revision: revision.clone(),
                                changes: vec![],
                                providers_json: Some(json),
                            },
                        )
                        .await?;
                        let reply = crate::recv(io.1).await?;
                        match reply {
                            Some(ServerMsg::ConfigResult { ok: true, .. }) => {
                                // Our own write moved the server's revision; refresh
                                // it so the next save in this session doesn't
                                // conflict with our own edit.
                                crate::send(
                                    io.0,
                                    &ClientMsg::ConfigSnapshot {
                                        target: ConfigTarget::Server,
                                    },
                                )
                                .await?;
                                let snapshot = crate::recv(io.1).await?;
                                if let Some(ServerMsg::ConfigSnapshotResult {
                                    revision: r, ..
                                }) = snapshot
                                {
                                    revision = r;
                                }
                                Ok(crate::provider_tui::SaveOutcome::Saved)
                            }
                            Some(ServerMsg::ConfigResult { error: Some(e), .. })
                                if e.kind == "conflict" =>
                            {
                                Ok(crate::provider_tui::SaveOutcome::Conflict(e.message))
                            }
                            Some(ServerMsg::ConfigResult { error: Some(e), .. }) => {
                                Err(CoreError::Message(match e.remediation {
                                    Some(r) => format!("{} — {r}", e.message),
                                    None => e.message,
                                }))
                            }
                            other => Err(CoreError::Provider(format!(
                                "expected a config result, got {other:?}"
                            ))),
                        }
                    })
                },
                |provider_name, _provider| {
                    if config_protocol < 4 {
                        return Err(
                            "server does not support provider model discovery (config protocol < 4)"
                                .to_string(),
                        );
                    }
                    handle
                        .block_on(async {
                            let mut io = io.lock().await;
                            crate::send(
                                io.0,
                                &ClientMsg::ProviderModelList {
                                    provider: provider_name.to_string(),
                                },
                            )
                            .await?;
                            let reply = crate::recv(io.1).await?;
                            match reply {
                                Some(ServerMsg::ProviderModelListResult {
                                    provider,
                                    model_ids,
                                    error,
                                }) if provider == provider_name => match error {
                                    None if !model_ids.is_empty() => Ok(model_ids),
                                    Some(e) => Err(CoreError::Message(match e.remediation {
                                        Some(r) => format!("{} — {r}", e.message),
                                        None => e.message,
                                    })),
                                    None => Err(CoreError::Message(
                                        "server returned no model IDs".to_string(),
                                    )),
                                },
                                other => Err(CoreError::Provider(format!(
                                    "expected provider model result, got {other:?}"
                                ))),
                            }
                        })
                        .map_err(|e: CoreError| e.to_string())
                },
                auth_states,
            )
        })?;
        // An OAuth action the editor asked for: the just-added/edited provider is
        // already applied to the server (the save above ran the ConfigApply), and
        // the editor tore down the full-screen UI so the browser flow can use the
        // plain terminal. Run the sign-in/out/switch against THIS server, then
        // reopen the editor on a fresh snapshot.
        if let Some(req) = outcome.auth_request {
            crate::config_panel::run_auth_action(&req).await;
            continue;
        }
        // A concurrent-edit conflict: reload from a fresh snapshot and reopen.
        match outcome.conflict {
            None => return Ok(()),
            Some(msg) => {
                println!("{msg} — reloading the current server configuration…");
                continue;
            }
        }
    }
}
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::input::LineEditor;

/// Run a `config` subcommand. The dispatch (list/get/set/unset/help + the
/// line-based edit) is shared in `fleety_tools::config` so `fleety-server` and
/// `fleetyd` expose the same `config` command; the CLI only overrides `edit` to
/// open the ratatui screen when stdout is a TTY.
pub fn run(args: &[String]) -> Result<()> {
    if matches!(args.first().map(String::as_str), Some("provider" | "model")) {
        return Err(CoreError::Message(
            "provider and model configuration is owned by the server; use `fleety config \
             --target server ...` (the CLI never edits providers.toml directly)"
                .to_string(),
        ));
    }
    if matches!(config::parse(args), config::Command::Edit) && std::io::stdout().is_terminal() {
        run_tui_edit(&config::config_path())
    } else {
        // The CLI's config path is always the *local* target (main.rs routes
        // remote via config_remote), so restrict it to this device's scopes.
        config::run_scoped(args, Some(config::LOCAL_SCOPES))
    }
}

// ---- ratatui edit screen (CLI-only; needs a TTY) ----

/// One row in the config screen (raw resolved value + its source).
struct Row {
    key: &'static str,
    scope: &'static str,
    secret: bool,
    value: String,
    source: String,
}

fn source_label(s: Source) -> &'static str {
    match s {
        Source::Env => "env",
        Source::Config => "config",
        Source::Default => "default",
    }
}

fn build_rows(map: &ConfigMap) -> Vec<Row> {
    // The local edit screen shows only CLI-owned settings. Shared settings are
    // daemon-owned and travel through fleetyd like other daemon settings.
    config::registry()
        .iter()
        .filter(|s| config::LOCAL_SCOPES.contains(&s.scope))
        .filter_map(|s| {
            let r = config::resolve(s.key, map)?;
            Some(Row {
                key: s.key,
                scope: s.scope.as_str(),
                secret: s.secret,
                value: r.value,
                source: source_label(r.source).to_string(),
            })
        })
        .collect()
}

/// Config-screen state. `on_key` is pure (no I/O) so it's unit-testable; the run
/// loop owns terminal + file I/O.
struct ConfigApp {
    rows: Vec<Row>,
    map: ConfigMap,
    sel: usize,
    /// `Some(editor)` while editing the selected row's value.
    edit: Option<LineEditor>,
    status: String,
    quit: bool,
}

impl ConfigApp {
    fn new(map: ConfigMap) -> Self {
        Self {
            rows: build_rows(&map),
            map,
            sel: 0,
            edit: None,
            status: "↑/↓ move · Enter edit · q quit".to_string(),
            quit: false,
        }
    }
}

/// Handle one key. Returns `true` when the map changed and should be saved.
fn on_key(app: &mut ConfigApp, key: KeyCode) -> bool {
    if let Some(ed) = app.edit.as_mut() {
        match key {
            KeyCode::Char(c) => ed.insert(c),
            KeyCode::Backspace => ed.backspace(),
            KeyCode::Delete => ed.delete(),
            KeyCode::Left => ed.left(),
            KeyCode::Right => ed.right(),
            KeyCode::Home => ed.home(),
            KeyCode::End => ed.end(),
            KeyCode::Esc => {
                app.edit = None;
                app.status = "edit cancelled".to_string();
            }
            KeyCode::Enter => {
                let buf = app.edit.take().map(|mut e| e.take()).unwrap_or_default();
                let Some(row) = app.rows.get(app.sel) else {
                    return false;
                };
                let Some(setting) = config::find(row.key) else {
                    return false;
                };
                if buf.is_empty() {
                    app.map.remove(&(setting.scope, setting.key.to_string()));
                    app.status = format!("unset {} (reverts to env/default)", setting.key);
                } else {
                    // Reject out-of-domain values before they reach the map/file;
                    // reopen the editor with the rejected text so the user can
                    // fix it, surface the reason, and skip the save.
                    if let Err(e) = config::validate(setting, &buf) {
                        app.status = e.to_string();
                        let mut ed = LineEditor::default();
                        ed.set_text(buf);
                        app.edit = Some(ed);
                        return false;
                    }
                    app.map
                        .insert((setting.scope, setting.key.to_string()), buf);
                    app.status = format!("set {}", setting.key);
                }
                if let Some(r) = config::resolve(setting.key, &app.map) {
                    app.rows[app.sel].value = r.value;
                    app.rows[app.sel].source = source_label(r.source).to_string();
                }
                return true;
            }
            _ => {}
        }
        return false;
    }
    match key {
        KeyCode::Up | KeyCode::Char('k') => app.sel = app.sel.saturating_sub(1),
        KeyCode::Down | KeyCode::Char('j') => {
            if app.sel + 1 < app.rows.len() {
                app.sel += 1;
            }
        }
        KeyCode::Enter => {
            // Prefill the raw value (never the mask) only when it really comes
            // from the config file — env values and placeholder defaults like
            // `(heuristic)` must not be one Enter away from being saved as if
            // they were literal values.
            let raw = app
                .rows
                .get(app.sel)
                .filter(|r| r.source == "config")
                .map(|r| r.value.clone())
                .unwrap_or_default();
            let mut ed = LineEditor::default();
            ed.set_text(raw);
            app.edit = Some(ed);
            app.status = "type a value · Enter save · empty=unset · Esc cancel".to_string();
        }
        KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
        _ => {}
    }
    false
}

fn render(f: &mut Frame, app: &ConfigApp) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(f.area());
    let inner_w = chunks[0].width.saturating_sub(2) as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(app.rows.len());
    for (i, r) in app.rows.iter().enumerate() {
        let shown = if let (true, Some(ed)) = (i == app.sel, &app.edit) {
            // Show the editor's window so the cursor stays visible even when
            // a long value outgrows the row (prefix width mirrors the format
            // string below with the ">" marker).
            let prefix = format!("> [{:7}] {:<28} = ", r.scope, r.key);
            let avail = inner_w.saturating_sub(Line::from(prefix.as_str()).width());
            ed.display_window(avail).0.to_string()
        } else if r.secret && !r.value.is_empty() {
            "********".to_string()
        } else {
            r.value.clone()
        };
        let marker = if i == app.sel { ">" } else { " " };
        lines.push(Line::from(format!(
            "{marker} [{:7}] {:<28} = {shown}  ({})",
            r.scope, r.key, r.source
        )));
    }
    // Scroll so the selected row stays visible when the registry outgrows the
    // pane (row i renders on content line i).
    let inner_h = chunks[0].height.saturating_sub(2);
    let offset = (app.sel as u16 + 1).saturating_sub(inner_h);
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("fleety config — settings"))
            .scroll((offset, 0)),
        chunks[0],
    );
    // While editing, put the terminal cursor at its column inside the value
    // (same window as the row above, so cursor and glyphs line up).
    if let (Some(ed), Some(r)) = (&app.edit, app.rows.get(app.sel)) {
        let prefix = format!("> [{:7}] {:<28} = ", r.scope, r.key);
        let prefix_w = Line::from(prefix.as_str()).width();
        let (_, x) = ed.display_window(inner_w.saturating_sub(prefix_w));
        f.set_cursor_position((
            chunks[0].x + 1 + (prefix_w + x as usize).min(inner_w) as u16,
            chunks[0].y + 1 + (app.sel as u16).saturating_sub(offset),
        ));
    }
    f.render_widget(
        Paragraph::new(app.status.clone()).block(Block::bordered()),
        chunks[1],
    );
}

fn run_tui_edit(path: &Path) -> Result<()> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};
    let mut app = ConfigApp::new(config::load(path));
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        loop {
            terminal
                .draw(|f| render(f, &app))
                .map_err(|e| CoreError::Message(format!("draw failed: {e}")))?;
            if app.quit {
                break;
            }
            if let Event::Key(k) =
                event::read().map_err(|e| CoreError::Message(format!("read failed: {e}")))?
            {
                if k.kind != KeyEventKind::Release && on_key(&mut app, k.code) {
                    config::save(path, &app.map)?;
                }
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_target_extracts_and_defaults() {
        let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // Default is automatic ownership, args untouched.
        let (t, rest) = split_target(&s(&["set", "FLEETY_MODEL", "gpt-5"])).unwrap();
        assert_eq!(t, Target::Auto);
        assert_eq!(rest, s(&["set", "FLEETY_MODEL", "gpt-5"]));
        // --target local is stripped.
        let (t, rest) = split_target(&s(&["--target", "local", "list"])).unwrap();
        assert_eq!(t, Target::Cli);
        assert_eq!(rest, s(&["list"]));
        // A non-server/local value is a device id; stripped from the middle.
        let (t, rest) = split_target(&s(&["provider", "--target", "pi", "list"])).unwrap();
        assert_eq!(t, Target::Device("pi".into()));
        assert_eq!(rest, s(&["provider", "list"]));
    }

    #[test]
    fn provider_edit_routes_by_target_and_gates_by_version() {
        let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // Server target uses the remote flow. CLI/local is not a provider owner,
        // so other dispatch code rejects it before any file access.
        assert!(is_remote_provider_edit(
            &s(&["provider", "edit"]),
            &Target::Server
        ));
        assert!(!is_remote_provider_edit(
            &s(&["provider", "edit"]),
            &Target::Cli
        ));
        assert!(!is_remote_provider_edit(
            &s(&["provider", "list"]),
            &Target::Server
        ));

        // Version gate: pre-credential-era servers silently drop the write-back
        // field, so the editor must not open against them.
        let msg = provider_edit_support_err(1)
            .expect("old server refused")
            .to_string();
        assert!(msg.contains("update"), "gate names the remedy: {msg}");
        assert!(msg.contains("server-owned"), "gate names ownership: {msg}");
        assert!(provider_edit_support_err(0).is_some());
        assert!(provider_edit_support_err(2).is_none());
        assert!(
            provider_edit_support_err(3).is_none(),
            "future versions pass"
        );
    }

    #[test]
    fn owner_route_matrix_and_target_mismatches() {
        let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            resolve_target(Target::Auto, &s(&["set", "FLEETY_ADDR", "x"]), "dev").unwrap(),
            Target::Server
        );
        assert_eq!(
            resolve_target(Target::Auto, &s(&["set", "FLEETY_TZ", "UTC"]), "dev").unwrap(),
            Target::Device("dev".into())
        );
        assert_eq!(
            resolve_target(
                Target::Auto,
                &s(&["set", "FLEETY_VOICE_AUDIO", "auto"]),
                "dev"
            )
            .unwrap(),
            Target::Cli
        );
        assert!(resolve_target(Target::Server, &s(&["set", "FLEETY_TZ", "UTC"]), "dev").is_err());
        assert!(resolve_target(Target::Cli, &s(&["provider", "edit"]), "dev").is_err());
        assert!(split_target(&s(&["--target"])).is_err());
    }

    #[test]
    fn config_tui_key_handling() {
        let mut app = ConfigApp::new(ConfigMap::new());
        assert!(app.rows.len() >= 2);

        // Navigation.
        on_key(&mut app, KeyCode::Down);
        assert_eq!(app.sel, 1);
        on_key(&mut app, KeyCode::Up);
        assert_eq!(app.sel, 0);

        // Drive the edit assertions on a CLI-owned key. Shared keys such as
        // FLEETY_TZ belong to fleetyd and intentionally do not appear here.
        app.sel = app
            .rows
            .iter()
            .position(|r| r.key == "FLEETY_VOICE_AUDIO")
            .expect("FLEETY_VOICE_AUDIO present");
        let key0 = app.rows[app.sel].key;
        let setting = config::find(key0).expect("known");

        // Edit then cancel → no change, no save.
        on_key(&mut app, KeyCode::Enter);
        assert!(app.edit.is_some());
        on_key(&mut app, KeyCode::Esc);
        assert!(app.edit.is_none());
        assert!(!app.map.contains_key(&(setting.scope, key0.to_string())));

        // Edit then commit a value → saved + map updated.
        on_key(&mut app, KeyCode::Enter);
        app.edit = Some(LineEditor::default()); // clear the prefilled default for a clean assert
        for c in "off".chars() {
            on_key(&mut app, KeyCode::Char(c));
        }
        assert!(on_key(&mut app, KeyCode::Enter));
        assert_eq!(
            app.map
                .get(&(setting.scope, key0.to_string()))
                .map(String::as_str),
            Some("off")
        );

        // Empty buffer → unset (removed from map).
        on_key(&mut app, KeyCode::Enter);
        app.edit = Some(LineEditor::default());
        assert!(on_key(&mut app, KeyCode::Enter));
        assert!(!app.map.contains_key(&(setting.scope, key0.to_string())));

        // Quit.
        on_key(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
    }

    #[test]
    fn config_tui_rejects_invalid_value() {
        let mut app = ConfigApp::new(ConfigMap::new());
        // Select a validated CLI-owned key.
        app.sel = app
            .rows
            .iter()
            .position(|r| r.key == "FLEETY_VOICE_AUDIO")
            .expect("FLEETY_VOICE_AUDIO present");
        let setting = config::find("FLEETY_VOICE_AUDIO").expect("known");

        // Type an out-of-domain value and commit it.
        on_key(&mut app, KeyCode::Enter);
        app.edit = Some(LineEditor::default()); // clear any prefill
        for c in "abc".chars() {
            on_key(&mut app, KeyCode::Char(c));
        }
        let saved = on_key(&mut app, KeyCode::Enter);

        // Not saved, not stored, and the error names the key + accepted values.
        assert!(!saved, "an invalid commit must not request a save");
        assert!(
            !app.map
                .contains_key(&(setting.scope, "FLEETY_VOICE_AUDIO".to_string())),
            "the rejected value must not enter the map"
        );
        assert!(
            app.status.contains("FLEETY_VOICE_AUDIO"),
            "status: {}",
            app.status
        );
        assert!(
            app.status.contains("auto") && app.status.contains("off"),
            "status should list the accepted voice values, got: {}",
            app.status
        );
    }

    #[test]
    fn build_rows_is_local_scope_only() {
        let rows = build_rows(&ConfigMap::new());
        assert!(!rows.is_empty());
        for r in &rows {
            assert!(
                r.scope == "cli",
                "local edit rows are CLI-only, got {} [{}]",
                r.key,
                r.scope
            );
        }
        assert!(
            !rows.iter().any(|r| r.key == "FLEETY_TZ"),
            "a Shared key is excluded"
        );
        assert!(
            !rows.iter().any(|r| r.key == "FLEETY_ADDR"),
            "a Server key is excluded"
        );
    }

    #[test]
    fn config_edit_cursor_keys() {
        let mut app = ConfigApp::new(ConfigMap::new());
        on_key(&mut app, KeyCode::Enter);
        app.edit = Some(LineEditor::default()); // start from a clean buffer
        for c in "abc".chars() {
            on_key(&mut app, KeyCode::Char(c));
        }
        // Left + insert edits mid-value; Home/Delete removes the first char.
        on_key(&mut app, KeyCode::Left);
        on_key(&mut app, KeyCode::Char('X'));
        on_key(&mut app, KeyCode::Home);
        on_key(&mut app, KeyCode::Delete);
        assert_eq!(app.edit.as_ref().map(|e| e.text()), Some("bXc"));
        // End puts the cursor back for a tail backspace.
        on_key(&mut app, KeyCode::End);
        on_key(&mut app, KeyCode::Backspace);
        assert_eq!(app.edit.as_ref().map(|e| e.text()), Some("bX"));
        on_key(&mut app, KeyCode::Esc);
        assert!(app.edit.is_none());
    }
}
