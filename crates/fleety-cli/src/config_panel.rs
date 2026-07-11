//! `fleety config` (bare, on a TTY) — the three-region interactive panel.
//!
//! One entry point, three Tab-switchable regions (design §7): **Connection**
//! (edits `~/.fleety/connections.toml`), **This device** (edits the local
//! Cli/Shared settings), and **Server** (edits the connected server's settings
//! over the structured `ConfigSnapshot`/`ConfigApply` wire when the server
//! supports it, per `Welcome.config_protocol`).
//!
//! This is the minimal-viable panel (design Risks note): each region does a
//! single-value edit; the Connection region also switches the current server.
//! Full interactive provider/model editing in the Server region is a follow-up —
//! use `fleety config provider|model …` for that meanwhile.

use std::collections::BTreeMap;

use agent_core::{CoreError, Result};
use fleety_protocol::{ChangeOp, ClientMsg, ConfigChange, ConfigEntry, ConfigTarget, ServerMsg};
use fleety_tools::connection::{self, Connections};
use ratatui::crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::input::LineEditor;

/// Which region has focus.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Connection,
    Local,
    Server,
}

impl Region {
    fn next(self) -> Self {
        match self {
            Region::Connection => Region::Local,
            Region::Local => Region::Server,
            Region::Server => Region::Connection,
        }
    }
    fn title(self) -> &'static str {
        match self {
            Region::Connection => "[1] Connection",
            Region::Local => "[2] This device",
            Region::Server => "[3] Server",
        }
    }
}

/// Keys whose overwrite could redirect data/credentials off-box — the panel
/// double-confirms before staging a change (mirrors the server-side audit).
fn is_sensitive(key: &str) -> bool {
    matches!(
        key,
        "FLEETY_MODEL_KEY"
            | "FLEETY_MODEL_BASE_URL"
            | "FLEETY_BACKUP_REPO"
            | "FLEETY_BACKUP_TOKEN"
            | "FLEETY_CODEX_AUTHORIZE_URL"
            | "FLEETY_CODEX_TOKEN_URL"
            | "FLEETY_CODEX_BACKEND_URL"
    )
}

/// The panel state. `on_key` is pure (no I/O) so it is unit-testable; the run
/// loop owns the terminal + the connection.
struct Panel {
    region: Region,
    sel: usize,
    /// `Some(editor)` while editing the selected row's value; the bool marks a
    /// pending sensitive-key confirmation before the edit is applied.
    edit: Option<LineEditor>,
    confirm_sensitive: Option<String>,
    // Connection region.
    conns: Connections,
    // Local region (Cli/Shared rows: key, scope, value, source).
    local: Vec<(String, String, String, String)>,
    local_map: fleety_tools::config::ConfigMap,
    // Server region.
    server_supported: bool,
    entries: Vec<ConfigEntry>,
    revision: String,
    /// Staged, not-yet-applied server changes (tri-state), keyed by setting.
    staged: BTreeMap<String, ConfigChange>,
    apply_now: bool,
    status: String,
    quit: bool,
}

impl Panel {
    fn new(
        conns: Connections,
        local_map: fleety_tools::config::ConfigMap,
        server_supported: bool,
        entries: Vec<ConfigEntry>,
        revision: String,
    ) -> Self {
        let local =
            fleety_tools::config::rows_in_scopes(&local_map, fleety_tools::config::LOCAL_SCOPES);
        Self {
            region: Region::Connection,
            sel: 0,
            edit: None,
            confirm_sensitive: None,
            conns,
            local,
            local_map,
            server_supported,
            entries,
            revision,
            staged: BTreeMap::new(),
            apply_now: false,
            status: "Tab: region · ↑/↓: move · Enter: edit · a: apply server · q: quit".to_string(),
            quit: false,
        }
    }

    /// The number of rows in the active region.
    fn rows_len(&self) -> usize {
        match self.region {
            Region::Connection => self.conns.profiles.len(),
            Region::Local => self.local.len(),
            Region::Server => self.entries.len(),
        }
    }

    /// The connection profile names in display order.
    fn profile_names(&self) -> Vec<String> {
        self.conns.profiles.keys().cloned().collect()
    }

    /// Begin editing the selected server entry (staging its current value),
    /// double-confirming a sensitive key first.
    fn begin_server_edit(&mut self) {
        let Some(entry) = self.entries.get(self.sel) else {
            return;
        };
        if is_sensitive(&entry.key) && self.confirm_sensitive.as_deref() != Some(&entry.key) {
            self.confirm_sensitive = Some(entry.key.clone());
            self.status = format!(
                "'{}' can redirect data/credentials — press Enter again to confirm editing it",
                entry.key
            );
            return;
        }
        self.confirm_sensitive = None;
        let mut ed = LineEditor::default();
        // Secrets are write-only: never prefill the (masked/empty) value.
        if !entry.secret {
            ed.set_text(entry.value.clone());
        }
        self.edit = Some(ed);
    }

    /// Commit the in-progress edit for the active region. Returns `true` when a
    /// local file was written (the run loop persists connection/local edits).
    fn commit_edit(&mut self, value: String) -> bool {
        let value = value.trim().to_string();
        match self.region {
            Region::Connection => {
                // Edit = set the selected profile's url.
                if let Some(name) = self.profile_names().get(self.sel).cloned() {
                    if let Some(p) = self.conns.profiles.get_mut(&name) {
                        p.url = value;
                        self.status = format!("set url for '{name}' (s to save)");
                        return false;
                    }
                }
                false
            }
            Region::Local => {
                if let Some((key, _, _, _)) = self.local.get(self.sel).cloned() {
                    if let Some(setting) = fleety_tools::config::find(&key) {
                        if let Err(e) = fleety_tools::config::validate(setting, &value) {
                            self.status = format!("error: {e}");
                            return false;
                        }
                        if value.is_empty() {
                            self.local_map.remove(&(setting.scope, key.clone()));
                        } else {
                            self.local_map
                                .insert((setting.scope, key.clone()), value.clone());
                        }
                        self.local = fleety_tools::config::rows_in_scopes(
                            &self.local_map,
                            fleety_tools::config::LOCAL_SCOPES,
                        );
                        self.status = format!("set {key} locally");
                        return true; // caller saves config.toml
                    }
                }
                false
            }
            Region::Server => {
                if let Some(entry) = self.entries.get(self.sel).cloned() {
                    // Tri-state: empty → clear, else set. (keep = untouched.)
                    let op = if value.is_empty() {
                        ChangeOp::Clear
                    } else {
                        ChangeOp::Set
                    };
                    self.staged.insert(
                        entry.key.clone(),
                        ConfigChange {
                            key: entry.key.clone(),
                            op,
                            value: if value.is_empty() { None } else { Some(value) },
                        },
                    );
                    self.status =
                        format!("staged '{}' — press a to apply to the server", entry.key);
                }
                false
            }
        }
    }
}

/// One key event. Returns `true` when the connection/local file should be
/// persisted by the run loop. Pure (no I/O).
fn on_key(p: &mut Panel, code: KeyCode) -> bool {
    if let Some(ed) = p.edit.as_mut() {
        match code {
            KeyCode::Char(c) => ed.insert(c),
            KeyCode::Backspace => ed.backspace(),
            KeyCode::Left => ed.left(),
            KeyCode::Right => ed.right(),
            KeyCode::Esc => {
                p.edit = None;
                p.status = "edit cancelled".to_string();
            }
            KeyCode::Enter => {
                let value = p.edit.take().map(|mut e| e.take()).unwrap_or_default();
                return p.commit_edit(value);
            }
            _ => {}
        }
        return false;
    }
    match code {
        KeyCode::Char('q') => p.quit = true,
        KeyCode::Tab => {
            p.region = p.region.next();
            p.sel = 0;
            p.confirm_sensitive = None;
        }
        KeyCode::Up => p.sel = p.sel.saturating_sub(1),
        KeyCode::Down => {
            let max = p.rows_len().saturating_sub(1);
            p.sel = (p.sel + 1).min(max);
        }
        KeyCode::Char('u') if p.region == Region::Connection => {
            if let Some(name) = p.profile_names().get(p.sel).cloned() {
                p.conns.current = Some(name.clone());
                p.status = format!("current → '{name}' (s to save)");
            }
        }
        KeyCode::Char('s') if p.region == Region::Connection => {
            // Persist connections.toml (handled by the run loop via return? no —
            // save here is I/O; the run loop saves on this flag). Signal via
            // status; the run loop checks and saves.
            p.status = "__save_conns__".to_string();
        }
        KeyCode::Char('a') if p.region == Region::Server => {
            if p.server_supported && !p.staged.is_empty() {
                p.apply_now = true;
            } else if !p.server_supported {
                p.status =
                    "this server does not support structured config; use `fleety config set …`"
                        .to_string();
            } else {
                p.status = "nothing staged".to_string();
            }
        }
        KeyCode::Enter => match p.region {
            Region::Server => p.begin_server_edit(),
            _ => {
                let mut ed = LineEditor::default();
                let prefill = match p.region {
                    Region::Connection => p
                        .profile_names()
                        .get(p.sel)
                        .and_then(|n| p.conns.profiles.get(n))
                        .map(|pr| pr.url.clone())
                        .unwrap_or_default(),
                    Region::Local => p.local.get(p.sel).map(|r| r.2.clone()).unwrap_or_default(),
                    Region::Server => String::new(),
                };
                ed.set_text(prefill);
                p.edit = Some(ed);
            }
        },
        _ => {}
    }
    false
}

fn render(f: &mut Frame, p: &Panel) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(f.area());

    // Region tab bar.
    let tabs = [Region::Connection, Region::Local, Region::Server]
        .iter()
        .map(|r| {
            if *r == p.region {
                format!("▸{}", r.title())
            } else {
                format!(" {}", r.title())
            }
        })
        .collect::<Vec<_>>()
        .join("   ");
    f.render_widget(Paragraph::new(tabs), chunks[0]);

    // Region body.
    let mut lines: Vec<Line> = Vec::new();
    match p.region {
        Region::Connection => {
            for (i, name) in p.profile_names().iter().enumerate() {
                let marker = if i == p.sel { "▶" } else { " " };
                let cur = if p.conns.current.as_deref() == Some(name) {
                    "*"
                } else {
                    " "
                };
                let url = p
                    .conns
                    .profiles
                    .get(name)
                    .map(|x| x.url.clone())
                    .unwrap_or_default();
                lines.push(Line::from(format!("{marker}{cur} {name:<14} {url}")));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(
                "(u: switch current · Enter: edit url · s: save)",
            ));
        }
        Region::Local => {
            for (i, (key, scope, value, source)) in p.local.iter().enumerate() {
                let marker = if i == p.sel { "▶" } else { " " };
                lines.push(Line::from(format!(
                    "{marker} [{scope:6}] {key:<24} = {value}  ({source})"
                )));
            }
        }
        Region::Server => {
            if !p.server_supported {
                lines.push(Line::from(
                    "This server does not support structured config (older version).",
                ));
                lines.push(Line::from("Use `fleety config set <KEY> <VALUE>` instead."));
            } else {
                for (i, e) in p.entries.iter().enumerate() {
                    let marker = if i == p.sel { "▶" } else { " " };
                    let shown = if e.secret {
                        if e.is_set {
                            "********"
                        } else {
                            "(unset)"
                        }
                    } else {
                        &e.value
                    };
                    let staged = if p.staged.contains_key(&e.key) {
                        " *staged"
                    } else {
                        ""
                    };
                    lines.push(Line::from(format!(
                        "{marker} [{:6}] {:<24} = {shown}{staged}",
                        e.scope, e.key
                    )));
                }
            }
        }
    }
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title("fleety config")),
        chunks[1],
    );

    // Footer: the edit buffer, or the status + (for the selected server entry)
    // its effect/description.
    let footer = if let Some(ed) = &p.edit {
        let (view, _) = ed.display_window(60);
        format!("> {view}   (Enter save · Esc cancel)")
    } else if p.region == Region::Server {
        match p.entries.get(p.sel) {
            Some(e) => {
                let eff = match e.effect {
                    Some(fleety_protocol::Effect::Restart) => "restart",
                    Some(fleety_protocol::Effect::NextConnection) => "next connection",
                    None => "-",
                };
                format!("{}  (effect: {eff})  · {}", p.status, e.description)
            }
            None => p.status.clone(),
        }
    } else {
        p.status.clone()
    };
    f.render_widget(Paragraph::new(footer).block(Block::bordered()), chunks[2]);
}

/// Open the interactive config panel: resolve + connect to the current server,
/// negotiate the structured-config capability, pull a snapshot if supported,
/// then run the three-region editor. Connection/local edits persist to their
/// files; server edits are staged and applied via `ConfigApply`.
pub async fn run() -> Result<()> {
    // Resolve + connect, capturing the Welcome so we know the server's config
    // protocol. On any connection failure, the panel still opens for the local
    // + connection regions; the server region reports it is unavailable.
    let (mut conn, cp): (
        Option<(
            fleety_tools::transport::Sender,
            fleety_tools::transport::Receiver,
        )>,
        Option<u32>,
    ) = match crate::open_panel().await {
        Ok((streams, cp)) => (Some(streams), Some(cp)),
        Err(e) => {
            eprintln!(
                "note: could not reach the server ({}) — the Server region is unavailable; \
                     the Connection and This-device regions still work.",
                e.report().message
            );
            (None, None)
        }
    };
    let mut server_supported = false;
    let mut entries: Vec<ConfigEntry> = Vec::new();
    let mut revision = String::new();
    if let (Some((tx, rx)), Some(cp)) = (conn.as_mut(), cp) {
        server_supported = cp >= 1;
        if server_supported {
            match pull_snapshot(tx, rx).await {
                Ok((rev, es)) => {
                    revision = rev;
                    entries = es;
                }
                Err(e) => {
                    server_supported = false;
                    eprintln!(
                        "note: snapshot failed ({}); Server region read-only.",
                        e.report().message
                    );
                }
            }
        }
    }

    let conns = connection::load()?;
    let local_map = fleety_tools::config::load(&fleety_tools::config::config_path());
    let mut app = Panel::new(conns, local_map, server_supported, entries, revision);

    // Key events arrive over a channel from a blocking reader thread.
    let (key_tx, mut key_rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || loop {
        match ratatui::crossterm::event::read() {
            Ok(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                if key_tx.send(k.code).is_err() {
                    break;
                }
            }
            Ok(_) => {}
            Err(_) => break,
        }
    });

    let mut terminal = ratatui::init();
    let result: Result<()> = loop {
        if let Err(e) = terminal.draw(|f| render(f, &app)) {
            break Err(CoreError::Message(format!("draw failed: {e}")));
        }
        if app.quit {
            break Ok(());
        }
        let Some(code) = key_rx.recv().await else {
            break Ok(());
        };
        let persist = on_key(&mut app, code);
        // Connection "save" is signalled via a status sentinel.
        if app.status == "__save_conns__" {
            app.status = match connection::save(&app.conns) {
                Ok(()) => "saved connections".to_string(),
                Err(e) => format!("save failed: {}", e.report().message),
            };
        }
        if persist {
            if let Err(e) =
                fleety_tools::config::save(&fleety_tools::config::config_path(), &app.local_map)
            {
                app.status = format!("save failed: {}", e.report().message);
            }
        }
        if app.apply_now {
            app.apply_now = false;
            if let Some((tx, rx)) = conn.as_mut() {
                let changes: Vec<ConfigChange> = app.staged.values().cloned().collect();
                app.status = match apply_changes(tx, rx, &app.revision, changes).await {
                    Ok(msg) => {
                        app.staged.clear();
                        // Re-pull so the revision + values reflect the applied state.
                        if let Ok((rev, es)) = pull_snapshot(tx, rx).await {
                            app.revision = rev;
                            app.entries = es;
                        }
                        msg
                    }
                    Err(e) => format!("apply failed: {}", e.report().message),
                };
            }
        }
    };
    ratatui::restore();
    if let Some((mut tx, _)) = conn {
        let _ = tx.close().await;
    }
    result
}

/// Pull a `ConfigSnapshot` for the Server target and return (revision, entries).
async fn pull_snapshot(
    tx: &mut fleety_tools::transport::Sender,
    rx: &mut fleety_tools::transport::Receiver,
) -> Result<(String, Vec<ConfigEntry>)> {
    crate::send(
        tx,
        &ClientMsg::ConfigSnapshot {
            target: ConfigTarget::Server,
        },
    )
    .await?;
    match crate::recv(rx).await? {
        Some(ServerMsg::ConfigSnapshotResult {
            revision, entries, ..
        }) => Ok((revision, entries)),
        Some(ServerMsg::Error { error }) => Err(CoreError::Provider(error.message)),
        other => Err(CoreError::Provider(format!(
            "expected a config snapshot, got {other:?}"
        ))),
    }
}

/// Send a `ConfigApply` and return the result message (or an error).
async fn apply_changes(
    tx: &mut fleety_tools::transport::Sender,
    rx: &mut fleety_tools::transport::Receiver,
    base_revision: &str,
    changes: Vec<ConfigChange>,
) -> Result<String> {
    crate::send(
        tx,
        &ClientMsg::ConfigApply {
            target: ConfigTarget::Server,
            base_revision: base_revision.to_string(),
            changes,
        },
    )
    .await?;
    match crate::recv(rx).await? {
        Some(ServerMsg::ConfigResult {
            ok: true, output, ..
        }) => Ok(if output.is_empty() {
            "applied".to_string()
        } else {
            output
        }),
        Some(ServerMsg::ConfigResult {
            ok: false, error, ..
        }) => Err(CoreError::Provider(
            error
                .map(|e| e.message)
                .unwrap_or_else(|| "apply rejected".to_string()),
        )),
        other => Err(CoreError::Provider(format!(
            "unexpected apply reply: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel_with_entries(entries: Vec<ConfigEntry>) -> Panel {
        Panel::new(
            Connections::default(),
            fleety_tools::config::ConfigMap::new(),
            true,
            entries,
            "rev-1".to_string(),
        )
    }

    fn entry(key: &str, value: &str, secret: bool) -> ConfigEntry {
        ConfigEntry {
            key: key.into(),
            scope: "server".into(),
            value: value.into(),
            default: String::new(),
            description: String::new(),
            secret,
            is_set: !value.is_empty(),
            effect: Some(fleety_protocol::Effect::Restart),
            choices: vec![],
        }
    }

    #[test]
    fn tab_cycles_three_regions() {
        let mut p = panel_with_entries(vec![]);
        assert!(matches!(p.region, Region::Connection));
        on_key(&mut p, KeyCode::Tab);
        assert!(matches!(p.region, Region::Local));
        on_key(&mut p, KeyCode::Tab);
        assert!(matches!(p.region, Region::Server));
        on_key(&mut p, KeyCode::Tab);
        assert!(matches!(p.region, Region::Connection));
    }

    #[test]
    fn server_edit_stages_a_tristate_change() {
        let mut p = panel_with_entries(vec![entry("FLEETY_POLICY", "full_access", false)]);
        p.region = Region::Server;
        // Enter → edit (prefilled with current value), type a new value, Enter.
        on_key(&mut p, KeyCode::Enter);
        assert!(p.edit.is_some());
        // Replace with "require_approval".
        p.edit = Some(LineEditor::default());
        for c in "require_approval".chars() {
            on_key(&mut p, KeyCode::Char(c));
        }
        on_key(&mut p, KeyCode::Enter);
        let staged = p.staged.get("FLEETY_POLICY").expect("staged");
        assert_eq!(staged.op, ChangeOp::Set);
        assert_eq!(staged.value.as_deref(), Some("require_approval"));
        // Empty value stages a Clear.
        p.sel = 0;
        on_key(&mut p, KeyCode::Enter);
        p.edit = Some(LineEditor::default()); // empty
        on_key(&mut p, KeyCode::Enter);
        assert_eq!(p.staged.get("FLEETY_POLICY").unwrap().op, ChangeOp::Clear);
    }

    #[test]
    fn secret_edit_is_write_only_and_sensitive_confirms() {
        let mut p = panel_with_entries(vec![entry("FLEETY_MODEL_KEY", "", true)]);
        p.region = Region::Server;
        // First Enter on a sensitive key only arms a confirmation (no editor yet).
        on_key(&mut p, KeyCode::Enter);
        assert!(p.edit.is_none());
        assert_eq!(p.confirm_sensitive.as_deref(), Some("FLEETY_MODEL_KEY"));
        // Second Enter opens the editor, with NO prefilled (masked) value.
        on_key(&mut p, KeyCode::Enter);
        let ed = p.edit.as_ref().expect("editor open");
        assert!(ed.clone().take().is_empty(), "secret is never prefilled");
    }
}
