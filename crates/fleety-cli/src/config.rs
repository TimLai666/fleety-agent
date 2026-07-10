//! `fleety config` — inspect and edit settings from the terminal.
//!
//! Backed by the shared typed registry in `fleety_tools::config`. `list/get`
//! show the resolved value and its source (env / config / default), secrets
//! masked; `set/unset` edit `~/.fleety/config.toml` after validating the key;
//! `edit` opens an interactive screen — a ratatui list when stdout is a TTY,
//! else a line-based loop. Read precedence stays env → config → default, so an
//! explicit env var always wins.

use std::io::IsTerminal;
use std::path::Path;

use agent_core::{CoreError, Result};
use fleety_protocol::ConfigTarget;
use fleety_tools::config::{self, ConfigMap, Source};

/// Split a leading-or-embedded `--target <server|local|<device-id>>` out of the
/// config args, returning the target (default `Server`) and the remaining args.
/// Pure. `local` is handled by this CLI; `server`/`device` go over the wire.
pub fn split_target(args: &[String]) -> (ConfigTarget, Vec<String>) {
    let mut target = ConfigTarget::Server;
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--target" {
            if let Some(v) = args.get(i + 1) {
                target = match v.as_str() {
                    "server" => ConfigTarget::Server,
                    "local" => ConfigTarget::Local,
                    other => ConfigTarget::Device(other.to_string()),
                };
                i += 2;
                continue;
            }
        }
        rest.push(args[i].clone());
        i += 1;
    }
    (target, rest)
}

/// Whether `args` is an interactive edit (`edit` / `provider edit`), which is
/// always local + TTY (remote interactive editing is a follow-up).
pub fn is_interactive_edit(args: &[String]) -> bool {
    matches!(args.first().map(String::as_str), Some("edit"))
        || matches!(
            (
                args.first().map(String::as_str),
                args.get(1).map(String::as_str)
            ),
            (Some("provider"), Some("edit"))
        )
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
    // `config provider edit` on a TTY opens the interactive providers screen;
    // without a TTY it falls through to the shared subcommand dispatch.
    let head = (
        args.first().map(String::as_str),
        args.get(1).map(String::as_str),
    );
    if head == (Some("provider"), Some("edit")) && std::io::stdout().is_terminal() {
        return crate::provider_tui::run(&fleety_tools::providers_config::providers_path());
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
    // The local edit screen shows only this device's own settings (Cli/Shared);
    // server/daemon settings are edited on their own hosts.
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
        // Default is Server, args untouched.
        let (t, rest) = split_target(&s(&["set", "FLEETY_MODEL", "gpt-5"]));
        assert_eq!(t, ConfigTarget::Server);
        assert_eq!(rest, s(&["set", "FLEETY_MODEL", "gpt-5"]));
        // --target local is stripped.
        let (t, rest) = split_target(&s(&["--target", "local", "list"]));
        assert_eq!(t, ConfigTarget::Local);
        assert_eq!(rest, s(&["list"]));
        // A non-server/local value is a device id; stripped from the middle.
        let (t, rest) = split_target(&s(&["provider", "--target", "pi", "list"]));
        assert_eq!(t, ConfigTarget::Device("pi".into()));
        assert_eq!(rest, s(&["provider", "list"]));
        // Edit detection.
        assert!(is_interactive_edit(&s(&["edit"])));
        assert!(is_interactive_edit(&s(&["provider", "edit"])));
        assert!(!is_interactive_edit(&s(&["list"])));
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

        // Drive the edit assertions on a key with no validator so the committed
        // value isn't rejected by write validation (that path has its own test
        // below). `FLEETY_TZ` is a free-form (Shared-scope) timezone.
        app.sel = app
            .rows
            .iter()
            .position(|r| r.key == "FLEETY_TZ")
            .expect("FLEETY_TZ present");
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
        on_key(&mut app, KeyCode::Char('z'));
        assert!(on_key(&mut app, KeyCode::Enter));
        assert_eq!(
            app.map
                .get(&(setting.scope, key0.to_string()))
                .map(String::as_str),
            Some("z")
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
        // Select a validated local-scope key (FLEETY_AUTO_INSTALL_DEPS is Shared
        // and accepts only 0/1).
        app.sel = app
            .rows
            .iter()
            .position(|r| r.key == "FLEETY_AUTO_INSTALL_DEPS")
            .expect("FLEETY_AUTO_INSTALL_DEPS present");
        let setting = config::find("FLEETY_AUTO_INSTALL_DEPS").expect("known");

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
            !app
                .map
                .contains_key(&(setting.scope, "FLEETY_AUTO_INSTALL_DEPS".to_string())),
            "the rejected value must not enter the map"
        );
        assert!(
            app.status.contains("FLEETY_AUTO_INSTALL_DEPS"),
            "status: {}",
            app.status
        );
        assert!(
            app.status.contains('0') && app.status.contains('1'),
            "status should list the accepted 0/1 values, got: {}",
            app.status
        );
    }

    #[test]
    fn build_rows_is_local_scope_only() {
        let rows = build_rows(&ConfigMap::new());
        assert!(!rows.is_empty());
        for r in &rows {
            assert!(
                r.scope == "cli" || r.scope == "shared",
                "local edit rows are Cli/Shared only, got {} [{}]",
                r.key,
                r.scope
            );
        }
        assert!(rows.iter().any(|r| r.key == "FLEETY_TZ"), "a Shared key is present");
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
