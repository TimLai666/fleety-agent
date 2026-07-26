//! Owner-aware Settings content for the shared terminal workspace.
//!
//! Five Tab-switchable pages expose **Connection**, **CLI**, **Daemon**,
//! **Server**, and **Providers & Models**. Every value edit is staged first.
//! Apply acts on exactly one owner: CLI through the in-process CLI config
//! service, Daemon and Server through `ConfigApply`, and Provider/model changes
//! through the connected Server's structured Provider workflow. No remote
//! failure falls back to a local file.

use std::collections::BTreeMap;

use agent_core::{CoreError, Result};
use fleety_protocol::{ChangeOp, ClientMsg, ConfigChange, ConfigEntry, ConfigTarget, ServerMsg};
use fleety_tools::connection::{self, Connections};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use crate::input::LineEditor;

/// Which owner-aware Settings page has focus.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Connection,
    Cli,
    Daemon,
    Server,
    ProvidersAndModels,
}

impl Region {
    fn next(self) -> Self {
        match self {
            Region::Connection => Region::Cli,
            Region::Cli => Region::Daemon,
            Region::Daemon => Region::Server,
            Region::Server => Region::ProvidersAndModels,
            Region::ProvidersAndModels => Region::Connection,
        }
    }
    fn title(self) -> &'static str {
        match self {
            Region::Connection => "[1] Connection",
            Region::Cli => "[2] CLI",
            Region::Daemon => "[3] Daemon",
            Region::Server => "[4] Server",
            Region::ProvidersAndModels => "[5] Providers & Models",
        }
    }

    fn settings_page(self) -> crate::workspace::SettingsPage {
        match self {
            Self::Connection => crate::workspace::SettingsPage::Connection,
            Self::Cli => crate::workspace::SettingsPage::Cli,
            Self::Daemon => crate::workspace::SettingsPage::Daemon,
            Self::Server => crate::workspace::SettingsPage::Server,
            Self::ProvidersAndModels => crate::workspace::SettingsPage::ProvidersAndModels,
        }
    }
}

/// Keys whose overwrite could redirect data/credentials off-box — the panel
/// double-confirms before staging a change (mirrors the server-side audit).
fn is_sensitive(key: &str) -> bool {
    matches!(
        key,
        "FLEETY_MODEL_KEY" | "FLEETY_MODEL_BASE_URL" | "FLEETY_BACKUP_REPO" | "FLEETY_BACKUP_TOKEN"
    )
}

/// The first, always-offered pick in the timezone list: clear `FLEETY_TZ` so the
/// runtime follows the host device's own zone (then UTC). Selecting it commits an
/// empty value through the normal path.
const TZ_DEVICE_LABEL: &str = "(device — follow this machine)";

/// Candidate timezones matching `needle` (case-insensitive substring over the
/// IANA `TZ_VARIANTS`), with the device-default option first when it matches.
/// Pure — unit-tested; the picker renders and commits from this list.
fn tz_candidates(needle: &str) -> Vec<&'static str> {
    let n = needle.trim().to_ascii_lowercase();
    let mut out: Vec<&'static str> = Vec::new();
    if n.is_empty() || "device".contains(n.as_str()) {
        out.push(TZ_DEVICE_LABEL);
    }
    for tz in chrono_tz::TZ_VARIANTS {
        let name = tz.name();
        if n.is_empty() || name.to_ascii_lowercase().contains(n.as_str()) {
            out.push(name);
        }
    }
    out
}

/// Searchable IANA-timezone picker shown when editing `FLEETY_TZ`, so the user
/// selects a zone instead of free-typing one. Commits through the same
/// [`Panel::commit_edit`] path as any other value (device option → empty).
struct TzPicker {
    filter: LineEditor,
    sel: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileSwitchResolution {
    Apply,
    Discard,
    Cancel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProfileSwitchRetryAction {
    PassThrough,
    Waiting,
    Retry,
    Cancelled,
}

struct ProfileSwitchPrompt {
    old_profile: String,
    new_profile: String,
    selected: usize,
}

/// The panel state. `on_key` is pure (no I/O) so it is unit-testable; the run
/// loop owns the terminal + the connection.
struct Panel {
    region: Region,
    sel: usize,
    /// `Some(editor)` while editing the selected row's value; the bool marks a
    /// pending sensitive-key confirmation before the edit is applied.
    edit: Option<LineEditor>,
    /// `Some` while the `FLEETY_TZ` timezone picker is open (mutually exclusive
    /// with `edit`).
    tz_pick: Option<TzPicker>,
    confirm_sensitive: Option<String>,
    // Connection region.
    conns: Connections,
    /// Last persisted snapshot used to compute precise URL mutations without
    /// writing back unrelated token/fingerprint/current fields.
    persisted_conns: Connections,
    /// Runtime identity of the transport backing Settings. An invocation
    /// override may differ from the persisted `conns.current` selection.
    active_profile: String,
    active_endpoint: Option<String>,
    /// Profile selected with `u` and awaiting a successful Connection-region
    /// save before the run loop is allowed to replace the live transport.
    profile_switch_pending: Option<String>,
    profile_switch_discard_pending: bool,
    profile_switch_retry_required: bool,
    profile_switch_prompt: Option<ProfileSwitchPrompt>,
    profile_switch_apply_now: bool,
    // CLI region.
    local: Vec<(String, String, String, String)>,
    local_map: fleety_tools::config::ConfigMap,
    local_dirty: bool,
    apply_cli_now: bool,
    local_apply_error: Option<crate::workspace::WorkspaceError>,
    // Daemon region.
    daemon_supported: bool,
    daemon_device_id: String,
    daemon_entries: Vec<ConfigEntry>,
    daemon_revision: String,
    daemon_staged: BTreeMap<String, ConfigChange>,
    apply_daemon_now: bool,
    daemon_apply_error: Option<crate::workspace::WorkspaceError>,
    daemon_refresh_required: bool,
    // Server region.
    server_supported: bool,
    entries: Vec<ConfigEntry>,
    revision: String,
    /// Staged, not-yet-applied server changes (tri-state), keyed by setting.
    staged: BTreeMap<String, ConfigChange>,
    apply_now: bool,
    server_apply_error: Option<crate::workspace::WorkspaceError>,
    server_refresh_required: bool,
    open_provider_now: bool,
    provider_error: Option<crate::workspace::WorkspaceError>,
    status: String,
    quit: bool,
}

struct RemoteRegionState {
    supported: bool,
    entries: Vec<ConfigEntry>,
    revision: String,
}

#[derive(Debug)]
struct OwnerApplySuccess {
    message: String,
    effect: Option<fleety_protocol::Effect>,
}

enum OwnerApplyRefresh {
    Refreshed {
        success: OwnerApplySuccess,
        revision: String,
        entries: Vec<ConfigEntry>,
    },
    RefreshRequired {
        success: OwnerApplySuccess,
        reason: String,
    },
}

impl OwnerApplySuccess {
    fn display(self) -> String {
        let timing = match self.effect {
            Some(fleety_protocol::Effect::Restart) => " · restart required",
            Some(fleety_protocol::Effect::NextConnection) => {
                " · takes effect on the next connection"
            }
            None => "",
        };
        format!("{}{}", self.message, timing)
    }
}

impl RemoteRegionState {
    fn new(supported: bool, entries: Vec<ConfigEntry>, revision: String) -> Self {
        Self {
            supported,
            entries,
            revision,
        }
    }
}

impl Panel {
    fn new(
        conns: Connections,
        local_map: fleety_tools::config::ConfigMap,
        daemon: RemoteRegionState,
        server: RemoteRegionState,
    ) -> Self {
        let local =
            fleety_tools::config::rows_in_scopes(&local_map, fleety_tools::config::LOCAL_SCOPES);
        let active_profile = conns.current.clone().unwrap_or_else(|| "default".into());
        let active_endpoint = conns
            .current
            .as_ref()
            .and_then(|name| conns.profiles.get(name))
            .map(|profile| profile.url.clone());
        Self {
            region: Region::Connection,
            sel: 0,
            edit: None,
            tz_pick: None,
            confirm_sensitive: None,
            persisted_conns: conns.clone(),
            conns,
            active_profile,
            active_endpoint,
            profile_switch_pending: None,
            profile_switch_discard_pending: false,
            profile_switch_retry_required: false,
            profile_switch_prompt: None,
            profile_switch_apply_now: false,
            local,
            local_map,
            local_dirty: false,
            apply_cli_now: false,
            local_apply_error: None,
            daemon_supported: daemon.supported,
            daemon_device_id: crate::device_id(),
            daemon_entries: daemon.entries,
            daemon_revision: daemon.revision,
            daemon_staged: BTreeMap::new(),
            apply_daemon_now: false,
            daemon_apply_error: None,
            daemon_refresh_required: false,
            server_supported: server.supported,
            entries: server.entries,
            revision: server.revision,
            staged: BTreeMap::new(),
            apply_now: false,
            server_apply_error: None,
            server_refresh_required: false,
            open_provider_now: false,
            provider_error: None,
            // The key hints live on their own persistent footer line now; the
            // status line starts empty so it doesn't duplicate them.
            status: String::new(),
            quit: false,
        }
    }

    fn activate_target(&mut self, target: &connection::Resolved) {
        self.active_profile = crate::workspace_profile_label(target);
        self.active_endpoint = Some(target.url_owned());
    }

    /// Invalidate every piece of state tied to the previously connected
    /// server before attempting a saved profile switch. The run loop owns and
    /// closes the transport itself; this method makes both remote regions
    /// unusable immediately so no old revision or staged change can leak into
    /// the replacement connection.
    fn invalidate_remote_for_reconnect(&mut self, profile: &str, discarded: bool) {
        self.active_profile = profile.to_string();
        self.active_endpoint = self
            .conns
            .profiles
            .get(profile)
            .map(|entry| entry.url.clone());
        self.daemon_supported = false;
        self.daemon_entries.clear();
        self.daemon_revision.clear();
        self.daemon_staged.clear();
        self.apply_daemon_now = false;
        self.daemon_apply_error = None;
        self.daemon_refresh_required = false;

        self.server_supported = false;
        self.entries.clear();
        self.revision.clear();
        self.staged.clear();
        self.apply_now = false;
        self.server_apply_error = None;
        self.server_refresh_required = false;
        self.provider_error = None;

        self.confirm_sensitive = None;
        self.status = if discarded {
            format!(
                "connecting to '{profile}' (discarded staged remote changes from the previous profile)…"
            )
        } else {
            format!("connecting to '{profile}'…")
        };
    }

    fn queue_profile_switch(&mut self, profile: &str, discard_after_persist: bool) {
        self.profile_switch_pending = Some(profile.to_string());
        self.profile_switch_discard_pending = discard_after_persist;
        self.profile_switch_retry_required = false;
        self.profile_switch_prompt = None;
        self.profile_switch_apply_now = false;
        self.status = format!("saving profile selection '{profile}'…");
    }

    fn request_profile_switch(&mut self, profile: String) {
        let old_profile = self.active_profile.clone();
        if profile == old_profile {
            self.status = format!("'{profile}' is already selected");
            return;
        }
        if !self.daemon_staged.is_empty() || !self.staged.is_empty() {
            self.profile_switch_prompt = Some(ProfileSwitchPrompt {
                old_profile,
                new_profile: profile,
                selected: 0,
            });
            self.status = "resolve staged remote changes before switching profile".into();
        } else {
            self.queue_profile_switch(&profile, false);
        }
    }

    fn resolve_profile_switch(&mut self, resolution: ProfileSwitchResolution) {
        let Some(new_profile) = self
            .profile_switch_prompt
            .as_ref()
            .map(|prompt| prompt.new_profile.clone())
        else {
            return;
        };
        match resolution {
            ProfileSwitchResolution::Apply => {
                self.profile_switch_apply_now = true;
                self.status =
                    format!("applying staged remote changes before switching to '{new_profile}'…");
            }
            ProfileSwitchResolution::Discard => {
                self.queue_profile_switch(&new_profile, true);
            }
            ProfileSwitchResolution::Cancel => {
                self.profile_switch_prompt = None;
                self.profile_switch_apply_now = false;
                self.profile_switch_discard_pending = false;
                self.profile_switch_retry_required = false;
                self.status = "profile switch cancelled; staged changes retained".into();
            }
        }
    }

    fn resolve_profile_switch_retry(&mut self, key: KeyEvent) -> ProfileSwitchRetryAction {
        if !self.profile_switch_retry_required {
            return ProfileSwitchRetryAction::PassThrough;
        }
        if matches!(
            key.code,
            KeyCode::Char(character) if character.eq_ignore_ascii_case(&'c')
        ) && key.modifiers.contains(KeyModifiers::CONTROL)
        {
            return ProfileSwitchRetryAction::PassThrough;
        }
        if !key.modifiers.is_empty() {
            self.status =
                "profile selection remains pending; press r/Enter to retry or Esc/q to cancel"
                    .into();
            return ProfileSwitchRetryAction::Waiting;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.profile_switch_pending = None;
                self.profile_switch_discard_pending = false;
                self.profile_switch_retry_required = false;
                self.status = "profile switch cancelled; applied owner settings retained".into();
                ProfileSwitchRetryAction::Cancelled
            }
            KeyCode::Enter | KeyCode::Char('r') => {
                self.profile_switch_retry_required = false;
                self.status = "retrying profile selection…".into();
                ProfileSwitchRetryAction::Retry
            }
            _ => {
                self.status =
                    "profile selection remains pending; press r/Enter to retry or Esc/q to cancel"
                        .into();
                ProfileSwitchRetryAction::Waiting
            }
        }
    }

    /// The number of rows in the active region.
    fn rows_len(&self) -> usize {
        match self.region {
            Region::Connection => self.conns.profiles.len(),
            Region::Cli => self.local.len(),
            Region::Daemon => self.daemon_entries.len(),
            Region::Server => self.entries.len(),
            Region::ProvidersAndModels => 1,
        }
    }

    /// The connection profile names in display order.
    fn profile_names(&self) -> Vec<String> {
        self.conns.profiles.keys().cloned().collect()
    }

    /// The setting key of the selected row in an editable region (Local/Server),
    /// used to route `FLEETY_TZ` to the timezone picker. `None` for Connection.
    fn selected_key(&self) -> Option<String> {
        match self.region {
            Region::Cli => self.local.get(self.sel).map(|r| r.0.clone()),
            Region::Daemon => self.daemon_entries.get(self.sel).map(|e| e.key.clone()),
            Region::Server => self.entries.get(self.sel).map(|e| e.key.clone()),
            Region::Connection | Region::ProvidersAndModels => None,
        }
    }

    /// Begin editing the selected server entry (staging its current value),
    /// double-confirming a sensitive key first.
    fn begin_remote_edit(&mut self) {
        if self.remote_refresh_required(self.region) {
            self.status = Self::refresh_required_message(self.region);
            return;
        }
        let entry = match self.region {
            Region::Daemon => self.daemon_entries.get(self.sel),
            Region::Server => self.entries.get(self.sel),
            _ => None,
        };
        let Some(entry) = entry else {
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

    fn remote_refresh_required(&self, region: Region) -> bool {
        match region {
            Region::Daemon => self.daemon_refresh_required,
            Region::Server => self.server_refresh_required,
            _ => false,
        }
    }

    fn refresh_required_message(region: Region) -> String {
        format!(
            "{} changes were applied, but refresh failed — exit and reopen Settings before editing or applying again",
            if region == Region::Daemon { "Daemon" } else { "Server" }
        )
    }

    fn finish_remote_apply(
        &mut self,
        region: Region,
        outcome: std::result::Result<OwnerApplyRefresh, crate::provider_service::ProviderIssue>,
    ) {
        match outcome {
            Ok(OwnerApplyRefresh::Refreshed {
                success,
                revision,
                entries,
            }) => {
                if region == Region::Daemon {
                    self.daemon_staged.clear();
                    self.daemon_revision = revision;
                    self.daemon_entries = entries;
                    self.daemon_supported = true;
                    self.daemon_apply_error = None;
                    self.daemon_refresh_required = false;
                } else {
                    self.staged.clear();
                    self.revision = revision;
                    self.entries = entries;
                    self.server_supported = true;
                    self.server_apply_error = None;
                    self.server_refresh_required = false;
                }
                self.status = success.display();
            }
            Ok(OwnerApplyRefresh::RefreshRequired { success, reason }) => {
                let message = format!(
                    "{}; {}: {}",
                    success.display(),
                    Self::refresh_required_message(region),
                    reason
                );
                let error = crate::workspace::WorkspaceError {
                    kind: "refresh_required".into(),
                    message: message.clone(),
                    remediation: Some("Exit and reopen Settings to load a fresh snapshot".into()),
                };
                if region == Region::Daemon {
                    self.daemon_staged.clear();
                    self.daemon_entries.clear();
                    self.daemon_revision.clear();
                    self.daemon_supported = false;
                    self.daemon_apply_error = Some(error);
                    self.daemon_refresh_required = true;
                    self.apply_daemon_now = false;
                } else {
                    self.staged.clear();
                    self.entries.clear();
                    self.revision.clear();
                    self.server_supported = false;
                    self.server_apply_error = Some(error);
                    self.server_refresh_required = true;
                    self.apply_now = false;
                }
                self.status = message;
            }
            Err(error) => {
                let workspace_error = crate::workspace::WorkspaceError {
                    kind: error.kind.clone(),
                    message: error.message.clone(),
                    remediation: error.remediation.clone(),
                };
                if region == Region::Daemon {
                    self.daemon_apply_error = Some(workspace_error);
                    self.status = format!("daemon apply failed: {}", error.display());
                } else {
                    self.server_apply_error = Some(workspace_error);
                    self.status = format!("apply failed: {}", error.display());
                }
            }
        }
    }

    /// Commit the in-progress edit for the active region. Returns `true` when a
    /// local file was written (the run loop persists connection/local edits).
    fn commit_edit(&mut self, value: String) -> bool {
        if self.remote_refresh_required(self.region) {
            self.status = Self::refresh_required_message(self.region);
            return false;
        }
        let value = value.trim().to_string();
        match self.region {
            Region::Connection => {
                // Edit = set the selected profile's url.
                if let Err(error) = connection::validate_ws_url(&value) {
                    self.status = format!("error: {}", error.report().message);
                    return false;
                }
                if let Some(name) = self.profile_names().get(self.sel).cloned() {
                    if let Some(p) = self.conns.profiles.get_mut(&name) {
                        let cleared = connection::reselect_profile_endpoint(p, value);
                        self.status = if cleared {
                            format!(
                                "set url for '{name}' and cleared its old credential (s to save, then re-pair)"
                            )
                        } else {
                            format!("set url for '{name}' (s to save)")
                        };
                        return false;
                    }
                }
                false
            }
            Region::Cli => {
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
                        self.local_dirty = true;
                        self.local_apply_error = None;
                        self.status = format!("staged {key} for the CLI owner — press a to apply");
                        return false;
                    }
                }
                false
            }
            Region::Daemon | Region::Server => {
                let entry = match self.region {
                    Region::Daemon => self.daemon_entries.get(self.sel).cloned(),
                    Region::Server => self.entries.get(self.sel).cloned(),
                    _ => None,
                };
                if let Some(entry) = entry {
                    // Tri-state: empty → clear, else set. (keep = untouched.)
                    let op = if value.is_empty() {
                        ChangeOp::Clear
                    } else {
                        ChangeOp::Set
                    };
                    let staged = if self.region == Region::Daemon {
                        self.daemon_apply_error = None;
                        &mut self.daemon_staged
                    } else {
                        self.server_apply_error = None;
                        &mut self.staged
                    };
                    staged.insert(
                        entry.key.clone(),
                        ConfigChange {
                            key: entry.key.clone(),
                            op,
                            value: if value.is_empty() { None } else { Some(value) },
                        },
                    );
                    self.status = format!(
                        "staged '{}' — press a to apply to the {}",
                        entry.key,
                        if self.region == Region::Daemon {
                            "daemon"
                        } else {
                            "server"
                        }
                    );
                }
                false
            }
            Region::ProvidersAndModels => false,
        }
    }
}

/// Handle a key while the `FLEETY_TZ` timezone picker is open. Commits through
/// [`Panel::commit_edit`] (device option → empty value; a highlighted zone → its
/// name; no candidates → the typed text), so validation + persistence match
/// every other edit. Returns `true` when a local file must be persisted. Pure.
fn on_key_tz_pick(p: &mut Panel, code: KeyCode) -> bool {
    let Some(pick) = p.tz_pick.as_mut() else {
        return false;
    };
    match code {
        KeyCode::Char(c) => {
            pick.filter.insert(c);
            pick.sel = 0;
        }
        KeyCode::Backspace => {
            pick.filter.backspace();
            pick.sel = 0;
        }
        KeyCode::Left => pick.filter.left(),
        KeyCode::Right => pick.filter.right(),
        KeyCode::Up => pick.sel = pick.sel.saturating_sub(1),
        KeyCode::Down => {
            let max = tz_candidates(pick.filter.text()).len().saturating_sub(1);
            pick.sel = (pick.sel + 1).min(max);
        }
        KeyCode::Esc => {
            p.tz_pick = None;
            p.status = "edit cancelled".to_string();
        }
        KeyCode::Enter => {
            let cands = tz_candidates(pick.filter.text());
            let picked = cands.get(pick.sel).copied();
            let raw = pick.filter.text().trim().to_string();
            p.tz_pick = None;
            let value = match picked {
                Some(name) if name == TZ_DEVICE_LABEL => String::new(),
                Some(name) => name.to_string(),
                None => raw,
            };
            return p.commit_edit(value);
        }
        _ => {}
    }
    false
}

/// One key event. Returns `true` when the connection/local file should be
/// persisted by the run loop. Pure (no I/O).
fn on_key(p: &mut Panel, code: KeyCode) -> bool {
    if p.profile_switch_prompt.is_some() {
        let mut resolution = None;
        if let Some(prompt) = p.profile_switch_prompt.as_mut() {
            match code {
                KeyCode::Left | KeyCode::Up => prompt.selected = prompt.selected.saturating_sub(1),
                KeyCode::Right | KeyCode::Down => prompt.selected = (prompt.selected + 1).min(2),
                KeyCode::Char('a' | 'A') => resolution = Some(ProfileSwitchResolution::Apply),
                KeyCode::Char('d' | 'D') => resolution = Some(ProfileSwitchResolution::Discard),
                KeyCode::Char('c' | 'C') | KeyCode::Esc => {
                    resolution = Some(ProfileSwitchResolution::Cancel)
                }
                KeyCode::Enter => {
                    resolution = Some(match prompt.selected {
                        0 => ProfileSwitchResolution::Apply,
                        1 => ProfileSwitchResolution::Discard,
                        _ => ProfileSwitchResolution::Cancel,
                    })
                }
                _ => {}
            }
        }
        if let Some(resolution) = resolution {
            p.resolve_profile_switch(resolution);
        }
        return false;
    }
    if p.remote_refresh_required(p.region) && matches!(code, KeyCode::Enter | KeyCode::Char('a')) {
        p.status = Panel::refresh_required_message(p.region);
        return false;
    }
    if p.tz_pick.is_some() {
        return on_key_tz_pick(p, code);
    }
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
        // q or Esc (when not editing) leaves the settings editor back to the menu.
        KeyCode::Char('q') | KeyCode::Esc => p.quit = true,
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
                p.request_profile_switch(name);
            }
        }
        KeyCode::Char('s') if p.region == Region::Connection => {
            // Persist connections.toml (handled by the run loop via return? no —
            // save here is I/O; the run loop saves on this flag). Signal via
            // status; the run loop checks and saves.
            p.status = "__save_conns__".to_string();
        }
        KeyCode::Char('a') if matches!(p.region, Region::Daemon | Region::Server) => {
            let (supported, staged) = if p.region == Region::Daemon {
                (p.daemon_supported, !p.daemon_staged.is_empty())
            } else {
                (p.server_supported, !p.staged.is_empty())
            };
            if supported && staged {
                if p.region == Region::Daemon {
                    p.apply_daemon_now = true;
                } else {
                    p.apply_now = true;
                }
            } else if !supported {
                p.status = format!(
                    "{} is unavailable; no local file fallback will be used",
                    if p.region == Region::Daemon {
                        "daemon"
                    } else {
                        "server"
                    }
                );
            } else {
                p.status = "nothing staged".to_string();
            }
        }
        KeyCode::Char('a') if p.region == Region::Cli => {
            if p.local_dirty {
                p.apply_cli_now = true;
            } else {
                p.status = "nothing staged for the CLI owner".to_string();
            }
        }
        KeyCode::Enter if p.selected_key().as_deref() == Some("FLEETY_TZ") => {
            // Guided timezone choice instead of free-typing an IANA name.
            p.tz_pick = Some(TzPicker {
                filter: LineEditor::default(),
                sel: 0,
            });
        }
        KeyCode::Enter => match p.region {
            Region::Daemon | Region::Server => p.begin_remote_edit(),
            Region::ProvidersAndModels => p.open_provider_now = true,
            _ => {
                let mut ed = LineEditor::default();
                let prefill = match p.region {
                    Region::Connection => p
                        .profile_names()
                        .get(p.sel)
                        .and_then(|n| p.conns.profiles.get(n))
                        .map(|pr| pr.url.clone())
                        .unwrap_or_default(),
                    Region::Cli => p.local.get(p.sel).map(|r| r.2.clone()).unwrap_or_default(),
                    Region::Daemon | Region::Server => String::new(),
                    Region::ProvidersAndModels => String::new(),
                };
                ed.set_text(prefill);
                p.edit = Some(ed);
            }
        },
        _ => {}
    }
    false
}

fn render_in_area(f: &mut Frame, p: &Panel, area: ratatui::layout::Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(4),
        ])
        .split(area);

    // Region tab bar.
    let tabs = [
        Region::Connection,
        Region::Cli,
        Region::Daemon,
        Region::Server,
        Region::ProvidersAndModels,
    ]
    .iter()
    .map(|r| {
        let state = region_state_label(p, *r);
        let dirty = if state != "available" {
            format!(" [{state}]")
        } else {
            String::new()
        };
        if *r == p.region {
            format!("▸{}{dirty}", r.title())
        } else {
            format!(" {}{dirty}", r.title())
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
                let cur = if p.active_profile == *name { "*" } else { " " };
                let url = p
                    .conns
                    .profiles
                    .get(name)
                    .map(connection_endpoint_label)
                    .unwrap_or_default();
                let name = crate::terminal_safe_text(name);
                lines.push(Line::from(format!("{marker}{cur} {name:<14} {url}")));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(
                "(u: switch current · Enter: edit url · s: save · 'local' needs no pairing)",
            ));
        }
        Region::Cli => {
            for (i, (key, scope, value, source)) in p.local.iter().enumerate() {
                let marker = if i == p.sel { "▶" } else { " " };
                let key = crate::terminal_safe_text(key);
                let scope = crate::terminal_safe_text(scope);
                let value = crate::terminal_safe_text(value);
                let source = crate::terminal_safe_text(source);
                lines.push(Line::from(format!(
                    "{marker} [{scope:6}] {key:<24} = {value}  ({source})"
                )));
            }
        }
        Region::Daemon | Region::Server => {
            let (supported, entries, staged, owner) = if p.region == Region::Daemon {
                (
                    p.daemon_supported,
                    &p.daemon_entries,
                    &p.daemon_staged,
                    "daemon",
                )
            } else {
                (p.server_supported, &p.entries, &p.staged, "server")
            };
            if !supported {
                lines.push(Line::from(format!(
                    "The {owner} is unavailable for structured config."
                )));
                lines.push(Line::from("No local config file fallback will be used."));
            } else {
                for (i, e) in entries.iter().enumerate() {
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
                    let staged = if staged.contains_key(&e.key) {
                        " *staged"
                    } else {
                        ""
                    };
                    let scope = crate::terminal_safe_text(&e.scope);
                    let key = crate::terminal_safe_text(&e.key);
                    let shown = crate::terminal_safe_text(shown);
                    lines.push(Line::from(format!(
                        "{marker} [{scope:6}] {key:<24} = {shown}{staged}"
                    )));
                }
            }
        }
        Region::ProvidersAndModels => {
            let profile = crate::terminal_safe_text(&p.active_profile);
            let endpoint = p
                .active_endpoint
                .as_deref()
                .map(crate::terminal_safe_endpoint)
                .unwrap_or_else(|| "unavailable".into());
            lines.push(Line::from(format!(
                "Providers & Models · profile {profile} · Server {endpoint}"
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Enter opens the connected Server's Provider and model workflow.",
            ));
            lines.push(Line::from(
                "Authentication, catalogs, and roles remain owned by that Server.",
            ));
        }
    }
    let profile = crate::terminal_safe_text(&p.active_profile);
    let mut body_title = format!(
        "{} · {profile} · {}",
        p.region.title().trim_start_matches(|character: char| {
            character == '[' || character == ']' || character.is_ascii_digit() || character == ' '
        }),
        region_state_label(p, p.region)
    );
    if p.region == Region::Daemon {
        body_title.push_str(" · device ");
        body_title.push_str(&crate::terminal_safe_text(&p.daemon_device_id));
    }
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(body_title)),
        chunks[1],
    );

    // Footer: a PERSISTENT key-hint line (so an action's output never hides how
    // to move / apply / leave), then the transient edit buffer or status below.
    let hints = "Tab: page · ↑↓: move · Enter: edit/open · a: apply owner · Esc/q: back";
    let status_line = if let Some(pick) = &p.tz_pick {
        let cands = tz_candidates(pick.filter.text());
        let n = cands.len();
        let hl = cands.get(pick.sel).copied().unwrap_or("(type a zone name)");
        let idx = if n == 0 { 0 } else { pick.sel + 1 };
        format!(
            "tz> {}   [{idx}/{n}] {hl}   (↑↓ pick · Enter set · Esc cancel)",
            pick.filter.text()
        )
    } else if let Some(ed) = &p.edit {
        let (view, _) = ed.display_window(60);
        format!(
            "> {}   (Enter save · Esc cancel)",
            crate::terminal_safe_text(view)
        )
    } else if let Some(error) = active_owner_error(p) {
        match &error.remediation {
            Some(remediation) => {
                crate::terminal_safe_text(&format!("{} · {remediation}", error.message))
            }
            None => crate::terminal_safe_text(&error.message),
        }
    } else if matches!(p.region, Region::Daemon | Region::Server) {
        let entries = if p.region == Region::Daemon {
            &p.daemon_entries
        } else {
            &p.entries
        };
        match entries.get(p.sel) {
            Some(e) => {
                let eff = match e.effect {
                    Some(fleety_protocol::Effect::Restart) => "restart",
                    Some(fleety_protocol::Effect::NextConnection) => "next connection",
                    None => "-",
                };
                // Show the selected key's effect + description; prefix the action
                // status only when there is one (so an empty status leaves no gap).
                if p.status.is_empty() {
                    format!(
                        "(effect: {eff})  · {}",
                        crate::terminal_safe_text(&e.description)
                    )
                } else {
                    format!(
                        "{}  ·  (effect: {eff})  · {}",
                        crate::terminal_safe_text(&p.status),
                        crate::terminal_safe_text(&e.description)
                    )
                }
            }
            None => crate::terminal_safe_text(&p.status),
        }
    } else {
        crate::terminal_safe_text(&p.status)
    };
    let footer = vec![Line::from(hints), Line::from(status_line)];
    f.render_widget(Paragraph::new(footer).block(Block::bordered()), chunks[2]);
}

fn connection_endpoint_label(profile: &connection::Profile) -> String {
    if profile.url.is_empty() {
        "(endpoint required — run `fleety init`)".to_string()
    } else {
        crate::terminal_safe_endpoint(&profile.url)
    }
}

fn active_owner_error(panel: &Panel) -> Option<&crate::workspace::WorkspaceError> {
    match panel.region {
        Region::Cli => panel.local_apply_error.as_ref(),
        Region::Daemon => panel.daemon_apply_error.as_ref(),
        Region::Server => panel.server_apply_error.as_ref(),
        Region::ProvidersAndModels => panel.provider_error.as_ref(),
        Region::Connection => None,
    }
}

fn region_state_label(panel: &Panel, region: Region) -> &'static str {
    match region {
        Region::Connection => "available",
        Region::Cli if panel.apply_cli_now => "applying",
        Region::Cli if panel.local_apply_error.is_some() => "failed",
        Region::Cli if panel.local_dirty => "dirty",
        Region::Cli => "available",
        Region::Daemon if panel.daemon_refresh_required => "reload required",
        Region::Daemon if !panel.daemon_supported => "unavailable",
        Region::Daemon
            if panel.apply_daemon_now
                || (panel.profile_switch_apply_now && !panel.daemon_staged.is_empty()) =>
        {
            "applying"
        }
        Region::Daemon
            if panel
                .daemon_apply_error
                .as_ref()
                .is_some_and(|error| error.kind == "conflict") =>
        {
            "conflict"
        }
        Region::Daemon if panel.daemon_apply_error.is_some() => "failed",
        Region::Daemon if !panel.daemon_staged.is_empty() => "dirty",
        Region::Daemon => "available",
        Region::Server if panel.server_refresh_required => "reload required",
        Region::Server if !panel.server_supported => "unavailable",
        Region::Server
            if panel.apply_now || (panel.profile_switch_apply_now && !panel.staged.is_empty()) =>
        {
            "applying"
        }
        Region::Server
            if panel
                .server_apply_error
                .as_ref()
                .is_some_and(|error| error.kind == "conflict") =>
        {
            "conflict"
        }
        Region::Server if panel.server_apply_error.is_some() => "failed",
        Region::Server if !panel.staged.is_empty() => "dirty",
        Region::Server => "available",
        Region::ProvidersAndModels if !panel.server_supported => "unavailable",
        Region::ProvidersAndModels if panel.open_provider_now => "applying",
        Region::ProvidersAndModels if panel.provider_error.is_some() => "failed",
        Region::ProvidersAndModels => "available",
    }
}

fn sync_workspace_from_panel(workspace: &mut crate::workspace::WorkspaceState, panel: &Panel) {
    match (&panel.profile_switch_prompt, &workspace.route) {
        (Some(prompt), crate::workspace::Route::Settings(_)) => {
            workspace.reduce(crate::workspace::Action::Navigate(
                crate::workspace::Route::Modal(
                    crate::workspace::Modal::ResolveDirtyProfileSwitch {
                        old_profile: prompt.old_profile.clone(),
                        new_profile: prompt.new_profile.clone(),
                    },
                ),
            ));
        }
        (
            None,
            crate::workspace::Route::Modal(crate::workspace::Modal::ResolveDirtyProfileSwitch {
                ..
            }),
        ) => {
            workspace.reduce(crate::workspace::Action::Back);
        }
        _ => {}
    }
    if matches!(workspace.route, crate::workspace::Route::Settings(_)) {
        workspace.route = crate::workspace::Route::Settings(panel.region.settings_page());
    }
    workspace.context.profile = panel.conns.current.clone();
    workspace.context.endpoint = panel
        .conns
        .current
        .as_ref()
        .and_then(|name| panel.conns.profiles.get(name))
        .map(|profile| profile.url.clone());

    let cli_snapshot = owner_snapshot_from_pairs(
        "cli",
        panel
            .local
            .iter()
            .map(|(key, _, value, _)| (key.clone(), value.clone())),
    );
    workspace.owners.insert(
        crate::workspace::Owner::Cli,
        owner_state(
            true,
            panel.local_dirty,
            panel.apply_cli_now,
            panel.local_apply_error.clone(),
            cli_snapshot,
        ),
    );

    let daemon_snapshot = owner_snapshot_from_pairs(
        panel.daemon_revision.clone(),
        panel
            .daemon_entries
            .iter()
            .map(|entry| (entry.key.clone(), entry.value.clone())),
    );
    workspace.owners.insert(
        crate::workspace::Owner::Daemon,
        owner_state(
            panel.daemon_supported,
            !panel.daemon_staged.is_empty(),
            panel.apply_daemon_now
                || (panel.profile_switch_apply_now && !panel.daemon_staged.is_empty()),
            panel.daemon_apply_error.clone(),
            daemon_snapshot,
        ),
    );

    let server_snapshot = owner_snapshot_from_pairs(
        panel.revision.clone(),
        panel
            .entries
            .iter()
            .map(|entry| (entry.key.clone(), entry.value.clone())),
    );
    workspace.owners.insert(
        crate::workspace::Owner::Server,
        owner_state(
            panel.server_supported,
            !panel.staged.is_empty(),
            panel.apply_now || (panel.profile_switch_apply_now && !panel.staged.is_empty()),
            panel.server_apply_error.clone(),
            server_snapshot.clone(),
        ),
    );
    workspace.owners.insert(
        crate::workspace::Owner::ProvidersAndModels,
        owner_state(
            panel.server_supported,
            false,
            panel.open_provider_now,
            panel.provider_error.clone(),
            server_snapshot,
        ),
    );
}

fn sync_active_target_context(
    workspace: &mut crate::workspace::WorkspaceState,
    target: Option<&connection::Resolved>,
    fingerprint: Option<&str>,
) {
    if let Some(target) = target {
        if workspace.context.endpoint.as_deref() != Some(target.url()) {
            workspace.context.provider = None;
            workspace.context.model = None;
            workspace.context.conversation_id = None;
        }
        workspace.context.profile = Some(crate::workspace_profile_label(target));
        workspace.context.endpoint = Some(target.url_owned());
        workspace.context.server_identity = fingerprint.map(str::to_string);
    } else {
        workspace.context.server_identity = None;
        workspace.context.server_version = None;
        workspace.context.provider = None;
        workspace.context.model = None;
        workspace.context.conversation_id = None;
    }
}

fn owner_snapshot_from_pairs(
    revision: impl Into<String>,
    values: impl IntoIterator<Item = (String, String)>,
) -> crate::workspace::OwnerSnapshot {
    crate::workspace::OwnerSnapshot {
        revision: revision.into(),
        values: values.into_iter().collect(),
    }
}

fn owner_state(
    supported: bool,
    dirty: bool,
    applying: bool,
    error: Option<crate::workspace::WorkspaceError>,
    snapshot: crate::workspace::OwnerSnapshot,
) -> crate::workspace::OwnerState<crate::workspace::OwnerSnapshot> {
    if !supported {
        return crate::workspace::OwnerState::Unavailable(error.unwrap_or_else(|| {
            crate::workspace::WorkspaceError {
                kind: "unavailable".into(),
                message: "This owner is unavailable".into(),
                remediation: Some("Reconnect to the selected Server".into()),
            }
        }));
    }
    if applying {
        return crate::workspace::OwnerState::Applying(snapshot);
    }
    if let Some(error) = error {
        if error.kind == "conflict" {
            return crate::workspace::OwnerState::Conflict(snapshot, error);
        }
        return crate::workspace::OwnerState::Failed(snapshot, error);
    }
    if dirty {
        crate::workspace::OwnerState::Dirty(snapshot)
    } else {
        crate::workspace::OwnerState::Available(snapshot)
    }
}

/// Open the interactive config panel: resolve + connect to the current server,
/// negotiate the structured-config capability, pull a snapshot if supported,
/// then run the four-region editor. Connection/CLI edits persist locally;
/// daemon and server edits are staged and applied via their owning runtimes.
/// Whether any saved profile already targets `local_url` (so the panel need not
/// inject a `local` entry). Pure.
fn has_local_profile(conns: &Connections, local_url: &str) -> bool {
    conns.profiles.values().any(|p| p.url == local_url)
}

/// Resolve exactly the profile the user saved in the Connection region. This
/// deliberately ignores environment overrides, mDNS, and later changes to the
/// global current profile so the reconnect transaction cannot drift to a
/// different server while awaiting network I/O.
fn resolve_saved_profile(conns: &Connections, name: &str) -> Result<connection::Resolved> {
    connection::resolve(
        conns,
        &connection::Target::Named(name.to_string()),
        None,
        None,
        || None,
    )
}

/// Close and remove the previous panel transport before connecting one
/// immutable profile snapshot. On failure `old` remains `None`, which prevents
/// any caller from silently falling back to the previous server.
async fn replace_profile_connection(
    old: &mut Option<(
        fleety_tools::transport::Sender,
        fleety_tools::transport::Receiver,
    )>,
    target: &connection::Resolved,
    connections_path: &std::path::Path,
) -> Result<(
    (
        fleety_tools::transport::Sender,
        fleety_tools::transport::Receiver,
    ),
    u32,
    Option<String>,
    connection::Resolved,
)> {
    if let Some((mut tx, _)) = old.take() {
        tx.close().await;
    }
    let (tx, rx, config_protocol, fingerprint, committed_target) =
        crate::connect_hello_for_profile_switch_target(target, connections_path).await?;
    Ok(((tx, rx), config_protocol, fingerprint, committed_target))
}

async fn apply_staged_before_profile_switch(
    panel: &mut Panel,
    connection: &mut Option<(
        fleety_tools::transport::Sender,
        fleety_tools::transport::Receiver,
    )>,
    config_protocol: u32,
) -> bool {
    panel.profile_switch_apply_now = false;
    let Some(new_profile) = panel
        .profile_switch_prompt
        .as_ref()
        .map(|prompt| prompt.new_profile.clone())
    else {
        return false;
    };
    if panel.daemon_refresh_required || panel.server_refresh_required {
        let owner = if panel.daemon_refresh_required {
            Region::Daemon
        } else {
            Region::Server
        };
        panel.status = format!(
            "profile switch paused: {}",
            Panel::refresh_required_message(owner)
        );
        return false;
    }
    let Some((tx, rx)) = connection.as_mut() else {
        panel.status = "cannot apply staged changes: the current Server is unavailable".into();
        return false;
    };
    if !panel.daemon_staged.is_empty() {
        let changes = panel.daemon_staged.values().cloned().collect();
        let outcome = apply_and_refresh(
            tx,
            rx,
            ConfigTarget::Device(panel.daemon_device_id.clone()),
            &panel.daemon_revision,
            changes,
            config_protocol,
        )
        .await;
        let refreshed = matches!(&outcome, Ok(OwnerApplyRefresh::Refreshed { .. }));
        panel.finish_remote_apply(Region::Daemon, outcome);
        if !refreshed {
            panel.status = format!("profile switch paused: {}", panel.status);
            return false;
        }
    }
    if !panel.staged.is_empty() {
        let changes = panel.staged.values().cloned().collect();
        let outcome = apply_and_refresh(
            tx,
            rx,
            ConfigTarget::Server,
            &panel.revision,
            changes,
            config_protocol,
        )
        .await;
        let refreshed = matches!(&outcome, Ok(OwnerApplyRefresh::Refreshed { .. }));
        panel.finish_remote_apply(Region::Server, outcome);
        if !refreshed {
            panel.status = format!("profile switch paused: {}", panel.status);
            return false;
        }
    }
    panel.queue_profile_switch(&new_profile, false);
    true
}

async fn commit_profile_switch(
    panel: &mut Panel,
    connection: &mut Option<(
        fleety_tools::transport::Sender,
        fleety_tools::transport::Receiver,
    )>,
    config_protocol: &mut Option<u32>,
    active_target: &mut Option<connection::Resolved>,
    active_fingerprint: &mut Option<String>,
) {
    commit_profile_switch_at(
        panel,
        connection,
        config_protocol,
        active_target,
        active_fingerprint,
        &connection::connections_path(),
        crate::server::notify_daemon_reconnect,
    )
    .await;
}

/// Commit one profile selection as a two-phase transaction. Persistence must
/// succeed before the old transport or any old remote snapshot is touched.
/// Taking the path explicitly keeps that safety boundary directly testable.
async fn commit_profile_switch_at(
    panel: &mut Panel,
    connection: &mut Option<(
        fleety_tools::transport::Sender,
        fleety_tools::transport::Receiver,
    )>,
    config_protocol: &mut Option<u32>,
    active_target: &mut Option<connection::Resolved>,
    active_fingerprint: &mut Option<String>,
    connections_path: &std::path::Path,
    notify_daemon: impl FnOnce(&str) -> Result<String>,
) {
    let Some(profile) = panel.profile_switch_pending.clone() else {
        return;
    };
    let old_persisted_profile = panel.persisted_conns.current.clone();
    let old_active_profile = panel.active_profile.clone();
    panel.conns.current = Some(profile.clone());
    let persisted = connection::mutate_at(connections_path, |live| {
        if live.current != old_persisted_profile {
            return Err(CoreError::Message(
                "the current profile changed in another Fleety process; reopen Settings before switching"
                    .to_string(),
            ));
        }
        // Resolve from the owner-current profile while the mutation lease is
        // held. Concurrent token/fingerprint rotation is legitimate and this
        // switch must reconnect with that live snapshot, not panel.conns.
        let target = resolve_saved_profile(live, &profile)?;
        live.current = Some(profile.clone());
        Ok((live.clone(), target))
    });
    let (persisted, target) = match persisted {
        Ok(result) => result,
        Err(error) => {
            panel.conns.current = old_persisted_profile;
            if panel.daemon_staged.is_empty() && panel.staged.is_empty() {
                panel.profile_switch_prompt = None;
                panel.profile_switch_retry_required = true;
                panel.status = format!(
                    "profile selection was not saved: {} — profile selection remains pending; press r/Enter to retry or Esc/q to cancel",
                    error.report().message
                );
            } else {
                panel.profile_switch_pending = None;
                panel.profile_switch_retry_required = false;
                panel.profile_switch_prompt = Some(ProfileSwitchPrompt {
                    old_profile: old_active_profile,
                    new_profile: profile,
                    selected: usize::from(panel.profile_switch_discard_pending),
                });
                panel.status = format!(
                    "profile selection was not saved: {} — staged changes retained",
                    error.report().message
                );
            }
            return;
        }
    };
    panel.persisted_conns = persisted.clone();
    panel.conns = persisted;
    // Persistence changes the Daemon's authoritative owner immediately. Tell
    // fleetyd to leave its old session even if the CLI's own B handshake later
    // fails; otherwise disk/UI can say B while A keeps accepting control.
    let daemon_notice = match notify_daemon(&profile) {
        Ok(message) => message,
        Err(error) => format!(
            "fleetyd reconnect failed: {}; profile is saved, run `fleetyd reconnect --profile {profile}`",
            error.report().message
        ),
    };

    let discarded = panel.profile_switch_discard_pending;
    panel.profile_switch_pending = None;
    panel.profile_switch_discard_pending = false;
    panel.profile_switch_retry_required = false;
    panel.invalidate_remote_for_reconnect(&profile, discarded);
    *config_protocol = None;
    *active_target = None;
    *active_fingerprint = None;
    match replace_profile_connection(connection, &target, connections_path).await {
        Ok((new_connection, protocol, fingerprint, committed_target)) => {
            *config_protocol = Some(protocol);
            let (mut new_tx, mut new_rx) = new_connection;
            let (server, daemon, notes) =
                reload_remote_regions(&mut new_tx, &mut new_rx, protocol, &panel.daemon_device_id)
                    .await;
            panel.server_supported = server.supported;
            panel.entries = server.entries;
            panel.revision = server.revision;
            panel.daemon_supported = daemon.supported;
            panel.daemon_entries = daemon.entries;
            panel.daemon_revision = daemon.revision;
            *connection = Some((new_tx, new_rx));
            *active_target = Some(committed_target);
            *active_fingerprint = fingerprint;
            panel.status = if notes.is_empty() {
                format!(
                    "connected to '{profile}'; Server and Daemon settings reloaded; {daemon_notice}"
                )
            } else {
                format!(
                    "connected to '{profile}'; {}; {daemon_notice}",
                    notes.join(" | ")
                )
            };
        }
        Err(error) => {
            panel.status = format!(
                "selected profile '{profile}', but CLI reconnect failed: {} — Server and Daemon settings are unavailable; the previous CLI connection and snapshots were not reused; {daemon_notice}",
                error.report().message,
            );
        }
    }
}

fn save_connection_url_edits_at(
    path: &std::path::Path,
    pending: &connection::Connections,
    baseline: &connection::Connections,
) -> Result<(connection::Connections, Option<String>)> {
    connection::mutate_at(path, |live| {
        let mut reconnect_profile = None;
        for (name, edited) in &pending.profiles {
            let Some(before) = baseline.profiles.get(name) else {
                continue;
            };
            if edited.url == before.url {
                continue;
            }
            let is_live_current = live.current.as_deref() == Some(name.as_str());
            let current = live.profiles.get_mut(name).ok_or_else(|| {
                CoreError::Message(format!(
                    "server profile '{name}' was removed in another Fleety process"
                ))
            })?;
            if current.url != before.url {
                return Err(CoreError::Message(format!(
                    "server profile '{name}' changed in another Fleety process; reopen Settings"
                )));
            }
            connection::reselect_profile_endpoint(current, edited.url.clone());
            if is_live_current {
                reconnect_profile = Some(name.clone());
            }
        }
        Ok((live.clone(), reconnect_profile))
    })
}

/// `fleety config` (bare, on a TTY): a top-level menu — pick what to configure
/// (Providers & Models / Settings) and drill into it; Esc/q leaves. Each item
/// runs its own screen and returns here.
pub async fn run(
    session: crate::workspace::WorkspaceSession,
) -> Result<crate::workspace::SessionResult> {
    run_settings(session).await
}

/// Run a provider's requested OAuth action (the editor exited to let us), print
/// the outcome, and wait for Enter so the user can read it before the editor
/// reopens. Switch = sign out then in. Shared by the local and remote editors.
pub(crate) async fn run_auth_action_on_target(
    req: &crate::provider_tui::AuthRequest,
    target: &fleety_tools::connection::Resolved,
    expected_fingerprint: Option<&str>,
    input: &mut crate::workspace::WorkspaceInput,
) {
    run_auth_action_for_target(req, Some((target, expected_fingerprint)), input).await;
}

async fn run_auth_action_for_target(
    req: &crate::provider_tui::AuthRequest,
    target: Option<(&fleety_tools::connection::Resolved, Option<&str>)>,
    input: &mut crate::workspace::WorkspaceInput,
) {
    use crate::provider_tui::AuthAction;
    let result = match req.action {
        AuthAction::Login => match target {
            Some((target, fingerprint)) => {
                crate::auth::login_on_target(&req.provider, false, target, fingerprint).await
            }
            None => crate::auth::login(&req.provider, false).await,
        },
        AuthAction::Logout => match target {
            Some((target, fingerprint)) => {
                crate::auth::logout_on_target(&req.provider, target, fingerprint).await
            }
            None => crate::auth::logout(&req.provider).await,
        },
        AuthAction::Switch => {
            // "Switch account" = sign in as a different account. Tolerate a logout
            // failure (e.g. this provider was never signed in) — reaching login is
            // what matters, so a delete of an absent credential must not abort it.
            match target {
                Some((target, fingerprint)) => {
                    let _ = crate::auth::logout_on_target(&req.provider, target, fingerprint).await;
                    crate::auth::login_on_target(&req.provider, false, target, fingerprint).await
                }
                None => {
                    let _ = crate::auth::logout(&req.provider).await;
                    crate::auth::login(&req.provider, false).await
                }
            }
        }
    };
    if let Err(e) = result {
        eprintln!(
            "auth for '{}': {}",
            crate::terminal_safe_text(&req.provider),
            crate::terminal_safe_text(&e.report().message)
        );
    }
    print!("\nPress Enter to return to the editor… ");
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = input.wait_for_enter().await;
    input.handoff().await;
}

/// The four-region settings editor (Connection / CLI / Daemon / Server),
/// launched from the top-level menu's "Settings" item.
async fn run_settings(
    session: crate::workspace::WorkspaceSession,
) -> Result<crate::workspace::SessionResult> {
    let crate::workspace::WorkspaceSession {
        mut workspace,
        chat,
        chat_transport,
        mut input,
        daemon_device_id,
    } = session;
    // Resolve + connect, capturing the Welcome so we know the server's config
    // protocol. On any connection failure, the panel still opens for the CLI
    // + connection regions; the daemon and server regions report unavailable.
    let requested_target = crate::resolve_target()?;
    let (mut conn, mut config_protocol, mut active_fingerprint, mut active_target) =
        match crate::open_panel(&requested_target).await {
            Ok((streams, cp, fingerprint, committed_target)) => {
                (Some(streams), Some(cp), fingerprint, Some(committed_target))
            }
            Err(e) => {
                eprintln!(
                    "note: could not reach the server ({}) — the Daemon and Server regions are \
                     unavailable; the Connection and CLI regions still work.",
                    crate::terminal_safe_text(&e.report().message)
                );
                (None, None, None, Some(requested_target))
            }
        };
    // Invocation context remains the active identity even when its transport is
    // unavailable. Falling back to `conns.current` here would make
    // `--profile B config open` present and mutate profile A after a failed B
    // connection attempt.
    let mut server_supported = false;
    let mut entries: Vec<ConfigEntry> = Vec::new();
    let mut revision = String::new();
    let mut daemon_supported = false;
    let mut daemon_entries: Vec<ConfigEntry> = Vec::new();
    let mut daemon_revision = String::new();
    if let (Some((tx, rx)), Some(cp)) = (conn.as_mut(), config_protocol) {
        server_supported = cp >= 5;
        if server_supported {
            match pull_owner_snapshot(tx, rx, ConfigTarget::Server, cp).await {
                Ok((rev, es)) => {
                    revision = rev;
                    entries = es;
                }
                Err(e) => {
                    server_supported = false;
                    eprintln!(
                        "note: snapshot failed ({}); Server region read-only.",
                        crate::terminal_safe_text(&e.report().message)
                    );
                }
            }
        } else {
            eprintln!(
                "note: Server settings require write-only Provider snapshots (config protocol 5); update the Server."
            );
        }
        if cp >= 1 {
            match pull_owner_snapshot(tx, rx, ConfigTarget::Device(daemon_device_id.clone()), cp)
                .await
            {
                Ok((rev, es)) => {
                    daemon_supported = true;
                    daemon_revision = rev;
                    daemon_entries = es;
                }
                Err(e) => {
                    eprintln!(
                        "note: daemon config is unavailable ({}); no local fallback will be used.",
                        crate::terminal_safe_text(&e.report().message)
                    );
                }
            }
        }
    }

    let mut conns = connection::load()?;
    // Offer the local server (like guided init): if one answers on loopback and
    // no saved profile already targets it, inject a `local` entry so it shows in
    // the Connection region. Kept in memory — only persisted if the user saves.
    let local_url = crate::local_server_url();
    if !has_local_profile(&conns, &local_url)
        && crate::probe_local_server(&local_url, std::time::Duration::from_secs(1))
            .await
            .is_some()
    {
        conns
            .profiles
            .entry("local".to_string())
            .or_insert_with(|| connection::Profile {
                url: local_url.clone(),
                ..Default::default()
            });
    }
    let local_map = fleety_tools::config::load(&fleety_tools::config::config_path());
    let mut app = Panel::new(
        conns,
        local_map,
        RemoteRegionState::new(daemon_supported, daemon_entries, daemon_revision),
        RemoteRegionState::new(server_supported, entries, revision),
    );
    app.daemon_device_id = daemon_device_id.clone();
    if let Some(target) = active_target.as_ref() {
        app.activate_target(target);
    }
    if conn.is_some() {
        workspace.reduce(crate::workspace::Action::Connected);
    } else {
        workspace.reduce(crate::workspace::Action::Offline(
            "The selected Server is unavailable".into(),
        ));
    }
    sync_workspace_from_panel(&mut workspace, &app);
    sync_active_target_context(
        &mut workspace,
        active_target.as_ref(),
        active_fingerprint.as_deref(),
    );

    let mut terminal = ratatui::init();
    let result: Result<crate::workspace::SessionResult> = loop {
        if matches!(
            &workspace.route,
            crate::workspace::Route::Chat | crate::workspace::Route::Conversations
        ) {
            break Ok(crate::workspace::SessionResult::Continue(Box::new(
                crate::workspace::WorkspaceSession {
                    workspace,
                    chat,
                    chat_transport,
                    input,
                    daemon_device_id,
                },
            )));
        }
        if matches!(&workspace.route, crate::workspace::Route::ConnectionPicker) {
            app.region = Region::Connection;
            workspace.route =
                crate::workspace::Route::Settings(crate::workspace::SettingsPage::Connection);
        }
        sync_workspace_from_panel(&mut workspace, &app);
        sync_active_target_context(
            &mut workspace,
            active_target.as_ref(),
            active_fingerprint.as_deref(),
        );
        if let Err(e) = terminal.draw(|frame| {
            crate::workspace::render(frame, &workspace, |frame, area| {
                render_in_area(frame, &app, area);
            });
        }) {
            break Err(CoreError::Message(format!("draw failed: {e}")));
        }
        if app.quit {
            break Ok(crate::workspace::SessionResult::Exit);
        }
        let Some(mut key) = input.recv().await else {
            break Ok(crate::workspace::SessionResult::Exit);
        };
        if app.profile_switch_retry_required {
            match app.resolve_profile_switch_retry(key) {
                ProfileSwitchRetryAction::PassThrough => {}
                ProfileSwitchRetryAction::Retry => {
                    key = ratatui::crossterm::event::KeyEvent::new(
                        KeyCode::Null,
                        ratatui::crossterm::event::KeyModifiers::NONE,
                    );
                }
                ProfileSwitchRetryAction::Cancelled | ProfileSwitchRetryAction::Waiting => {
                    continue;
                }
            }
        }
        if app.profile_switch_prompt.is_some() {
            on_key(&mut app, key.code);
            sync_workspace_from_panel(&mut workspace, &app);
            if !app.profile_switch_apply_now && app.profile_switch_pending.is_none() {
                continue;
            }
            key = ratatui::crossterm::event::KeyEvent::new(
                KeyCode::Null,
                ratatui::crossterm::event::KeyModifiers::NONE,
            );
        }
        let key_context = crate::workspace::KeyContext {
            turn_in_flight: false,
            has_unsent_input: false,
            has_dirty_owner: app.local_dirty
                || !app.daemon_staged.is_empty()
                || !app.staged.is_empty(),
            text_input_focused: app.edit.is_some() || app.tz_pick.is_some(),
        };
        let workspace_key = if key.code == KeyCode::Char('q') && !key_context.text_input_focused {
            ratatui::crossterm::event::KeyEvent::new(
                KeyCode::Esc,
                ratatui::crossterm::event::KeyModifiers::NONE,
            )
        } else {
            key
        };
        match crate::workspace::on_key(&mut workspace, workspace_key, key_context) {
            crate::workspace::KeyOutcome::ExitRequested => {
                break Ok(crate::workspace::SessionResult::Exit)
            }
            crate::workspace::KeyOutcome::Consumed(_) => continue,
            crate::workspace::KeyOutcome::Forward => {}
        }
        let _changed = on_key(&mut app, key.code);
        if app.profile_switch_apply_now {
            workspace.reduce(crate::workspace::Action::ResolveNotices(
                "Profile switch apply failed".into(),
            ));
            sync_workspace_from_panel(&mut workspace, &app);
            let _ = terminal.draw(|frame| {
                crate::workspace::render(frame, &workspace, |frame, area| {
                    render_in_area(frame, &app, area);
                });
            });
            if !apply_staged_before_profile_switch(
                &mut app,
                &mut conn,
                config_protocol.unwrap_or_default(),
            )
            .await
            {
                let error = app
                    .daemon_apply_error
                    .as_ref()
                    .or(app.server_apply_error.as_ref());
                let mut notice = crate::workspace::Notice::error("Profile switch apply failed")
                    .details(app.status.clone());
                if let Some(remediation) = error.and_then(|error| error.remediation.clone()) {
                    notice = notice.remediation(remediation);
                }
                workspace.reduce(crate::workspace::Action::PushNotice(notice));
            }
        }
        if app.profile_switch_pending.is_some() {
            commit_profile_switch(
                &mut app,
                &mut conn,
                &mut config_protocol,
                &mut active_target,
                &mut active_fingerprint,
            )
            .await;
            if conn.is_some() {
                workspace.reduce(crate::workspace::Action::Connected);
            } else {
                workspace.reduce(crate::workspace::Action::Offline(app.status.clone()));
            }
        }
        // Explicit URL/profile edits still support `s`; a profile switch itself
        // persists through the transaction above and never needs a second key.
        if app.status == "__save_conns__" {
            let pending = app.conns.clone();
            let baseline = app.persisted_conns.clone();
            let needs_repair = pending.profiles.iter().any(|(name, edited)| {
                baseline.profiles.get(name).is_some_and(|before| {
                    edited.url != before.url
                        && (before.token.is_some() || before.fingerprint.is_some())
                })
            });
            match save_connection_url_edits_at(&connection::connections_path(), &pending, &baseline)
            {
                Ok((saved, reconnect_profile)) => {
                    app.persisted_conns = saved.clone();
                    app.conns = saved;
                    let base_status = if needs_repair {
                        "saved connections; old credentials were cleared — re-pair the changed profile"
                            .to_string()
                    } else {
                        "saved connections".to_string()
                    };
                    app.status = if let Some(profile) = reconnect_profile {
                        conn = None;
                        config_protocol = None;
                        active_target = None;
                        active_fingerprint = None;
                        app.invalidate_remote_for_reconnect(&profile, false);
                        match crate::server::notify_daemon_reconnect(&profile) {
                            Ok(notice) if notice.is_empty() => base_status,
                            Ok(notice) => format!("{base_status}; {notice}"),
                            Err(error) => format!(
                                "{base_status}; fleetyd notification failed: {} — run `fleetyd reconnect --profile {profile}`",
                                error.report().message
                            ),
                        }
                    } else {
                        base_status
                    };
                }
                Err(e) => {
                    app.status = format!("save failed: {}", e.report().message);
                }
            }
        }
        if app.apply_cli_now {
            app.apply_cli_now = false;
            if let Err(e) =
                crate::config::apply_cli_owner(&fleety_tools::config::config_path(), &app.local_map)
            {
                app.status = format!("save failed: {}", e.report().message);
                app.local_apply_error = Some(crate::workspace::WorkspaceError {
                    kind: "apply_failed".into(),
                    message: e.report().message,
                    remediation: Some("Fix the value and retry the CLI owner".into()),
                });
            } else {
                app.local_dirty = false;
                app.local_apply_error = None;
                app.status = "applied CLI settings".into();
            }
        }
        if app.open_provider_now {
            app.open_provider_now = false;
            ratatui::restore();
            let provider_result = match active_target.as_ref() {
                Some(target) => {
                    crate::config::provider_edit_remote_on_target(
                        target,
                        active_fingerprint.as_deref(),
                        &mut input,
                    )
                    .await
                }
                None => Err(CoreError::Message(
                    "Providers & Models are unavailable because no Server is connected".into(),
                )),
            };
            match provider_result {
                Ok(()) => {
                    app.provider_error = None;
                    app.status = "Provider and model workflow closed".into();
                }
                Err(error) => {
                    app.provider_error = Some(crate::workspace::WorkspaceError {
                        kind: "provider_workflow".into(),
                        message: error.report().message.clone(),
                        remediation: Some("Reconnect and reopen Providers & Models".into()),
                    });
                    app.status = format!("Provider workflow failed: {}", error.report().message);
                }
            }
            input.handoff().await;
            terminal = ratatui::init();
        }
        if app.apply_now {
            sync_workspace_from_panel(&mut workspace, &app);
            app.apply_now = false;
            if let Some((tx, rx)) = conn.as_mut() {
                let changes: Vec<ConfigChange> = app.staged.values().cloned().collect();
                let revision = app.revision.clone();
                let outcome = apply_and_refresh(
                    tx,
                    rx,
                    ConfigTarget::Server,
                    &revision,
                    changes,
                    config_protocol.unwrap_or_default(),
                )
                .await;
                app.finish_remote_apply(Region::Server, outcome);
            } else {
                app.server_apply_error = Some(crate::workspace::WorkspaceError {
                    kind: "unavailable".into(),
                    message: "The Server owner is unavailable".into(),
                    remediation: Some("Reconnect before applying Server settings".into()),
                });
                app.status = "Server apply failed: owner unavailable".into();
            }
        }
        if app.apply_daemon_now {
            sync_workspace_from_panel(&mut workspace, &app);
            app.apply_daemon_now = false;
            if let Some((tx, rx)) = conn.as_mut() {
                let changes: Vec<ConfigChange> = app.daemon_staged.values().cloned().collect();
                let target = ConfigTarget::Device(app.daemon_device_id.clone());
                let revision = app.daemon_revision.clone();
                let outcome = apply_and_refresh(
                    tx,
                    rx,
                    target,
                    &revision,
                    changes,
                    config_protocol.unwrap_or_default(),
                )
                .await;
                app.finish_remote_apply(Region::Daemon, outcome);
            } else {
                app.daemon_apply_error = Some(crate::workspace::WorkspaceError {
                    kind: "unavailable".into(),
                    message: "The Daemon owner is unavailable".into(),
                    remediation: Some("Reconnect the selected device before applying".into()),
                });
                app.status = "Daemon apply failed: owner unavailable".into();
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
    target: ConfigTarget,
) -> Result<(String, Vec<ConfigEntry>)> {
    crate::send(tx, &ClientMsg::ConfigSnapshot { target }).await?;
    match crate::recv(rx).await? {
        Some(ServerMsg::ConfigSnapshotResult {
            revision, entries, ..
        }) => Ok((revision, entries)),
        Some(ServerMsg::Error { error }) => Err(CoreError::Provider(error.message)),
        Some(other) => Err(CoreError::Provider(format!(
            "expected a config snapshot, got {}",
            crate::server_msg_kind(&other)
        ))),
        None => Err(CoreError::Provider(
            "the Server closed before returning a config snapshot".to_string(),
        )),
    }
}

async fn pull_owner_snapshot(
    tx: &mut fleety_tools::transport::Sender,
    rx: &mut fleety_tools::transport::Receiver,
    target: ConfigTarget,
    config_protocol: u32,
) -> Result<(String, Vec<ConfigEntry>)> {
    if matches!(target, ConfigTarget::Server) {
        let snapshot = crate::provider_service::load_snapshot(tx, rx, config_protocol).await?;
        Ok((snapshot.revision, snapshot.entries))
    } else {
        pull_snapshot(tx, rx, target).await
    }
}

/// Reload the two owner-scoped regions over one freshly authenticated server
/// connection. Each result is independent: a missing daemon does not hide a
/// usable server snapshot, while an old config protocol leaves both regions
/// unavailable instead of falling back to direct files.
async fn reload_remote_regions(
    tx: &mut fleety_tools::transport::Sender,
    rx: &mut fleety_tools::transport::Receiver,
    config_protocol: u32,
    device_id: &str,
) -> (RemoteRegionState, RemoteRegionState, Vec<String>) {
    if config_protocol < 1 {
        return (
            RemoteRegionState::new(false, vec![], String::new()),
            RemoteRegionState::new(false, vec![], String::new()),
            vec![
                "server does not support structured config; Server and Daemon are unavailable"
                    .to_string(),
            ],
        );
    }

    let mut notes = Vec::new();
    let server = if config_protocol < 5 {
        notes.push(
            "Server settings unavailable until it supports write-only Provider snapshots; update the Server"
                .to_string(),
        );
        RemoteRegionState::new(false, vec![], String::new())
    } else {
        match pull_owner_snapshot(tx, rx, ConfigTarget::Server, config_protocol).await {
            Ok((revision, entries)) => RemoteRegionState::new(true, entries, revision),
            Err(e) => {
                notes.push(format!("Server unavailable: {}", e.report().message));
                RemoteRegionState::new(false, vec![], String::new())
            }
        }
    };
    let daemon_target = ConfigTarget::Device(device_id.to_string());
    let daemon = match pull_snapshot(tx, rx, daemon_target).await {
        Ok((revision, entries)) => RemoteRegionState::new(true, entries, revision),
        Err(e) => {
            notes.push(format!("Daemon unavailable: {}", e.report().message));
            RemoteRegionState::new(false, vec![], String::new())
        }
    };
    (server, daemon, notes)
}

/// Send one owner-scoped `ConfigApply` and preserve the wire error kind and
/// remediation so Conflict and Failed remain distinguishable in the UI.
async fn apply_changes(
    tx: &mut fleety_tools::transport::Sender,
    rx: &mut fleety_tools::transport::Receiver,
    target: ConfigTarget,
    base_revision: &str,
    changes: Vec<ConfigChange>,
) -> std::result::Result<OwnerApplySuccess, crate::provider_service::ProviderIssue> {
    crate::send(
        tx,
        &ClientMsg::ConfigApply {
            target,
            base_revision: base_revision.to_string(),
            changes,
            providers_json: None,
        },
    )
    .await
    .map_err(|error| {
        crate::provider_service::ProviderIssue::new(
            "transport",
            error.report().message,
            Some("Reconnect to the owner and retry"),
        )
    })?;
    match crate::recv(rx).await.map_err(|error| {
        crate::provider_service::ProviderIssue::new(
            "transport",
            error.report().message,
            Some("Reconnect to the owner and retry"),
        )
    })? {
        Some(ServerMsg::ConfigResult {
            ok: true,
            output,
            effect,
            ..
        }) => Ok(OwnerApplySuccess {
            message: if output.is_empty() {
                "Saved".to_string()
            } else {
                output
            },
            effect,
        }),
        Some(ServerMsg::ConfigResult {
            ok: false, error, ..
        }) => Err(error.map_or_else(
            || {
                crate::provider_service::ProviderIssue::new(
                    "rejected",
                    "Apply was rejected without a reason",
                    Some("Reload the owner snapshot and retry"),
                )
            },
            crate::provider_service::issue_from_wire,
        )),
        other => Err(crate::provider_service::ProviderIssue::new(
            "unexpected_reply",
            format!(
                "Expected an owner apply result, got {}",
                crate::server_msg_kind_option(other.as_ref())
            ),
            Some("Reconnect and retry"),
        )),
    }
}

async fn apply_and_refresh(
    tx: &mut fleety_tools::transport::Sender,
    rx: &mut fleety_tools::transport::Receiver,
    target: ConfigTarget,
    base_revision: &str,
    changes: Vec<ConfigChange>,
    config_protocol: u32,
) -> std::result::Result<OwnerApplyRefresh, crate::provider_service::ProviderIssue> {
    let success = apply_changes(tx, rx, target.clone(), base_revision, changes).await?;
    Ok(
        match pull_owner_snapshot(tx, rx, target, config_protocol).await {
            Ok((revision, entries)) => OwnerApplyRefresh::Refreshed {
                success,
                revision,
                entries,
            },
            Err(error) => OwnerApplyRefresh::RefreshRequired {
                success,
                reason: error.report().message,
            },
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{SinkExt, StreamExt};
    use tokio::sync::oneshot;
    use tokio_tungstenite::tungstenite::Message;

    async fn start_close_observer() -> (String, oneshot::Receiver<bool>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind close observer");
        let addr = listener.local_addr().expect("close observer address");
        let (closed_tx, closed_rx) = oneshot::channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept old connection");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("accept old websocket");
            let closed = matches!(ws.next().await, Some(Ok(Message::Close(_))) | None);
            let _ = closed_tx.send(closed);
        });
        (format!("ws://{addr}"), closed_rx)
    }

    async fn start_welcome_server() -> (String, oneshot::Receiver<ClientMsg>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind welcome server");
        let addr = listener.local_addr().expect("welcome server address");
        let (hello_tx, hello_rx) = oneshot::channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept new connection");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("accept new websocket");
            let frame = ws
                .next()
                .await
                .expect("hello frame")
                .expect("read hello frame");
            let hello =
                serde_json::from_str::<ClientMsg>(frame.to_text().expect("hello is a text frame"))
                    .expect("deserialize hello");
            ws.send(Message::Text(
                serde_json::to_string(&ServerMsg::Welcome {
                    session_id: "session-a".into(),
                    conversation_id: "conversation-a".into(),
                    protocol: fleety_protocol::PROTOCOL_VERSION,
                    server_version: String::new(),
                    audio_input: false,
                    config_protocol: fleety_protocol::CONFIG_PROTOCOL_VERSION,
                    server_fingerprint: Some("fingerprint-a".into()),
                    loopback_trusted: false,
                    token: None,
                })
                .expect("serialize welcome"),
            ))
            .await
            .expect("send welcome");
            let _ = hello_tx.send(hello);
            let _ = ws.next().await;
        });
        (format!("ws://{addr}"), hello_rx)
    }

    async fn start_snapshot_server(daemon_ok: bool) -> (String, oneshot::Receiver<Vec<ClientMsg>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind snapshot server");
        let addr = listener.local_addr().expect("snapshot server address");
        let (requests_tx, requests_rx) = oneshot::channel();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept snapshot connection");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("accept snapshot websocket");
            let mut requests = Vec::new();
            for step in 0..3 {
                let frame = ws
                    .next()
                    .await
                    .expect("client frame")
                    .expect("read client frame");
                let request = serde_json::from_str::<ClientMsg>(
                    frame.to_text().expect("client frame is text"),
                )
                .expect("deserialize client request");
                requests.push(request.clone());
                let response = match step {
                    0 => ServerMsg::Welcome {
                        session_id: "session-a".into(),
                        conversation_id: "conversation-a".into(),
                        protocol: fleety_protocol::PROTOCOL_VERSION,
                        server_version: String::new(),
                        audio_input: false,
                        config_protocol: fleety_protocol::CONFIG_PROTOCOL_VERSION,
                        server_fingerprint: Some("fingerprint-a".into()),
                        loopback_trusted: false,
                        token: None,
                    },
                    1 => ServerMsg::ConfigSnapshotResult {
                        revision: "server-a-rev".into(),
                        entries: vec![entry("FLEETY_POLICY", "full_access", false)],
                        providers_json: r#"{"key_present":[]}"#.into(),
                    },
                    2 if daemon_ok => ServerMsg::ConfigSnapshotResult {
                        revision: "daemon-a-rev".into(),
                        entries: vec![entry("FLEETY_TZ", "Asia/Taipei", false)],
                        providers_json: String::new(),
                    },
                    2 => ServerMsg::Error {
                        error: fleety_protocol::WireError {
                            kind: "not_connected".into(),
                            message: "daemon unavailable".into(),
                            remediation: None,
                        },
                    },
                    _ => unreachable!(),
                };
                ws.send(Message::Text(
                    serde_json::to_string(&response).expect("serialize scripted response"),
                ))
                .await
                .expect("send scripted response");
            }
            let _ = requests_tx.send(requests);
            let _ = ws.next().await;
        });
        (format!("ws://{addr}"), requests_rx)
    }

    #[derive(Clone, Copy)]
    enum ProfileSwitchApplyScript {
        ServerConflict,
        DaemonRefreshFailure,
        AllRefreshed,
        ReloadThenServerApply,
    }

    async fn start_profile_switch_apply_server(
        script: ProfileSwitchApplyScript,
    ) -> (String, oneshot::Receiver<Vec<ClientMsg>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind profile switch apply server");
        let addr = listener.local_addr().expect("profile switch apply address");
        let (requests_tx, requests_rx) = oneshot::channel();
        tokio::spawn(async move {
            let (stream, _) = listener
                .accept()
                .await
                .expect("accept profile switch apply connection");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("accept profile switch apply websocket");
            let mut requests = Vec::new();
            let mut server_snapshots = 0;
            loop {
                let frame = match tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    websocket.next(),
                )
                .await
                {
                    Ok(Some(Ok(frame))) => frame,
                    _ => break,
                };
                let request: ClientMsg =
                    serde_json::from_str(frame.to_text().expect("profile switch request is text"))
                        .expect("parse profile switch request");
                let response = match &request {
                    ClientMsg::Hello { .. } => ServerMsg::Welcome {
                        session_id: "profile-switch-session".into(),
                        conversation_id: "profile-switch-conversation".into(),
                        protocol: fleety_protocol::PROTOCOL_VERSION,
                        server_version: String::new(),
                        audio_input: false,
                        config_protocol: fleety_protocol::CONFIG_PROTOCOL_VERSION,
                        server_fingerprint: Some("fingerprint-a".into()),
                        loopback_trusted: false,
                        token: None,
                    },
                    ClientMsg::ConfigApply {
                        target: ConfigTarget::Device(device),
                        ..
                    } => {
                        assert_eq!(device, "remote-B");
                        ServerMsg::ConfigResult {
                            ok: true,
                            output: "Daemon saved".into(),
                            effect: None,
                            error: None,
                        }
                    }
                    ClientMsg::ConfigSnapshot {
                        target: ConfigTarget::Device(device),
                    } => {
                        assert_eq!(device, "remote-B");
                        match script {
                            ProfileSwitchApplyScript::DaemonRefreshFailure => ServerMsg::Error {
                                error: fleety_protocol::WireError {
                                    kind: "snapshot_failed".into(),
                                    message: "fresh Daemon snapshot unavailable".into(),
                                    remediation: None,
                                },
                            },
                            _ => ServerMsg::ConfigSnapshotResult {
                                revision: if matches!(
                                    script,
                                    ProfileSwitchApplyScript::ReloadThenServerApply
                                ) {
                                    "daemon-r1"
                                } else {
                                    "daemon-r2"
                                }
                                .into(),
                                entries: vec![entry(
                                    "FLEETY_TZ",
                                    if matches!(
                                        script,
                                        ProfileSwitchApplyScript::ReloadThenServerApply
                                    ) {
                                        "UTC"
                                    } else {
                                        "Asia/Taipei"
                                    },
                                    false,
                                )],
                                providers_json: String::new(),
                            },
                        }
                    }
                    ClientMsg::ConfigApply {
                        target: ConfigTarget::Server,
                        ..
                    } => match script {
                        ProfileSwitchApplyScript::ServerConflict => ServerMsg::ConfigResult {
                            ok: false,
                            output: String::new(),
                            effect: None,
                            error: Some(fleety_protocol::WireError {
                                kind: "conflict".into(),
                                message: "Server revision changed".into(),
                                remediation: Some("Reload Server settings".into()),
                            }),
                        },
                        _ => ServerMsg::ConfigResult {
                            ok: true,
                            output: "Server saved".into(),
                            effect: None,
                            error: None,
                        },
                    },
                    ClientMsg::ConfigSnapshot {
                        target: ConfigTarget::Server,
                    } => {
                        server_snapshots += 1;
                        let initial_reload =
                            matches!(script, ProfileSwitchApplyScript::ReloadThenServerApply)
                                && server_snapshots == 1;
                        ServerMsg::ConfigSnapshotResult {
                            revision: if initial_reload {
                                "server-r1"
                            } else {
                                "server-r2"
                            }
                            .into(),
                            entries: vec![entry(
                                "FLEETY_POLICY",
                                if initial_reload {
                                    "full_access"
                                } else {
                                    "require_approval"
                                },
                                false,
                            )],
                            providers_json: r#"{"key_present":[]}"#.into(),
                        }
                    }
                    other => panic!("unexpected profile switch request: {other:?}"),
                };
                requests.push(request);
                websocket
                    .send(Message::Text(
                        serde_json::to_string(&response)
                            .expect("serialize profile switch response"),
                    ))
                    .await
                    .expect("send profile switch response");
                let expected_requests =
                    if matches!(script, ProfileSwitchApplyScript::ReloadThenServerApply) {
                        5
                    } else {
                        4
                    };
                if requests.len() == expected_requests {
                    break;
                }
            }
            let _ = requests_tx.send(requests);
        });
        (format!("ws://{addr}"), requests_rx)
    }

    fn dirty_two_owner_profile_switch_panel() -> Panel {
        let mut panel = Panel::new(
            Connections {
                current: Some("A".into()),
                ..Default::default()
            },
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(
                true,
                vec![entry("FLEETY_TZ", "UTC", false)],
                "daemon-r1".into(),
            ),
            RemoteRegionState::new(
                true,
                vec![entry("FLEETY_POLICY", "full_access", false)],
                "server-r1".into(),
            ),
        );
        panel.active_profile = "A".into();
        panel.daemon_device_id = "remote-B".into();
        panel.daemon_staged.insert(
            "FLEETY_TZ".into(),
            ConfigChange {
                key: "FLEETY_TZ".into(),
                op: ChangeOp::Set,
                value: Some("Asia/Taipei".into()),
            },
        );
        panel.staged.insert(
            "FLEETY_POLICY".into(),
            ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            },
        );
        panel.request_profile_switch("B".into());
        panel.resolve_profile_switch(ProfileSwitchResolution::Apply);
        panel
    }

    #[test]
    fn has_local_profile_matches_by_url() {
        let mut conns = Connections::default();
        assert!(!has_local_profile(&conns, "ws://127.0.0.1:8787"));
        conns.profiles.insert(
            "home".into(),
            connection::Profile {
                url: "ws://mini:8787".into(),
                ..Default::default()
            },
        );
        assert!(!has_local_profile(&conns, "ws://127.0.0.1:8787"));
        conns.profiles.insert(
            "local".into(),
            connection::Profile {
                url: "ws://127.0.0.1:8787".into(),
                ..Default::default()
            },
        );
        assert!(has_local_profile(&conns, "ws://127.0.0.1:8787"));
    }

    fn panel_with_entries(entries: Vec<ConfigEntry>) -> Panel {
        Panel::new(
            Connections::default(),
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(false, vec![], String::new()),
            RemoteRegionState::new(true, entries, "rev-1".to_string()),
        )
    }

    #[test]
    fn profile_switch_invalidates_old_remote_state() {
        let mut panel = Panel::new(
            Connections::default(),
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(
                true,
                vec![entry("FLEETY_TZ", "UTC", false)],
                "daemon-b".into(),
            ),
            RemoteRegionState::new(
                true,
                vec![entry("FLEETY_POLICY", "full_access", false)],
                "server-b".into(),
            ),
        );
        panel.daemon_staged.insert(
            "FLEETY_TZ".into(),
            ConfigChange {
                key: "FLEETY_TZ".into(),
                op: ChangeOp::Set,
                value: Some("Asia/Taipei".into()),
            },
        );
        panel.staged.insert(
            "FLEETY_POLICY".into(),
            ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            },
        );
        panel.apply_daemon_now = true;
        panel.apply_now = true;

        panel.invalidate_remote_for_reconnect("a", true);

        assert!(!panel.daemon_supported);
        assert!(panel.daemon_entries.is_empty());
        assert!(panel.daemon_revision.is_empty());
        assert!(panel.daemon_staged.is_empty());
        assert!(!panel.apply_daemon_now);
        assert!(!panel.server_supported);
        assert!(panel.entries.is_empty());
        assert!(panel.revision.is_empty());
        assert!(panel.staged.is_empty());
        assert!(!panel.apply_now);
        assert!(panel.status.contains("connecting to 'a'"));
        assert!(panel.status.contains("discarded staged remote changes"));
    }

    #[test]
    fn dirty_profile_switch_cancel_keeps_profile_edits_and_transport_intent() {
        let mut panel = panel_with_entries(vec![entry("FLEETY_POLICY", "full_access", false)]);
        panel.conns.current = Some("A".into());
        panel.active_profile = "A".into();
        panel.staged.insert(
            "FLEETY_POLICY".into(),
            ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            },
        );
        panel.request_profile_switch("B".into());
        let prompt = panel.profile_switch_prompt.as_ref().expect("switch prompt");
        assert_eq!(prompt.old_profile, "A");
        assert_eq!(prompt.new_profile, "B");

        panel.resolve_profile_switch(ProfileSwitchResolution::Cancel);

        assert_eq!(panel.conns.current.as_deref(), Some("A"));
        assert!(panel.staged.contains_key("FLEETY_POLICY"));
        assert!(panel.profile_switch_prompt.is_none());
        assert!(panel.profile_switch_pending.is_none());
        assert!(!panel.profile_switch_apply_now);
    }

    #[test]
    fn dirty_profile_switch_discard_waits_for_profile_persistence() {
        let mut panel = panel_with_entries(vec![entry("FLEETY_POLICY", "full_access", false)]);
        panel.conns.current = Some("A".into());
        panel.active_profile = "A".into();
        panel.local_dirty = true;
        panel.staged.insert(
            "FLEETY_POLICY".into(),
            ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            },
        );
        panel.request_profile_switch("B".into());

        panel.resolve_profile_switch(ProfileSwitchResolution::Discard);

        assert!(panel.local_dirty, "CLI staging is not profile-scoped");
        assert!(
            panel.staged.contains_key("FLEETY_POLICY"),
            "discard is committed only after the profile selection is persisted"
        );
        assert!(panel.daemon_staged.is_empty());
        assert_eq!(panel.conns.current.as_deref(), Some("A"));
        assert_eq!(panel.profile_switch_pending.as_deref(), Some("B"));
    }

    #[test]
    fn dirty_profile_switch_apply_retains_edits_until_apply_succeeds() {
        let mut panel = panel_with_entries(vec![entry("FLEETY_POLICY", "full_access", false)]);
        panel.conns.current = Some("A".into());
        panel.active_profile = "A".into();
        panel.staged.insert(
            "FLEETY_POLICY".into(),
            ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            },
        );
        panel.request_profile_switch("B".into());

        panel.resolve_profile_switch(ProfileSwitchResolution::Apply);

        assert!(panel.profile_switch_apply_now);
        assert!(panel.staged.contains_key("FLEETY_POLICY"));
        assert!(panel.profile_switch_pending.is_none());
        assert_eq!(panel.conns.current.as_deref(), Some("A"));
    }

    #[test]
    fn clean_profile_switch_queues_without_a_dirty_state_prompt() {
        let mut panel = panel_with_entries(vec![]);
        panel.conns.current = Some("A".into());
        panel.active_profile = "A".into();

        panel.request_profile_switch("B".into());

        assert!(panel.profile_switch_prompt.is_none());
        assert_eq!(panel.profile_switch_pending.as_deref(), Some("B"));
        assert_eq!(panel.conns.current.as_deref(), Some("A"));
    }

    #[test]
    fn unavailable_invocation_override_is_active_without_mutating_persisted_current() {
        let mut conns = Connections {
            current: Some("A".into()),
            ..Default::default()
        };
        conns.profiles.insert(
            "A".into(),
            connection::Profile {
                url: "ws://a.test:8787".into(),
                ..Default::default()
            },
        );
        conns.profiles.insert(
            "B".into(),
            connection::Profile {
                url: "ws://b.test:8787".into(),
                ..Default::default()
            },
        );
        let mut panel = Panel::new(
            conns,
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(false, vec![], String::new()),
            RemoteRegionState::new(false, vec![], String::new()),
        );
        let target = connection::Resolved::unowned(
            "ws://b.test:8787".into(),
            None,
            connection::Source::Profile("B".into()),
        );
        panel.activate_target(&target);

        assert!(!panel.daemon_supported);
        assert!(!panel.server_supported);

        panel.request_profile_switch("B".into());
        assert!(panel.status.contains("already selected"));
        assert!(panel.profile_switch_pending.is_none());
        assert_eq!(panel.conns.current.as_deref(), Some("A"));

        panel.status.clear();
        panel.staged.insert(
            "FLEETY_POLICY".into(),
            ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            },
        );
        panel.request_profile_switch("A".into());
        let prompt = panel.profile_switch_prompt.as_ref().expect("dirty prompt");
        assert_eq!(prompt.old_profile, "B");
        assert_eq!(prompt.new_profile, "A");
        assert_eq!(panel.conns.current.as_deref(), Some("A"));
        assert_eq!(panel.active_profile, "B");
        assert_eq!(panel.active_endpoint.as_deref(), Some("ws://b.test:8787"));
    }

    #[test]
    fn dirty_profile_switch_modal_names_both_profiles_and_all_resolutions() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut panel = panel_with_entries(vec![entry("FLEETY_POLICY", "full_access", false)]);
        panel.conns.current = Some("office".into());
        panel.active_profile = "office".into();
        panel.staged.insert(
            "FLEETY_POLICY".into(),
            ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            },
        );
        panel.request_profile_switch("home".into());
        let mut workspace = crate::workspace::WorkspaceState::new(
            crate::workspace::Route::Settings(crate::workspace::SettingsPage::Connection),
        );
        sync_workspace_from_panel(&mut workspace, &panel);
        let mut terminal = Terminal::new(TestBackend::new(90, 20)).expect("terminal");
        terminal
            .draw(|frame| crate::workspace::render(frame, &workspace, |_, _| {}))
            .expect("draw profile switch modal");
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(content.contains("office"), "{content}");
        assert!(content.contains("home"), "{content}");
        assert!(content.contains("A: Apply"), "{content}");
        assert!(content.contains("D: Discard"), "{content}");
        assert!(content.contains("C/Esc: Cancel"), "{content}");
    }

    #[tokio::test]
    async fn profile_selection_save_failure_after_apply_stays_pending_without_dirty_modal() {
        let mut conns = Connections {
            current: Some("A".into()),
            ..Default::default()
        };
        conns.profiles.insert(
            "A".into(),
            connection::Profile {
                url: "ws://a.test:8787".into(),
                ..Default::default()
            },
        );
        conns.profiles.insert(
            "B".into(),
            connection::Profile {
                url: "ws://b.test:8787".into(),
                ..Default::default()
            },
        );
        let mut panel = Panel::new(
            conns,
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(
                true,
                vec![entry("FLEETY_TZ", "Asia/Taipei", false)],
                "daemon-r2".into(),
            ),
            RemoteRegionState::new(
                true,
                vec![entry("FLEETY_POLICY", "require_approval", false)],
                "server-r2".into(),
            ),
        );
        panel.queue_profile_switch("B", false);
        let blocked_path = std::env::temp_dir().join(format!(
            "fleety-profile-switch-applied-blocked-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&blocked_path).expect("create blocking directory");
        let mut connection = None;
        let mut config_protocol = Some(fleety_protocol::CONFIG_PROTOCOL_VERSION);
        let mut active_target = None;
        let mut active_fingerprint = None;

        commit_profile_switch_at(
            &mut panel,
            &mut connection,
            &mut config_protocol,
            &mut active_target,
            &mut active_fingerprint,
            &blocked_path,
            |_| Ok("fleetyd test reconnect accepted".to_string()),
        )
        .await;

        assert_eq!(panel.conns.current.as_deref(), Some("A"));
        assert_eq!(panel.profile_switch_pending.as_deref(), Some("B"));
        assert!(panel.profile_switch_prompt.is_none());
        assert!(panel.daemon_staged.is_empty());
        assert!(panel.staged.is_empty());
        assert_eq!(panel.daemon_revision, "daemon-r2");
        assert_eq!(panel.revision, "server-r2");
        assert_eq!(
            config_protocol,
            Some(fleety_protocol::CONFIG_PROTOCOL_VERSION)
        );
        assert!(panel.status.contains("profile selection remains pending"));
        assert!(!panel.status.contains("staged changes retained"));
        assert!(panel.profile_switch_retry_required);

        assert_eq!(
            panel.resolve_profile_switch_retry(KeyEvent::new(
                KeyCode::Char('r'),
                KeyModifiers::CONTROL,
            )),
            ProfileSwitchRetryAction::Waiting
        );
        assert_eq!(
            panel.resolve_profile_switch_retry(KeyEvent::new(
                KeyCode::Char('q'),
                KeyModifiers::CONTROL,
            )),
            ProfileSwitchRetryAction::Waiting
        );
        assert_eq!(
            panel.resolve_profile_switch_retry(KeyEvent::new(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
            )),
            ProfileSwitchRetryAction::PassThrough
        );
        assert_eq!(
            panel.resolve_profile_switch_retry(KeyEvent::new(
                KeyCode::Char('C'),
                KeyModifiers::CONTROL,
            )),
            ProfileSwitchRetryAction::PassThrough
        );
        assert_eq!(
            panel.resolve_profile_switch_retry(KeyEvent::new(
                KeyCode::Char('C'),
                KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            )),
            ProfileSwitchRetryAction::PassThrough
        );
        assert_eq!(panel.profile_switch_pending.as_deref(), Some("B"));
        assert_eq!(
            panel.resolve_profile_switch_retry(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            ProfileSwitchRetryAction::Cancelled
        );
        assert!(!panel.profile_switch_retry_required);
        assert!(panel.profile_switch_pending.is_none());
        commit_profile_switch_at(
            &mut panel,
            &mut connection,
            &mut config_protocol,
            &mut active_target,
            &mut active_fingerprint,
            &blocked_path,
            |_| panic!("Esc cancellation must prevent profile persistence and reconnect"),
        )
        .await;
        assert_eq!(panel.conns.current.as_deref(), Some("A"));
        assert!(panel.status.contains("cancelled"));
        std::fs::remove_dir(&blocked_path).expect("remove blocking directory");
    }

    #[tokio::test]
    async fn profile_switch_save_failure_keeps_old_profile_transport_and_staging() {
        let (old_url, old_closed) = start_close_observer().await;
        let old_transport = fleety_tools::transport::connect(&old_url, None)
            .await
            .expect("connect old profile");
        let mut old_connection = Some(old_transport.split());
        let mut conns = Connections {
            current: Some("A".into()),
            ..Default::default()
        };
        conns.profiles.insert(
            "A".into(),
            connection::Profile {
                url: old_url.clone(),
                ..Default::default()
            },
        );
        conns.profiles.insert(
            "B".into(),
            connection::Profile {
                url: "ws://127.0.0.1:9".into(),
                ..Default::default()
            },
        );
        let mut panel = Panel::new(
            conns,
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(false, vec![], String::new()),
            RemoteRegionState::new(
                true,
                vec![entry("FLEETY_POLICY", "full_access", false)],
                "rev-a".into(),
            ),
        );
        panel.staged.insert(
            "FLEETY_POLICY".into(),
            ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            },
        );
        panel.request_profile_switch("B".into());
        panel.resolve_profile_switch(ProfileSwitchResolution::Discard);
        let expected_old_url = old_url.clone();
        let mut active_target = Some(connection::Resolved::unowned(
            old_url,
            None,
            connection::Source::Profile("A".into()),
        ));
        let mut active_fingerprint = Some("fingerprint-a".into());
        let blocked_path = std::env::temp_dir().join(format!(
            "fleety-profile-switch-blocked-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&blocked_path).expect("create blocking directory");
        let mut config_protocol = Some(4);

        commit_profile_switch_at(
            &mut panel,
            &mut old_connection,
            &mut config_protocol,
            &mut active_target,
            &mut active_fingerprint,
            &blocked_path,
            |_| Ok("fleetyd test reconnect accepted".to_string()),
        )
        .await;

        assert_eq!(panel.conns.current.as_deref(), Some("A"));
        assert!(panel.staged.contains_key("FLEETY_POLICY"));
        assert!(panel.profile_switch_prompt.is_some());
        assert!(old_connection.is_some());
        assert_eq!(
            active_target.as_ref().map(connection::Resolved::url),
            Some(expected_old_url.as_str())
        );
        assert_eq!(active_fingerprint.as_deref(), Some("fingerprint-a"));
        assert_eq!(config_protocol, Some(4));
        assert!(panel.status.contains("staged changes retained"));
        let mut old_closed = old_closed;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut old_closed)
                .await
                .is_err(),
            "old transport must remain open when persistence fails"
        );
        if let Some((mut tx, _)) = old_connection.take() {
            tx.close().await;
        }
        assert!(old_closed.await.expect("close observation"));
        std::fs::remove_dir(&blocked_path).expect("remove blocking directory");
    }

    #[tokio::test]
    async fn profile_switch_apply_failure_stays_on_old_profile_with_typed_error() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind apply failure server");
        let address = listener.local_addr().expect("apply failure address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept apply client");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("accept websocket");
            let frame = websocket
                .next()
                .await
                .expect("apply frame")
                .expect("read apply frame");
            let request: ClientMsg =
                serde_json::from_str(frame.to_text().expect("text apply frame"))
                    .expect("parse apply request");
            assert!(matches!(
                request,
                ClientMsg::ConfigApply {
                    target: ConfigTarget::Server,
                    ..
                }
            ));
            websocket
                .send(Message::Text(
                    serde_json::to_string(&ServerMsg::ConfigResult {
                        ok: false,
                        output: String::new(),
                        effect: None,
                        error: Some(fleety_protocol::WireError {
                            kind: "conflict".into(),
                            message: "revision changed".into(),
                            remediation: Some("Reload before switching profiles".into()),
                        }),
                    })
                    .expect("serialize conflict"),
                ))
                .await
                .expect("send conflict");
        });
        let transport = fleety_tools::transport::connect(&format!("ws://{address}"), None)
            .await
            .expect("connect apply client");
        let mut connection = Some(transport.split());
        let mut panel = panel_with_entries(vec![entry("FLEETY_POLICY", "full_access", false)]);
        panel.conns.current = Some("A".into());
        panel.active_profile = "A".into();
        panel.staged.insert(
            "FLEETY_POLICY".into(),
            ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            },
        );
        panel.request_profile_switch("B".into());
        panel.resolve_profile_switch(ProfileSwitchResolution::Apply);

        assert!(
            !apply_staged_before_profile_switch(
                &mut panel,
                &mut connection,
                fleety_protocol::CONFIG_PROTOCOL_VERSION,
            )
            .await
        );

        assert_eq!(panel.conns.current.as_deref(), Some("A"));
        assert!(panel.staged.contains_key("FLEETY_POLICY"));
        assert!(panel.profile_switch_prompt.is_some());
        assert!(panel.profile_switch_pending.is_none());
        assert!(matches!(
            panel.server_apply_error.as_ref(),
            Some(error)
                if error.kind == "conflict"
                    && error.remediation.as_deref()
                        == Some("Reload before switching profiles")
        ));
        assert!(panel.status.contains("profile switch paused"));
        server.await.expect("apply failure server task");
    }

    #[tokio::test]
    async fn profile_switch_refreshes_daemon_before_server_conflict() {
        let (url, requests_received) =
            start_profile_switch_apply_server(ProfileSwitchApplyScript::ServerConflict).await;
        let transport = fleety_tools::transport::connect(&url, None)
            .await
            .expect("connect profile switch apply client");
        let mut connection = Some(transport.split());
        let mut panel = dirty_two_owner_profile_switch_panel();

        assert!(
            !apply_staged_before_profile_switch(
                &mut panel,
                &mut connection,
                fleety_protocol::CONFIG_PROTOCOL_VERSION,
            )
            .await
        );

        assert_eq!(panel.conns.current.as_deref(), Some("A"));
        assert!(panel.daemon_staged.is_empty());
        assert_eq!(panel.daemon_revision, "daemon-r2");
        assert_eq!(
            panel
                .daemon_entries
                .iter()
                .find(|entry| entry.key == "FLEETY_TZ")
                .map(|entry| entry.value.as_str()),
            Some("Asia/Taipei")
        );
        assert!(panel.daemon_apply_error.is_none());
        assert!(panel.staged.contains_key("FLEETY_POLICY"));
        assert_eq!(panel.revision, "server-r1");
        assert!(matches!(
            panel.server_apply_error.as_ref(),
            Some(error) if error.kind == "conflict"
        ));
        assert!(panel.profile_switch_prompt.is_some());
        assert!(panel.profile_switch_pending.is_none());
        let requests = requests_received.await.expect("profile switch requests");
        assert!(matches!(
            requests.as_slice(),
            [
                ClientMsg::ConfigApply {
                    target: ConfigTarget::Device(_),
                    ..
                },
                ClientMsg::ConfigSnapshot {
                    target: ConfigTarget::Device(_)
                },
                ClientMsg::ConfigApply {
                    target: ConfigTarget::Server,
                    ..
                }
            ]
        ));
    }

    #[tokio::test]
    async fn profile_switch_daemon_refresh_failure_sends_zero_server_apply() {
        let (url, requests_received) =
            start_profile_switch_apply_server(ProfileSwitchApplyScript::DaemonRefreshFailure).await;
        let transport = fleety_tools::transport::connect(&url, None)
            .await
            .expect("connect profile switch apply client");
        let mut connection = Some(transport.split());
        let mut panel = dirty_two_owner_profile_switch_panel();

        assert!(
            !apply_staged_before_profile_switch(
                &mut panel,
                &mut connection,
                fleety_protocol::CONFIG_PROTOCOL_VERSION,
            )
            .await
        );

        assert_eq!(panel.conns.current.as_deref(), Some("A"));
        assert!(panel.daemon_staged.is_empty());
        assert!(panel.daemon_refresh_required);
        assert!(!panel.daemon_supported);
        assert!(panel.daemon_revision.is_empty());
        assert!(panel.daemon_entries.is_empty());
        assert_eq!(
            panel
                .daemon_apply_error
                .as_ref()
                .map(|error| error.kind.as_str()),
            Some("refresh_required")
        );
        assert!(panel.staged.contains_key("FLEETY_POLICY"));
        assert!(panel.server_apply_error.is_none());
        assert!(panel.profile_switch_prompt.is_some());
        assert!(panel.profile_switch_pending.is_none());
        panel.resolve_profile_switch(ProfileSwitchResolution::Apply);
        assert!(
            !apply_staged_before_profile_switch(
                &mut panel,
                &mut connection,
                fleety_protocol::CONFIG_PROTOCOL_VERSION,
            )
            .await,
            "retry must stay behind the failed Daemon refresh barrier"
        );
        let requests = requests_received.await.expect("profile switch requests");
        assert!(matches!(
            requests.as_slice(),
            [
                ClientMsg::ConfigApply {
                    target: ConfigTarget::Device(_),
                    ..
                },
                ClientMsg::ConfigSnapshot {
                    target: ConfigTarget::Device(_)
                }
            ]
        ));
    }

    #[tokio::test]
    async fn profile_switch_queues_selection_only_after_both_owners_refresh() {
        let (url, requests_received) =
            start_profile_switch_apply_server(ProfileSwitchApplyScript::AllRefreshed).await;
        let transport = fleety_tools::transport::connect(&url, None)
            .await
            .expect("connect profile switch apply client");
        let mut connection = Some(transport.split());
        let mut panel = dirty_two_owner_profile_switch_panel();

        assert!(
            apply_staged_before_profile_switch(
                &mut panel,
                &mut connection,
                fleety_protocol::CONFIG_PROTOCOL_VERSION,
            )
            .await
        );

        assert_eq!(panel.conns.current.as_deref(), Some("A"));
        assert!(panel.daemon_staged.is_empty());
        assert_eq!(panel.daemon_revision, "daemon-r2");
        assert!(panel.staged.is_empty());
        assert_eq!(panel.revision, "server-r2");
        assert!(panel.profile_switch_prompt.is_none());
        assert_eq!(panel.profile_switch_pending.as_deref(), Some("B"));
        let requests = requests_received.await.expect("profile switch requests");
        assert!(matches!(
            requests.as_slice(),
            [
                ClientMsg::ConfigApply {
                    target: ConfigTarget::Device(_),
                    ..
                },
                ClientMsg::ConfigSnapshot {
                    target: ConfigTarget::Device(_)
                },
                ClientMsg::ConfigApply {
                    target: ConfigTarget::Server,
                    ..
                },
                ClientMsg::ConfigSnapshot {
                    target: ConfigTarget::Server
                }
            ]
        ));
    }

    #[tokio::test]
    async fn profile_switch_updates_live_protocol_before_the_next_dirty_switch() {
        let (new_url, requests_received) =
            start_profile_switch_apply_server(ProfileSwitchApplyScript::ReloadThenServerApply)
                .await;
        let mut conns = Connections {
            current: Some("A".into()),
            ..Default::default()
        };
        conns.profiles.insert(
            "A".into(),
            connection::Profile {
                url: "ws://127.0.0.1:9".into(),
                ..Default::default()
            },
        );
        conns.profiles.insert(
            "B".into(),
            connection::Profile {
                url: new_url,
                fingerprint: Some("fingerprint-a".into()),
                ..Default::default()
            },
        );
        let mut panel = Panel::new(
            conns,
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(false, vec![], String::new()),
            RemoteRegionState::new(false, vec![], String::new()),
        );
        panel.daemon_device_id = "remote-B".into();
        panel.request_profile_switch("B".into());
        let path = std::env::temp_dir().join(format!(
            "fleety-profile-switch-live-protocol-{}-{}.toml",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        connection::save_at(&path, &panel.persisted_conns).expect("seed old profile");
        let mut connection = None;
        let mut config_protocol = Some(1);
        let mut active_target = None;
        let mut active_fingerprint = None;

        commit_profile_switch_at(
            &mut panel,
            &mut connection,
            &mut config_protocol,
            &mut active_target,
            &mut active_fingerprint,
            &path,
            |_| Ok("fleetyd test reconnect accepted".to_string()),
        )
        .await;

        assert_eq!(
            config_protocol,
            Some(fleety_protocol::CONFIG_PROTOCOL_VERSION)
        );
        assert_eq!(panel.revision, "server-r1");
        panel.staged.insert(
            "FLEETY_POLICY".into(),
            ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            },
        );
        panel.request_profile_switch("A".into());
        panel.resolve_profile_switch(ProfileSwitchResolution::Apply);

        assert!(
            apply_staged_before_profile_switch(
                &mut panel,
                &mut connection,
                config_protocol.unwrap_or_default(),
            )
            .await
        );
        assert_eq!(panel.revision, "server-r2");
        assert_eq!(panel.profile_switch_pending.as_deref(), Some("A"));
        let requests = requests_received.await.expect("mixed protocol requests");
        assert!(matches!(
            requests.as_slice(),
            [
                ClientMsg::Hello { .. },
                ClientMsg::ConfigSnapshot {
                    target: ConfigTarget::Server
                },
                ClientMsg::ConfigSnapshot {
                    target: ConfigTarget::Device(_)
                },
                ClientMsg::ConfigApply {
                    target: ConfigTarget::Server,
                    ..
                },
                ClientMsg::ConfigSnapshot {
                    target: ConfigTarget::Server
                }
            ]
        ));
        if let Some((mut tx, _)) = connection.take() {
            tx.close().await;
        }
        std::fs::remove_file(path).expect("remove mixed protocol fixture");
    }

    #[tokio::test]
    async fn profile_switch_connects_selected_profile_and_closes_old_connection() {
        let (old_url, old_closed) = start_close_observer().await;
        let old_transport = fleety_tools::transport::connect(&old_url, None)
            .await
            .expect("connect to server B");
        let mut old_connection = Some(old_transport.split());

        let (new_url, hello_received) = start_welcome_server().await;
        let mut conns = Connections {
            current: Some("b".into()),
            ..Default::default()
        };
        conns.profiles.insert(
            "a".into(),
            connection::Profile {
                url: new_url.clone(),
                token: Some("token-a".into()),
                fingerprint: Some("fingerprint-a".into()),
                ..Default::default()
            },
        );
        let mut panel = Panel::new(
            conns.clone(),
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(false, vec![], String::new()),
            RemoteRegionState::new(false, vec![], String::new()),
        );
        on_key(&mut panel, KeyCode::Char('u'));
        assert_eq!(panel.profile_switch_pending.as_deref(), Some("a"));

        let target = resolve_saved_profile(&conns, "a").expect("resolve profile A");
        assert_eq!(target.url(), new_url);
        assert_eq!(target.token(), Some("token-a"));
        assert_eq!(
            target.source(),
            &connection::Source::OverrideProfile("a".into())
        );
        let connections_path = std::env::temp_dir().join(format!(
            "fleety-profile-switch-direct-{}-{}.toml",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        connection::save_at(&connections_path, &conns).expect("save direct switch profiles");

        let ((mut new_tx, _new_rx), config_protocol, fingerprint, _committed_target) =
            replace_profile_connection(&mut old_connection, &target, &connections_path)
                .await
                .expect("switch to profile A");

        assert!(old_connection.is_none());
        assert_eq!(config_protocol, fleety_protocol::CONFIG_PROTOCOL_VERSION);
        assert_eq!(fingerprint.as_deref(), Some("fingerprint-a"));
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), old_closed)
                .await
                .expect("server B close deadline")
                .expect("server B close observation")
        );
        match hello_received.await.expect("server A hello") {
            ClientMsg::Hello { token, .. } => assert_eq!(token.as_deref(), Some("token-a")),
            other => panic!("expected Hello for server A, got {other:?}"),
        }
        new_tx.close().await;
        std::fs::remove_file(connections_path).expect("remove direct switch profiles");
    }

    #[tokio::test]
    async fn profile_switch_reloads_owner_snapshots() {
        let (url, requests_received) = start_snapshot_server(false).await;
        let target = connection::Resolved::unowned(
            url,
            Some("token-a".into()),
            connection::Source::OverrideUrl,
        );
        let mut old_connection = None;
        let unused_path = std::env::temp_dir().join(format!(
            "fleety-profile-switch-unowned-{}-{}.toml",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let ((mut tx, mut rx), config_protocol, _fingerprint, _committed_target) =
            replace_profile_connection(&mut old_connection, &target, &unused_path)
                .await
                .expect("connect profile A");

        let (server, daemon, notes) =
            reload_remote_regions(&mut tx, &mut rx, config_protocol, "device-a").await;

        assert!(server.supported);
        assert_eq!(server.revision, "server-a-rev");
        assert_eq!(server.entries[0].key, "FLEETY_POLICY");
        assert!(!daemon.supported);
        assert!(daemon.entries.is_empty());
        assert!(daemon.revision.is_empty());
        assert!(notes.iter().any(|note| note.contains("daemon unavailable")));

        let requests = requests_received.await.expect("snapshot requests");
        assert!(matches!(requests[0], ClientMsg::Hello { .. }));
        assert!(matches!(
            requests[1],
            ClientMsg::ConfigSnapshot {
                target: ConfigTarget::Server
            }
        ));
        assert!(matches!(
            &requests[2],
            ClientMsg::ConfigSnapshot {
                target: ConfigTarget::Device(id)
            } if id == "device-a"
        ));
        tx.close().await;
    }

    #[tokio::test]
    async fn profile_switch_transaction_persists_reconnects_and_reloads_both_owners() {
        let (old_url, old_closed) = start_close_observer().await;
        let old_transport = fleety_tools::transport::connect(&old_url, None)
            .await
            .expect("connect profile A");
        let mut connection = Some(old_transport.split());
        let (new_url, requests_received) = start_snapshot_server(true).await;
        let mut conns = Connections {
            current: Some("A".into()),
            ..Default::default()
        };
        conns.profiles.insert(
            "A".into(),
            connection::Profile {
                url: old_url,
                ..Default::default()
            },
        );
        conns.profiles.insert(
            "B".into(),
            connection::Profile {
                url: new_url.clone(),
                token: Some("token-b".into()),
                ..Default::default()
            },
        );
        let mut panel = Panel::new(
            conns,
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(
                true,
                vec![entry("OLD_DAEMON", "old", false)],
                "old-daemon-rev".into(),
            ),
            RemoteRegionState::new(
                true,
                vec![entry("OLD_SERVER", "old", false)],
                "old-server-rev".into(),
            ),
        );
        panel.daemon_device_id = "remote-B".into();
        panel.request_profile_switch("B".into());
        let path = std::env::temp_dir().join(format!(
            "fleety-profile-switch-success-{}-{}.toml",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        connection::save_at(&path, &panel.persisted_conns).expect("seed persisted profile A");
        let mut active_target = Some(connection::Resolved::unowned(
            "ws://old.invalid".into(),
            None,
            connection::Source::Profile("A".into()),
        ));
        let mut active_fingerprint = Some("old-fingerprint".into());
        let mut config_protocol = Some(1);

        commit_profile_switch_at(
            &mut panel,
            &mut connection,
            &mut config_protocol,
            &mut active_target,
            &mut active_fingerprint,
            &path,
            |_| Ok("fleetyd test reconnect accepted".to_string()),
        )
        .await;

        assert_eq!(panel.conns.current.as_deref(), Some("B"));
        assert_eq!(
            connection::load_at(&path)
                .expect("load persisted selection")
                .current
                .as_deref(),
            Some("B")
        );
        assert!(connection.is_some());
        assert_eq!(
            active_target.as_ref().map(connection::Resolved::url),
            Some(new_url.as_str())
        );
        assert_eq!(active_fingerprint.as_deref(), Some("fingerprint-a"));
        assert_eq!(
            connection::load_at(&path)
                .expect("load pinned switched profile")
                .profiles["B"]
                .fingerprint
                .as_deref(),
            Some("fingerprint-a")
        );
        assert_eq!(
            config_protocol,
            Some(fleety_protocol::CONFIG_PROTOCOL_VERSION),
            "the live transport protocol must replace the startup protocol"
        );
        assert!(panel.server_supported);
        assert_eq!(panel.revision, "server-a-rev");
        assert_eq!(panel.entries[0].key, "FLEETY_POLICY");
        assert!(panel.daemon_supported);
        assert_eq!(panel.daemon_revision, "daemon-a-rev");
        assert_eq!(panel.daemon_entries[0].key, "FLEETY_TZ");
        assert!(!panel.entries.iter().any(|entry| entry.key == "OLD_SERVER"));
        assert!(!panel
            .daemon_entries
            .iter()
            .any(|entry| entry.key == "OLD_DAEMON"));
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), old_closed)
                .await
                .expect("old profile close deadline")
                .expect("old profile close observation")
        );
        let requests = requests_received.await.expect("new profile requests");
        assert!(matches!(
            &requests[0],
            ClientMsg::Hello { token, .. } if token.as_deref() == Some("token-b")
        ));
        assert!(matches!(
            requests[1],
            ClientMsg::ConfigSnapshot {
                target: ConfigTarget::Server
            }
        ));
        assert!(matches!(
            &requests[2],
            ClientMsg::ConfigSnapshot {
                target: ConfigTarget::Device(id)
            } if id == "remote-B"
        ));
        if let Some((mut tx, _)) = connection.take() {
            tx.close().await;
        }
        std::fs::remove_file(path).expect("remove persisted selection");
    }

    #[tokio::test]
    async fn profile_switch_uses_live_credentials_rotated_after_panel_opened() {
        let (new_url, requests_received) = start_snapshot_server(true).await;
        let mut conns = Connections {
            current: Some("A".into()),
            ..Default::default()
        };
        conns.profiles.insert(
            "A".into(),
            connection::Profile {
                url: "ws://a.invalid:8787".into(),
                ..Default::default()
            },
        );
        conns.profiles.insert(
            "B".into(),
            connection::Profile {
                url: new_url,
                token: Some("old-token".into()),
                fingerprint: Some("old-fingerprint".into()),
                ..Default::default()
            },
        );
        let mut panel = Panel::new(
            conns,
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(false, vec![], String::new()),
            RemoteRegionState::new(false, vec![], String::new()),
        );
        panel.daemon_device_id = "remote-B".into();
        panel.request_profile_switch("B".into());
        let path = std::env::temp_dir().join(format!(
            "fleety-profile-switch-rotation-{}-{}.toml",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        connection::save_at(&path, &panel.persisted_conns).expect("seed stale panel snapshot");
        connection::mutate_at(&path, |live| {
            let profile = live.profiles.get_mut("B").expect("live profile B");
            profile.token = Some("rotated-token".into());
            profile.fingerprint = Some("fingerprint-a".into());
            Ok(())
        })
        .expect("rotate credentials concurrently");

        let mut transport = None;
        let mut config_protocol = None;
        let mut active_target = None;
        let mut active_fingerprint = None;
        commit_profile_switch_at(
            &mut panel,
            &mut transport,
            &mut config_protocol,
            &mut active_target,
            &mut active_fingerprint,
            &path,
            |_| Ok("fleetyd test reconnect accepted".to_string()),
        )
        .await;

        assert_eq!(panel.conns.current.as_deref(), Some("B"));
        assert_eq!(
            panel.conns.profiles["B"].fingerprint.as_deref(),
            Some("fingerprint-a")
        );
        assert_eq!(
            active_target.as_ref().and_then(connection::Resolved::token),
            Some("rotated-token")
        );
        assert_eq!(
            config_protocol,
            Some(fleety_protocol::CONFIG_PROTOCOL_VERSION)
        );
        let requests = requests_received.await.expect("new profile requests");
        assert!(matches!(
            &requests[0],
            ClientMsg::Hello { token, .. } if token.as_deref() == Some("rotated-token")
        ));
        if let Some((mut tx, _)) = transport.take() {
            tx.close().await;
        }
        std::fs::remove_file(path).expect("remove rotated profile fixture");
    }

    #[tokio::test]
    async fn profile_switch_failure_never_reuses_old_connection() {
        let (old_url, old_closed) = start_close_observer().await;
        let old_transport = fleety_tools::transport::connect(&old_url, None)
            .await
            .expect("connect to server B");
        let mut old_connection = Some(old_transport.split());

        let unavailable = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("reserve unavailable address");
        let address = unavailable.local_addr().expect("unavailable address");
        drop(unavailable);
        let target = connection::Resolved::unowned(
            format!("ws://{address}"),
            None,
            connection::Source::Profile("a".into()),
        );

        let unused_path = std::env::temp_dir().join(format!(
            "fleety-profile-switch-failure-{}-{}.toml",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let result = replace_profile_connection(&mut old_connection, &target, &unused_path).await;

        assert!(result.is_err());
        assert!(old_connection.is_none());
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), old_closed)
                .await
                .expect("server B close deadline")
                .expect("server B close observation")
        );
    }

    #[tokio::test]
    async fn profile_switch_transaction_keeps_new_selection_but_never_old_state_on_reconnect_failure(
    ) {
        let (old_url, old_closed) = start_close_observer().await;
        let old_transport = fleety_tools::transport::connect(&old_url, None)
            .await
            .expect("connect profile A");
        let mut connection = Some(old_transport.split());
        let unavailable = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("reserve unavailable address");
        let address = unavailable.local_addr().expect("unavailable address");
        drop(unavailable);
        let mut conns = Connections {
            current: Some("A".into()),
            ..Default::default()
        };
        conns.profiles.insert(
            "A".into(),
            connection::Profile {
                url: old_url,
                ..Default::default()
            },
        );
        conns.profiles.insert(
            "B".into(),
            connection::Profile {
                url: format!("ws://{address}"),
                ..Default::default()
            },
        );
        let mut panel = Panel::new(
            conns,
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(
                true,
                vec![entry("OLD_DAEMON", "old", false)],
                "old-daemon-rev".into(),
            ),
            RemoteRegionState::new(
                true,
                vec![entry("OLD_SERVER", "old", false)],
                "old-server-rev".into(),
            ),
        );
        panel.request_profile_switch("B".into());
        let path = std::env::temp_dir().join(format!(
            "fleety-profile-switch-connect-failure-{}-{}.toml",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        connection::save_at(&path, &panel.persisted_conns).expect("seed persisted profile A");
        let mut active_target = Some(connection::Resolved::unowned(
            "ws://old.invalid".into(),
            None,
            connection::Source::Profile("A".into()),
        ));
        let mut active_fingerprint = Some("old-fingerprint".into());
        let mut config_protocol = Some(fleety_protocol::CONFIG_PROTOCOL_VERSION);
        let mut daemon_notified = false;

        commit_profile_switch_at(
            &mut panel,
            &mut connection,
            &mut config_protocol,
            &mut active_target,
            &mut active_fingerprint,
            &path,
            |_| {
                daemon_notified = true;
                Ok("unexpected notification".to_string())
            },
        )
        .await;

        assert_eq!(panel.conns.current.as_deref(), Some("B"));
        assert_eq!(
            connection::load_at(&path)
                .expect("load persisted selection")
                .current
                .as_deref(),
            Some("B")
        );
        assert!(connection.is_none());
        assert!(active_target.is_none());
        assert!(active_fingerprint.is_none());
        assert!(config_protocol.is_none());
        assert!(!panel.server_supported);
        assert!(panel.entries.is_empty());
        assert!(panel.revision.is_empty());
        assert!(!panel.daemon_supported);
        assert!(panel.daemon_entries.is_empty());
        assert!(panel.daemon_revision.is_empty());
        assert!(panel
            .status
            .contains("previous CLI connection and snapshots were not reused"));
        assert!(
            daemon_notified,
            "persisting B must tell fleetyd to leave A even when the CLI handshake fails"
        );
        assert!(panel.status.contains("unexpected notification"));
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(2), old_closed)
                .await
                .expect("old profile close deadline")
                .expect("old profile close observation")
        );
        std::fs::remove_file(path).expect("remove persisted selection");
    }

    #[tokio::test]
    async fn owner_apply_preserves_conflict_kind_and_targets_only_that_owner() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind apply server");
        let address = listener.local_addr().expect("apply address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept apply client");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("accept websocket");
            let frame = websocket
                .next()
                .await
                .expect("apply frame")
                .expect("read apply frame");
            let request: ClientMsg =
                serde_json::from_str(frame.to_text().expect("text apply frame"))
                    .expect("parse apply request");
            assert!(matches!(
                request,
                ClientMsg::ConfigApply {
                    target: ConfigTarget::Server,
                    ..
                }
            ));
            websocket
                .send(Message::Text(
                    serde_json::to_string(&ServerMsg::ConfigResult {
                        ok: false,
                        output: String::new(),
                        effect: None,
                        error: Some(fleety_protocol::WireError {
                            kind: "conflict".into(),
                            message: "revision changed".into(),
                            remediation: Some("Reload or retry".into()),
                        }),
                    })
                    .expect("serialize conflict"),
                ))
                .await
                .expect("send conflict");
        });
        let connection = fleety_tools::transport::connect(&format!("ws://{address}"), None)
            .await
            .expect("connect apply client");
        let (mut tx, mut rx) = connection.split();

        let error = apply_changes(
            &mut tx,
            &mut rx,
            ConfigTarget::Server,
            "old-revision",
            vec![ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            }],
        )
        .await
        .expect_err("conflict must stay typed");

        assert_eq!(error.kind, "conflict");
        assert_eq!(error.message, "revision changed");
        assert_eq!(error.remediation.as_deref(), Some("Reload or retry"));
        server.await.expect("apply server task");
    }

    #[derive(Clone, Copy)]
    enum SnapshotFailureReply {
        Error,
        Close,
        WrongReply,
    }

    async fn owner_refresh_failure(region: Region, reply: SnapshotFailureReply) -> Panel {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind owner refresh server");
        let address = listener.local_addr().expect("owner refresh address");
        let target = if region == Region::Server {
            ConfigTarget::Server
        } else {
            ConfigTarget::Device("remote-B".into())
        };
        let expected_target = target.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept refresh client");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("accept refresh websocket");
            for step in 0..2 {
                let frame = websocket
                    .next()
                    .await
                    .expect("owner request frame")
                    .expect("read owner request frame");
                let request: ClientMsg =
                    serde_json::from_str(frame.to_text().expect("owner request is text"))
                        .expect("parse owner request");
                if step == 0 {
                    assert!(matches!(
                        request,
                        ClientMsg::ConfigApply { target, .. } if target == expected_target
                    ));
                    websocket
                        .send(Message::Text(
                            serde_json::to_string(&ServerMsg::ConfigResult {
                                ok: true,
                                output: "Saved".into(),
                                effect: None,
                                error: None,
                            })
                            .expect("serialize apply success"),
                        ))
                        .await
                        .expect("send apply success");
                    continue;
                }
                assert!(matches!(
                    request,
                    ClientMsg::ConfigSnapshot { target } if target == expected_target
                ));
                match reply {
                    SnapshotFailureReply::Error => websocket
                        .send(Message::Text(
                            serde_json::to_string(&ServerMsg::Error {
                                error: fleety_protocol::WireError {
                                    kind: "snapshot_failed".into(),
                                    message: "fresh snapshot unavailable".into(),
                                    remediation: None,
                                },
                            })
                            .expect("serialize snapshot error"),
                        ))
                        .await
                        .expect("send snapshot error"),
                    SnapshotFailureReply::WrongReply => websocket
                        .send(Message::Text(
                            serde_json::to_string(&ServerMsg::ConfigResult {
                                ok: true,
                                output: "not a snapshot".into(),
                                effect: None,
                                error: None,
                            })
                            .expect("serialize wrong snapshot reply"),
                        ))
                        .await
                        .expect("send wrong snapshot reply"),
                    SnapshotFailureReply::Close => {}
                }
            }
        });
        let connection = fleety_tools::transport::connect(&format!("ws://{address}"), None)
            .await
            .expect("connect owner refresh client");
        let (mut tx, mut rx) = connection.split();
        let mut panel = Panel::new(
            Connections::default(),
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(
                true,
                vec![entry("FLEETY_TZ", "UTC", false)],
                "daemon-r1".into(),
            ),
            RemoteRegionState::new(
                true,
                vec![entry("FLEETY_POLICY", "full_access", false)],
                "server-r1".into(),
            ),
        );
        panel.daemon_device_id = "remote-B".into();
        panel.region = region;
        let change = ConfigChange {
            key: if region == Region::Server {
                "FLEETY_POLICY".into()
            } else {
                "FLEETY_TZ".into()
            },
            op: ChangeOp::Set,
            value: Some("changed".into()),
        };
        if region == Region::Server {
            panel.staged.insert(change.key.clone(), change.clone());
        } else {
            panel
                .daemon_staged
                .insert(change.key.clone(), change.clone());
        }
        let base_revision = if region == Region::Server {
            panel.revision.clone()
        } else {
            panel.daemon_revision.clone()
        };
        let outcome =
            apply_and_refresh(&mut tx, &mut rx, target, &base_revision, vec![change], 5).await;
        panel.finish_remote_apply(region, outcome);
        server.await.expect("owner refresh server task");
        panel
    }

    fn assert_refresh_barrier(mut panel: Panel, region: Region) {
        assert!(panel.status.contains("applied"), "{}", panel.status);
        assert!(panel.status.contains("refresh failed"), "{}", panel.status);
        assert!(panel.status.contains("reopen"), "{}", panel.status);
        assert_eq!(region_state_label(&panel, region), "reload required");
        assert_eq!(
            active_owner_error(&panel).map(|error| error.kind.as_str()),
            Some("refresh_required")
        );
        if region == Region::Server {
            assert!(panel.staged.is_empty());
            assert!(panel.server_refresh_required);
            assert!(panel.revision.is_empty());
            assert!(panel.entries.is_empty());
            assert!(!panel.server_supported);
        } else {
            assert!(panel.daemon_staged.is_empty());
            assert!(panel.daemon_refresh_required);
            assert!(panel.daemon_revision.is_empty());
            assert!(panel.daemon_entries.is_empty());
            assert!(!panel.daemon_supported);
        }
        on_key(&mut panel, KeyCode::Enter);
        on_key(&mut panel, KeyCode::Char('a'));
        assert!(panel.edit.is_none());
        assert!(!panel.apply_now);
        assert!(!panel.apply_daemon_now);
    }

    #[tokio::test]
    async fn server_snapshot_error_after_apply_requires_reopen() {
        assert_refresh_barrier(
            owner_refresh_failure(Region::Server, SnapshotFailureReply::Error).await,
            Region::Server,
        );
    }

    #[tokio::test]
    async fn server_snapshot_close_after_apply_requires_reopen() {
        assert_refresh_barrier(
            owner_refresh_failure(Region::Server, SnapshotFailureReply::Close).await,
            Region::Server,
        );
    }

    #[tokio::test]
    async fn server_snapshot_wrong_reply_after_apply_requires_reopen() {
        assert_refresh_barrier(
            owner_refresh_failure(Region::Server, SnapshotFailureReply::WrongReply).await,
            Region::Server,
        );
    }

    #[tokio::test]
    async fn daemon_snapshot_error_after_apply_requires_reopen() {
        assert_refresh_barrier(
            owner_refresh_failure(Region::Daemon, SnapshotFailureReply::Error).await,
            Region::Daemon,
        );
    }

    #[tokio::test]
    async fn daemon_snapshot_close_after_apply_requires_reopen() {
        assert_refresh_barrier(
            owner_refresh_failure(Region::Daemon, SnapshotFailureReply::Close).await,
            Region::Daemon,
        );
    }

    #[tokio::test]
    async fn daemon_snapshot_wrong_reply_after_apply_requires_reopen() {
        assert_refresh_barrier(
            owner_refresh_failure(Region::Daemon, SnapshotFailureReply::WrongReply).await,
            Region::Daemon,
        );
    }

    #[tokio::test]
    async fn daemon_apply_preserves_the_explicit_remote_device_owner() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind apply server");
        let address = listener.local_addr().expect("apply address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept apply client");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("accept websocket");
            let frame = websocket
                .next()
                .await
                .expect("apply frame")
                .expect("read apply frame");
            let request: ClientMsg =
                serde_json::from_str(frame.to_text().expect("text apply frame"))
                    .expect("parse apply request");
            assert!(matches!(
                request,
                ClientMsg::ConfigApply {
                    target: ConfigTarget::Device(ref id),
                    ..
                } if id == "remote-B"
            ));
            websocket
                .send(Message::Text(
                    serde_json::to_string(&ServerMsg::ConfigResult {
                        ok: true,
                        output: "saved".into(),
                        effect: None,
                        error: None,
                    })
                    .expect("serialize result"),
                ))
                .await
                .expect("send result");
        });
        let connection = fleety_tools::transport::connect(&format!("ws://{address}"), None)
            .await
            .expect("connect apply client");
        let (mut tx, mut rx) = connection.split();

        apply_changes(
            &mut tx,
            &mut rx,
            ConfigTarget::Device("remote-B".into()),
            "remote-revision",
            vec![ConfigChange {
                key: "FLEETY_TZ".into(),
                op: ChangeOp::Set,
                value: Some("Asia/Taipei".into()),
            }],
        )
        .await
        .expect("remote device apply");
        server.await.expect("apply server task");
    }

    #[test]
    fn owner_apply_success_names_effect_timing() {
        assert_eq!(
            OwnerApplySuccess {
                message: "Saved".into(),
                effect: Some(fleety_protocol::Effect::Restart),
            }
            .display(),
            "Saved · restart required"
        );
        assert_eq!(
            OwnerApplySuccess {
                message: "Saved".into(),
                effect: Some(fleety_protocol::Effect::NextConnection),
            }
            .display(),
            "Saved · takes effect on the next connection"
        );
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
    fn tab_cycles_all_owner_aware_pages() {
        let mut p = panel_with_entries(vec![]);
        assert!(matches!(p.region, Region::Connection));
        on_key(&mut p, KeyCode::Tab);
        assert!(matches!(p.region, Region::Cli));
        on_key(&mut p, KeyCode::Tab);
        assert!(matches!(p.region, Region::Daemon));
        on_key(&mut p, KeyCode::Tab);
        assert!(matches!(p.region, Region::Server));
        on_key(&mut p, KeyCode::Tab);
        assert!(matches!(p.region, Region::ProvidersAndModels));
        on_key(&mut p, KeyCode::Tab);
        assert!(matches!(p.region, Region::Connection));
    }

    #[test]
    fn connection_region_marks_urlless_profile_as_requiring_init() {
        let label = connection_endpoint_label(&connection::Profile::default());
        assert!(label.contains("endpoint required"), "{label}");
        assert!(label.contains("fleety init"), "{label}");
        assert!(!label.contains("mDNS"), "{label}");
    }

    #[test]
    fn cli_edit_is_staged_until_its_owner_is_applied() {
        let mut map = fleety_tools::config::ConfigMap::new();
        map.insert(
            (
                fleety_tools::config::Scope::Cli,
                "FLEETY_VOICE_AUDIO".into(),
            ),
            "auto".into(),
        );
        let mut panel = Panel::new(
            Connections::default(),
            map,
            RemoteRegionState::new(false, vec![], String::new()),
            RemoteRegionState::new(false, vec![], String::new()),
        );
        panel.region = Region::Cli;
        panel.sel = panel
            .local
            .iter()
            .position(|row| row.0 == "FLEETY_VOICE_AUDIO")
            .expect("CLI row");
        assert!(!panel.commit_edit("off".into()));
        assert!(panel.local_dirty);
        assert!(!panel.apply_cli_now);
        on_key(&mut panel, KeyCode::Char('a'));
        assert!(panel.apply_cli_now);
        assert_eq!(
            panel
                .local_map
                .get(&(
                    fleety_tools::config::Scope::Cli,
                    "FLEETY_VOICE_AUDIO".into()
                ))
                .map(String::as_str),
            Some("off")
        );
    }

    #[test]
    fn cli_owner_apply_is_the_only_write_and_preserves_server_scope() {
        let path = std::env::temp_dir().join(format!(
            "fleety-settings-cli-owner-{}-{}.toml",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let mut seed = fleety_tools::config::ConfigMap::new();
        seed.insert(
            (
                fleety_tools::config::Scope::Cli,
                "FLEETY_VOICE_AUDIO".into(),
            ),
            "auto".into(),
        );
        seed.insert(
            (fleety_tools::config::Scope::Server, "FLEETY_MODEL".into()),
            "gpt-server".into(),
        );
        fleety_tools::config::save(&path, &seed).expect("seed config");
        let before = std::fs::read(&path).expect("before bytes");
        let mut panel = Panel::new(
            Connections::default(),
            seed,
            RemoteRegionState::new(false, vec![], String::new()),
            RemoteRegionState::new(false, vec![], String::new()),
        );
        panel.region = Region::Cli;
        panel.sel = panel
            .local
            .iter()
            .position(|row| row.0 == "FLEETY_VOICE_AUDIO")
            .expect("CLI row");

        panel.commit_edit("off".into());
        assert_eq!(std::fs::read(&path).expect("staged bytes"), before);

        crate::config::apply_cli_owner(&path, &panel.local_map).expect("apply CLI owner");
        let applied = fleety_tools::config::load_strict(&path).expect("load applied config");
        assert_eq!(
            applied.get(&(
                fleety_tools::config::Scope::Cli,
                "FLEETY_VOICE_AUDIO".into()
            )),
            Some(&"off".to_string())
        );
        assert_eq!(
            applied.get(&(fleety_tools::config::Scope::Server, "FLEETY_MODEL".into())),
            Some(&"gpt-server".to_string())
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn connection_url_edit_clears_old_credential_before_repair() {
        let mut connections = Connections {
            current: Some("office".into()),
            ..Default::default()
        };
        connections.profiles.insert(
            "office".into(),
            connection::Profile {
                url: "ws://old.test:8787".into(),
                token: Some("old-token".into()),
                fingerprint: Some("old-pin".into()),
                ..Default::default()
            },
        );
        let mut panel = Panel::new(
            connections,
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(false, vec![], String::new()),
            RemoteRegionState::new(false, vec![], String::new()),
        );

        assert!(!panel.commit_edit("http://wrong.test".into()));
        let untouched = &panel.conns.profiles["office"];
        assert_eq!(untouched.url, "ws://old.test:8787");
        assert_eq!(untouched.token.as_deref(), Some("old-token"));
        assert_eq!(untouched.fingerprint.as_deref(), Some("old-pin"));
        assert!(panel.status.contains("ws:// or wss://"), "{}", panel.status);

        assert!(!panel.commit_edit("ws://new.test:8787".into()));
        let edited = &panel.conns.profiles["office"];
        assert_eq!(edited.url, "ws://new.test:8787");
        assert_eq!(edited.token, None);
        assert_eq!(edited.fingerprint, None);
        assert!(panel.status.contains("re-pair"), "{}", panel.status);
    }

    #[test]
    fn connection_url_save_uses_the_live_current_profile_for_reconnect() {
        let path = std::env::temp_dir().join(format!(
            "fleety-settings-live-current-{}.toml",
            uuid::Uuid::new_v4()
        ));
        let mut baseline = Connections {
            current: Some("A".into()),
            ..Default::default()
        };
        baseline.profiles.insert(
            "A".into(),
            connection::Profile {
                url: "ws://a.test:8787".into(),
                ..Default::default()
            },
        );
        baseline.profiles.insert(
            "B".into(),
            connection::Profile {
                url: "ws://b-old.test:8787".into(),
                token: Some("b-token".into()),
                fingerprint: Some("b-pin".into()),
                ..Default::default()
            },
        );
        let mut pending = baseline.clone();
        connection::reselect_profile_endpoint(
            pending.profiles.get_mut("B").expect("pending B"),
            "ws://b-new.test:8787".into(),
        );
        let mut live = baseline.clone();
        live.current = Some("B".into());
        connection::save_at(&path, &live).expect("seed concurrent current B");

        let (saved, reconnect) =
            save_connection_url_edits_at(&path, &pending, &baseline).expect("save B URL");

        assert_eq!(reconnect.as_deref(), Some("B"));
        assert_eq!(saved.current.as_deref(), Some("B"));
        assert_eq!(saved.profiles["B"].url, "ws://b-new.test:8787");
        assert_eq!(saved.profiles["B"].token, None);
        assert_eq!(saved.profiles["B"].fingerprint, None);
        std::fs::remove_file(path).expect("remove Settings fixture");
    }

    #[test]
    fn settings_render_names_owner_profile_and_dirty_pages_without_storage_files() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut connections = Connections {
            current: Some("office".into()),
            ..Default::default()
        };
        connections.profiles.insert(
            "office".into(),
            connection::Profile {
                url: "ws://office.test:8787".into(),
                ..Default::default()
            },
        );
        let mut panel = Panel::new(
            connections,
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(false, vec![], String::new()),
            RemoteRegionState::new(true, vec![], "r1".into()),
        );
        panel.region = Region::ProvidersAndModels;
        panel.staged.insert(
            "FLEETY_POLICY".into(),
            ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            },
        );
        let mut terminal = Terminal::new(TestBackend::new(110, 20)).expect("terminal");
        terminal
            .draw(|frame| render_in_area(frame, &panel, frame.area()))
            .expect("draw");
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(content.contains("Providers & Models"), "{content}");
        assert!(content.contains("office"), "{content}");
        assert!(content.contains("ws://office.test:8787"), "{content}");
        assert!(content.contains("Server [dirty]"), "{content}");
        assert!(!content.contains("providers.toml"), "{content}");
    }

    #[test]
    fn settings_content_is_safe_at_supported_sizes_with_unicode_and_long_endpoint() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut connections = Connections {
            current: Some("辦公室🚀".into()),
            ..Default::default()
        };
        connections.profiles.insert(
            "辦公室🚀".into(),
            connection::Profile {
                url: "wss://非常長的伺服器端點.example.test:8787/設定路徑".into(),
                ..Default::default()
            },
        );
        let mut panel = Panel::new(
            connections,
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(false, vec![], String::new()),
            RemoteRegionState::new(true, vec![], "修訂-α".into()),
        );
        panel.region = Region::ProvidersAndModels;

        for (width, height) in [(120, 30), (80, 24), (50, 16)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
            terminal
                .draw(|frame| {
                    crate::workspace::render(
                        frame,
                        &crate::workspace::WorkspaceState::new(crate::workspace::Route::Settings(
                            crate::workspace::SettingsPage::ProvidersAndModels,
                        )),
                        |frame, area| render_in_area(frame, &panel, area),
                    );
                })
                .expect("draw responsive Settings");
            let content = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(
                content.contains("Providers & Models"),
                "{width}x{height}: {content}"
            );
            assert!(!content.contains('�'), "{width}x{height}: {content}");
        }
    }

    #[test]
    fn settings_render_redacts_endpoint_secrets_controls_and_names_remote_device() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let profile_name = "office\u{1b}]52;c;STEAL\u{7}\nnext";
        let mut connections = Connections {
            current: Some(profile_name.into()),
            ..Default::default()
        };
        connections.profiles.insert(
            profile_name.into(),
            connection::Profile {
                url: "wss://user:pass@example.test/path?token=SECRET#fragment".into(),
                ..Default::default()
            },
        );
        let mut panel = Panel::new(
            connections,
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(true, vec![], "daemon-rev".into()),
            RemoteRegionState::new(true, vec![], "server-rev".into()),
        );
        panel.daemon_device_id = "remote-B\nforged".into();
        panel.region = Region::Daemon;
        panel.status = "server said wss://u:p@host/x?token=NOTICE#tail\u{1b}[31m".into();

        let mut terminal = Terminal::new(TestBackend::new(120, 24)).expect("terminal");
        terminal
            .draw(|frame| render_in_area(frame, &panel, frame.area()))
            .expect("draw");
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        for secret in ["pass", "SECRET", "NOTICE", "#fragment"] {
            assert!(!content.contains(secret), "leaked {secret}: {content}");
        }
        assert!(!content.contains('\u{1b}'), "{content}");
        assert!(!content.contains('\u{7}'), "{content}");
        assert!(
            content.contains("\\u{1b}]52;c;STEAL\\u{7}\\nnext"),
            "{content}"
        );
        assert!(content.contains("remote-B\\nforged"), "{content}");
        assert!(content.contains("token=<redacted>"), "{content}");
    }

    #[test]
    fn workspace_owner_states_keep_conflict_dirty_and_unavailable_separate() {
        let mut panel = panel_with_entries(vec![entry("FLEETY_POLICY", "full_access", false)]);
        panel.staged.insert(
            "FLEETY_POLICY".into(),
            ConfigChange {
                key: "FLEETY_POLICY".into(),
                op: ChangeOp::Set,
                value: Some("require_approval".into()),
            },
        );
        panel.server_apply_error = Some(crate::workspace::WorkspaceError {
            kind: "conflict".into(),
            message: "revision changed".into(),
            remediation: Some("Reload or retry".into()),
        });
        panel.local_dirty = true;
        let mut workspace = crate::workspace::WorkspaceState::new(
            crate::workspace::Route::Settings(crate::workspace::SettingsPage::Server),
        );

        sync_workspace_from_panel(&mut workspace, &panel);

        assert!(matches!(
            workspace.owners.get(&crate::workspace::Owner::Server),
            Some(crate::workspace::OwnerState::Conflict(_, error))
                if error.kind == "conflict"
        ));
        assert!(matches!(
            workspace.owners.get(&crate::workspace::Owner::Cli),
            Some(crate::workspace::OwnerState::Dirty(_))
        ));
        assert!(matches!(
            workspace.owners.get(&crate::workspace::Owner::Daemon),
            Some(crate::workspace::OwnerState::Unavailable(_))
        ));
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
    fn tz_candidates_offers_device_and_filters() {
        let all = tz_candidates("");
        assert_eq!(all.first().copied(), Some(TZ_DEVICE_LABEL));
        assert!(all.contains(&"Asia/Taipei"));
        assert!(all.contains(&"UTC"));
        // Substring filter is case-insensitive and drops the device option.
        assert_eq!(tz_candidates("TAIPEI"), vec!["Asia/Taipei"]);
        // "device" narrows to just the device option (no zone contains it).
        assert_eq!(tz_candidates("device"), vec![TZ_DEVICE_LABEL]);
        // No match → empty (picker then commits the typed text).
        assert!(tz_candidates("zzz-nowhere").is_empty());
    }

    #[test]
    fn fleety_tz_edit_opens_picker_and_commits_zone() {
        let mut p = Panel::new(
            Connections::default(),
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(
                true,
                vec![entry("FLEETY_TZ", "UTC", false)],
                "daemon-rev".into(),
            ),
            RemoteRegionState::new(false, vec![], String::new()),
        );
        p.region = Region::Daemon;
        p.sel = 0;
        // Enter opens the timezone picker, not a free-text editor.
        assert!(!on_key(&mut p, KeyCode::Enter));
        assert!(p.tz_pick.is_some());
        assert!(p.edit.is_none());
        for c in "taipei".chars() {
            on_key(&mut p, KeyCode::Char(c));
        }
        // Enter commits the single match into daemon staging, never a local file.
        assert!(!on_key(&mut p, KeyCode::Enter));
        assert!(p.tz_pick.is_none());
        assert_eq!(
            p.daemon_staged
                .get("FLEETY_TZ")
                .and_then(|change| change.value.as_deref()),
            Some("Asia/Taipei")
        );
    }

    #[test]
    fn fleety_tz_device_option_clears_to_follow_the_machine() {
        let mut p = Panel::new(
            Connections::default(),
            fleety_tools::config::ConfigMap::new(),
            RemoteRegionState::new(
                true,
                vec![entry("FLEETY_TZ", "Asia/Taipei", false)],
                "daemon-rev".into(),
            ),
            RemoteRegionState::new(false, vec![], String::new()),
        );
        p.region = Region::Daemon;
        p.sel = 0;
        on_key(&mut p, KeyCode::Enter); // open picker (empty filter → device at index 0)
        assert!(!on_key(&mut p, KeyCode::Enter)); // choose device → stages clear
        assert_eq!(
            p.daemon_staged.get("FLEETY_TZ").map(|change| change.op),
            Some(ChangeOp::Clear)
        );
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
