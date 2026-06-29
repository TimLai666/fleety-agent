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
use fleety_tools::config::{self, ConfigMap, Source};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

/// Run a `config` subcommand. The dispatch (list/get/set/unset/help + the
/// line-based edit) is shared in `fleety_tools::config` so `fleety-server` and
/// `fleetyd` expose the same `config` command; the CLI only overrides `edit` to
/// open the ratatui screen when stdout is a TTY.
pub fn run(args: &[String]) -> Result<()> {
    if matches!(config::parse(args), config::Command::Edit) && std::io::stdout().is_terminal() {
        run_tui_edit(&config::config_path())
    } else {
        config::run(args)
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
    config::registry()
        .iter()
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
    /// `Some(buffer)` while editing the selected row's value.
    edit: Option<String>,
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
    if app.edit.is_some() {
        match key {
            KeyCode::Char(c) => {
                if let Some(b) = app.edit.as_mut() {
                    b.push(c);
                }
            }
            KeyCode::Backspace => {
                if let Some(b) = app.edit.as_mut() {
                    b.pop();
                }
            }
            KeyCode::Esc => {
                app.edit = None;
                app.status = "edit cancelled".to_string();
            }
            KeyCode::Enter => {
                let buf = app.edit.take().unwrap_or_default();
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
            // Edit the raw value (never the mask), so secrets edit cleanly.
            let raw = app
                .rows
                .get(app.sel)
                .map(|r| r.value.clone())
                .unwrap_or_default();
            app.edit = Some(raw);
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
    let mut lines: Vec<Line> = Vec::with_capacity(app.rows.len());
    for (i, r) in app.rows.iter().enumerate() {
        let editing_this = i == app.sel && app.edit.is_some();
        let shown = if editing_this {
            app.edit.clone().unwrap_or_default()
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
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title("fleety config — settings")),
        chunks[0],
    );
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
    fn config_tui_key_handling() {
        let mut app = ConfigApp::new(ConfigMap::new());
        assert!(app.rows.len() >= 2);
        let key0 = app.rows[0].key;
        let setting = config::find(key0).expect("known");

        // Navigation.
        on_key(&mut app, KeyCode::Down);
        assert_eq!(app.sel, 1);
        on_key(&mut app, KeyCode::Up);
        assert_eq!(app.sel, 0);

        // Edit then cancel → no change, no save.
        on_key(&mut app, KeyCode::Enter);
        assert!(app.edit.is_some());
        on_key(&mut app, KeyCode::Esc);
        assert!(app.edit.is_none());
        assert!(!app.map.contains_key(&(setting.scope, key0.to_string())));

        // Edit then commit a value → saved + map updated.
        on_key(&mut app, KeyCode::Enter);
        app.edit = Some(String::new()); // clear the prefilled default for a clean assert
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
        app.edit = Some(String::new());
        assert!(on_key(&mut app, KeyCode::Enter));
        assert!(!app.map.contains_key(&(setting.scope, key0.to_string())));

        // Quit.
        on_key(&mut app, KeyCode::Char('q'));
        assert!(app.quit);
    }
}
