//! Interactive `fleety provider edit` screen (CLI-only; needs a TTY).
//!
//! Lists a Server-owned provider snapshot and its `main`/`cheap` model roles.
//! Changes are staged in memory, validated through the same provider service as
//! the non-interactive `fleety provider|model` commands, and sent back to the
//! connected Server on save. The CLI never writes the provider configuration
//! file directly. Provider keys are masked on screen.
//!
//! The state mutations live on [`ProviderEditor`] as small, pure methods that
//! are unit-tested; the ratatui render + key loop around them is thin.

use agent_core::{CoreError, Result};
use fleety_tools::providers_config::{self as pc, Member, ProvidersConfig, Strategy};
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use std::collections::{BTreeMap, BTreeSet};

use crate::input::LineEditor;
use crate::provider_service::{
    catalog_gate, catalog_label, provider_views, AuthState, CatalogRequest, CatalogState,
    ModelSelection, ProviderIssue,
};

/// An editable in-memory Server snapshot. Mutations use the shared provider
/// service, so command and TUI validation cannot diverge.
pub struct ProviderEditor {
    cfg: ProvidersConfig,
    key_present: BTreeSet<String>,
    clear_keys: BTreeSet<String>,
}

impl ProviderEditor {
    #[cfg(test)]
    pub fn new(cfg: ProvidersConfig) -> Self {
        Self::with_key_presence(cfg, BTreeSet::new())
    }

    pub fn with_key_presence(cfg: ProvidersConfig, key_present: BTreeSet<String>) -> Self {
        Self {
            cfg,
            key_present,
            clear_keys: BTreeSet::new(),
        }
    }

    /// The provider names in display order (BTreeMap → sorted).
    pub fn provider_names(&self) -> Vec<String> {
        self.cfg.providers.keys().cloned().collect()
    }

    /// Add a type-tagged provider; a duplicate name is rejected. Type field
    /// rules (api needs base_url; oauth carries none) are enforced at [`save`].
    pub fn add_provider(
        &mut self,
        name: String,
        kind: String,
        base_url: Option<String>,
        key: Option<String>,
    ) -> std::result::Result<(), ProviderIssue> {
        let sets_key = key.is_some();
        crate::provider_service::add_provider(&mut self.cfg, name.clone(), kind, base_url, key)?;
        if sets_key {
            self.key_present.insert(name);
        }
        Ok(())
    }

    /// Upsert a provider: replace an existing entry (edit) or add a new one. No
    /// duplicate guard (unlike [`add_provider`](Self::add_provider)) — this is the
    /// edit path, where the name already exists. Field-shape rules (api needs
    /// base_url; oauth carries none) are enforced at [`save`].
    pub fn set_provider(
        &mut self,
        name: String,
        kind: String,
        base_url: Option<String>,
        key: Option<String>,
    ) -> std::result::Result<(), ProviderIssue> {
        let replaces_key = key.is_some();
        crate::provider_service::set_provider(
            &mut self.cfg,
            &name,
            Some(kind),
            base_url,
            key,
            false,
        )?;
        let allows_key = self
            .cfg
            .providers
            .get(&name)
            .and_then(|provider| pc::provider_type(&provider.kind))
            .is_some_and(|kind| kind.allows_key);
        if !allows_key {
            self.clear_keys.remove(&name);
            self.key_present.remove(&name);
        } else if replaces_key {
            self.clear_keys.remove(&name);
            self.key_present.insert(name);
        }
        Ok(())
    }

    /// Stage an explicit API-key removal. This intent stays separate from the
    /// redacted `None` in Server snapshots, which means Keep.
    pub fn clear_provider_key(&mut self, name: &str) -> std::result::Result<(), ProviderIssue> {
        let provider = self.cfg.providers.get_mut(name).ok_or_else(|| {
            ProviderIssue::new(
                "not_found",
                format!("No Provider named '{name}'"),
                Some("Select an existing API Provider"),
            )
        })?;
        let allows_key = pc::provider_types()
            .iter()
            .find(|kind| kind.name.eq_ignore_ascii_case(&provider.kind))
            .is_some_and(|kind| kind.allows_key);
        if !allows_key {
            return Err(ProviderIssue::new(
                "not_applicable",
                format!("Provider '{name}' does not use an API key"),
                Some("Use the OAuth actions for this Provider"),
            ));
        }
        provider.key = None;
        self.clear_keys.insert(name.to_string());
        self.key_present.remove(name);
        Ok(())
    }

    /// Remove a provider; rejected if a model role member still references it
    /// (the error names the role).
    pub fn remove_provider(&mut self, name: &str) -> std::result::Result<(), ProviderIssue> {
        crate::provider_service::remove_provider(&mut self.cfg, name)?;
        self.clear_keys.remove(name);
        self.key_present.remove(name);
        Ok(())
    }

    /// Create or replace a model role's member pool.
    pub fn set_model(
        &mut self,
        role: String,
        members: Vec<Member>,
        strategy: Strategy,
    ) -> std::result::Result<(), ProviderIssue> {
        crate::provider_service::set_model(&mut self.cfg, role, members, strategy)
    }

    /// Unset a model role; an undefined role is reported by name.
    pub fn unset_model(&mut self, role: &str) -> Result<()> {
        crate::provider_service::unset_model(&mut self.cfg, role)
            .map_err(crate::provider_service::issue_as_error)
    }

    /// The current edited configuration (for savers that ship it elsewhere,
    /// e.g. the remote apply).
    pub fn config(&self) -> &ProvidersConfig {
        &self.cfg
    }

    pub fn clear_keys(&self) -> &BTreeSet<String> {
        &self.clear_keys
    }

    pub fn key_present(&self) -> &BTreeSet<String> {
        &self.key_present
    }

    fn finish_save(&mut self) {
        for provider in self.cfg.providers.values_mut() {
            provider.key = None;
        }
        self.clear_keys.clear();
    }

    fn redact_secrets(&self, text: &str) -> String {
        crate::provider_service::redact_provider_secrets(text, &self.cfg)
    }
}

/// What a save attempt did. The remote saver reports a concurrent-edit
/// conflict as data (not an error) so the editor can retain the staged edit
/// without overwriting a newer snapshot.
pub enum SaveOutcome {
    Saved,
    /// `ConfigApply` succeeded, but the editor could not obtain the fresh
    /// revision required for another safe edit. The applied graph is no longer
    /// dirty, while this editor session becomes exit-only.
    SavedRefreshRequired(String),
    Conflict(String),
}

/// Parse `p1/m1|p2/m2` into members. Test-only: the guided wizard builds a
/// single member directly; multi-member pools use the non-interactive
/// `config model` subcommand.
#[cfg(test)]
fn parse_members(s: &str) -> Result<Vec<Member>> {
    let mut out = Vec::new();
    for spec in s.split('|').map(str::trim).filter(|s| !s.is_empty()) {
        let (provider, model) = spec.split_once('/').ok_or_else(|| {
            CoreError::Message(format!("member '{spec}' must be <provider>/<model>"))
        })?;
        out.push(Member {
            provider: provider.to_string(),
            model: model.to_string(),
            stream: false,
            modalities: None,
            effort: None,
        });
    }
    if out.is_empty() {
        return Err(CoreError::Message(
            "need at least one member <provider>/<model>".to_string(),
        ));
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(test)]
enum ModelFetchTarget {
    Api {
        base_url: String,
        key: Option<String>,
    },
    Server {
        provider: String,
    },
}

/// Select the discovery authority before touching a provider endpoint. OAuth
/// providers have no `base_url`; remote editors use the server authority.
#[cfg(test)]
fn model_fetch_target(
    provider_name: &str,
    provider: &pc::Provider,
    server_discovery: bool,
) -> std::result::Result<ModelFetchTarget, String> {
    if provider.kind == "oauth:codex" {
        return if server_discovery {
            Ok(ModelFetchTarget::Server {
                provider: provider_name.to_string(),
            })
        } else {
            Err("OAuth model discovery requires a connected server".to_string())
        };
    }
    let base_url = provider
        .base_url
        .clone()
        .ok_or_else(|| "provider has no base_url".to_string())?;
    Ok(ModelFetchTarget::Api {
        base_url,
        key: provider.key.clone(),
    })
}

#[cfg(test)]
pub type ProviderAuthState = AuthState;
pub type ProviderAuthStates = BTreeMap<String, AuthState>;

// ---- interactive screen ----

/// A single-line action collecting one input line. (Set-model is a guided
/// wizard, not a line action — see [`ModelWizard`].)
enum Action {
    UnsetModel,
}

enum Mode {
    Browse,
    /// A second confirmation before a staged endpoint or static API-key change
    /// is sent to the connected Server. The summary contains names/change kinds
    /// only and never includes a credential value.
    ConfirmSensitiveSave {
        summary: String,
        after_save: AfterSave,
    },
    /// Confirmation shown before a dirty editor may close. The selection order
    /// is deliberately Save / Discard / Cancel, matching the rendered prompt.
    ConfirmExit(ExitConfirm),
    Input {
        action: Action,
        prompt: &'static str,
        buffer: LineEditor,
    },
    /// Guided, step-by-step add-provider: pick the type from a menu, then fill
    /// each field with its own prompt (replaces the old one comma-separated line).
    AddProvider(AddWizard),
    /// Guided, two-level set-model: pick the role, pick a provider, then pick
    /// one of that provider's fetched models (or type an id when the fetch is
    /// unavailable). Replaces the old comma-separated `role, members, strategy`.
    SetModel(ModelWizard),
    /// Edit an existing **api** provider: change its base_url (prefilled) and api
    /// key (blank keeps the current one). The name and type stay fixed.
    EditProvider(EditWizard),
    /// The OAuth action menu for an existing **oauth:codex** provider: sign in,
    /// sign out, or switch account. The chosen action is carried out after the
    /// editor exits (the browser flow can't run under the full-screen UI).
    OauthActions(OauthMenu),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ExitChoice {
    Save,
    Discard,
    Cancel,
}

const EXIT_CHOICES: &[(&str, ExitChoice)] = &[
    ("Save", ExitChoice::Save),
    ("Discard", ExitChoice::Discard),
    ("Cancel", ExitChoice::Cancel),
];

struct ExitConfirm {
    sel: usize,
}

/// The two editable fields of an api provider, in order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditStep {
    BaseUrl,
    Key,
}

/// Accumulating state for editing an existing api provider. `base_url` starts
/// prefilled; `key` starts empty and, left blank, keeps the current key.
struct EditWizard {
    name: String,
    kind: String,
    /// The provider's current key, preserved when the user leaves the key blank.
    existing_key: Option<String>,
    step: EditStep,
    base_url: String,
    buffer: LineEditor,
}

impl EditWizard {
    fn new(name: String, kind: String, base_url: Option<String>, key: Option<String>) -> Self {
        let mut buffer = LineEditor::default();
        buffer.set_text(base_url.clone().unwrap_or_default());
        Self {
            name,
            kind,
            existing_key: key,
            step: EditStep::BaseUrl,
            base_url: base_url.unwrap_or_default(),
            buffer,
        }
    }

    /// Store the current field and advance. Returns `Some((base_url, key))` when
    /// the edit is complete (the caller upserts it). A blank key keeps the
    /// existing one; a non-blank key replaces it.
    fn advance(&mut self, text: String) -> Option<(Option<String>, Option<String>)> {
        match self.step {
            EditStep::BaseUrl => {
                self.base_url = text.trim().to_string();
                self.step = EditStep::Key;
                self.buffer = LineEditor::default();
                None
            }
            EditStep::Key => {
                let entered = text.trim().to_string();
                let key = if entered.is_empty() {
                    self.existing_key.clone()
                } else {
                    Some(entered)
                };
                let base_url = (!self.base_url.is_empty()).then(|| self.base_url.clone());
                Some((base_url, key))
            }
        }
    }
}

/// The OAuth actions offered for an `oauth:codex` provider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AuthAction {
    Login,
    Logout,
    Switch,
}

const AUTH_ACTIONS: &[(&str, AuthAction)] = &[
    ("Sign in", AuthAction::Login),
    ("Sign out", AuthAction::Logout),
    ("Switch account (sign out, then in)", AuthAction::Switch),
];

/// The OAuth action menu for one `oauth:codex` provider.
struct OauthMenu {
    provider: String,
    sel: usize,
}

/// What the editor asks the caller to do after it exits: run an OAuth action for
/// a provider (the browser flow needs the plain terminal, so it can't run under
/// the full-screen UI).
pub struct AuthRequest {
    pub action: AuthAction,
    pub provider: String,
}

/// One step of the add-provider wizard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum AddStep {
    PickType,
    Name,
    BaseUrl,
    Key,
}

/// The next step after `step` for a provider type, skipping fields the type does
/// not use (oauth types carry no base_url/key). `None` = the wizard is complete.
/// Pure — unit-testable.
fn add_next_step(step: AddStep, requires_base_url: bool, allows_key: bool) -> Option<AddStep> {
    match step {
        AddStep::PickType => Some(AddStep::Name),
        AddStep::Name => {
            if requires_base_url {
                Some(AddStep::BaseUrl)
            } else if allows_key {
                Some(AddStep::Key)
            } else {
                None
            }
        }
        AddStep::BaseUrl => allows_key.then_some(AddStep::Key),
        AddStep::Key => None,
    }
}

/// Accumulating state for the add-provider wizard.
struct AddWizard {
    step: AddStep,
    type_sel: usize,
    kind: String,
    name: String,
    base_url: String,
    key: String,
    buffer: LineEditor,
}

impl AddWizard {
    fn new() -> Self {
        Self {
            step: AddStep::PickType,
            type_sel: 0,
            kind: String::new(),
            name: String::new(),
            base_url: String::new(),
            key: String::new(),
            buffer: LineEditor::default(),
        }
    }

    fn selected_type(&self) -> Option<&'static pc::ProviderType> {
        pc::provider_types().get(self.type_sel)
    }

    /// Store the just-entered field text and advance; returns `true` when the
    /// wizard is complete (the caller then calls `add_provider`).
    fn advance_text(&mut self, text: String) -> bool {
        let text = text.trim().to_string();
        match self.step {
            AddStep::Name => self.name = text,
            AddStep::BaseUrl => self.base_url = text,
            AddStep::Key => self.key = text,
            AddStep::PickType => {}
        }
        let ty = self.selected_type();
        let (rb, ak) = ty
            .map(|t| (t.requires_base_url, t.allows_key))
            .unwrap_or((false, false));
        match add_next_step(self.step, rb, ak) {
            Some(next) => {
                self.step = next;
                self.buffer = LineEditor::default();
                false
            }
            None => true,
        }
    }
}

/// The model roles a guided set-model can target. Kept in sync with the two
/// roles the runtime resolves (`main`, `cheap`); the wizard offers exactly these
/// so the user picks instead of free-typing a role name.
const MODEL_ROLES: &[&str] = &["main", "cheap"];

/// One step of the set-model wizard.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ModelStep {
    PickRole,
    PickProvider,
    /// Transient: the run loop performs the `/models` fetch, then calls
    /// [`ModelWizard::apply_fetch`] to advance to `PickModel`.
    Fetching,
    CatalogFailed,
    PickModel,
}

/// Accumulating state for the two-level set-model wizard. The provider names are
/// captured at construction (non-empty — the caller guards on that), so the
/// role→provider steps are pure navigation; only the provider→model transition
/// touches the network, and that runs in the loop (not in `on_key`), keeping key
/// handling unit-testable.
struct ModelWizard {
    step: ModelStep,
    role_sel: usize,
    role: String,
    prov_names: Vec<String>,
    prov_sel: usize,
    provider: String,
    /// Models fetched from the provider's `/models`. Empty when the fetch was
    /// skipped or failed — then `manual` is set and the user types the id.
    models: Vec<String>,
    model_sel: usize,
    manual: bool,
    /// Search box in list mode; the whole model id in manual mode.
    filter: LineEditor,
    /// One-line hint shown in the title (model count, or why it fell to manual).
    note: String,
    selection: Option<ModelSelection>,
    show_catalog_details: bool,
}

impl ModelWizard {
    fn new(prov_names: Vec<String>) -> Self {
        Self {
            step: ModelStep::PickRole,
            role_sel: 0,
            role: String::new(),
            prov_names,
            prov_sel: 0,
            provider: String::new(),
            models: Vec::new(),
            model_sel: 0,
            manual: false,
            filter: LineEditor::default(),
            note: String::new(),
            selection: None,
            show_catalog_details: false,
        }
    }

    /// The models matching the current filter box (list mode).
    fn filtered(&self) -> Vec<&String> {
        crate::model_picker::filter_models(&self.models, self.filter.text())
    }

    /// Consume the `/models` fetch outcome and advance to `PickModel`: a
    /// non-empty list becomes a searchable pick; an empty list or an error
    /// degrades to manual entry (the wizard never dead-ends on a bad fetch).
    /// Pure — unit-tested.
    fn apply_fetch(&mut self, result: std::result::Result<Vec<String>, ProviderIssue>) {
        let Some(selection) = &mut self.selection else {
            self.note = "catalog result arrived without a selection".to_string();
            self.step = ModelStep::CatalogFailed;
            return;
        };
        selection.finish(result);
        self.apply_fetch_state();
    }

    fn apply_fetch_state(&mut self) {
        let Some(catalog) = self
            .selection
            .as_ref()
            .map(|selection| selection.catalog.clone())
        else {
            return;
        };
        match catalog {
            CatalogState::Available(models) => {
                self.note = format!("{} models · type to filter", models.len());
                self.models = models;
                self.manual = false;
                self.step = ModelStep::PickModel;
            }
            CatalogState::Failed(error) | CatalogState::Unavailable(error) => {
                self.models = Vec::new();
                self.manual = false;
                self.note = error.message;
                self.step = ModelStep::CatalogFailed;
            }
            CatalogState::Idle | CatalogState::Loading { .. } | CatalogState::Manual { .. } => {}
        }
        self.model_sel = 0;
        self.filter = LineEditor::default();
    }
}

struct App {
    ed: ProviderEditor,
    /// Last snapshot confirmed by the Server, used to identify endpoint/key
    /// changes that require an explicit confirmation before the next save.
    persisted_cfg: ProvidersConfig,
    sel: usize,
    mode: Mode,
    status: String,
    auth_states: ProviderAuthStates,
    connection_id: String,
    config_protocol: u32,
    /// True when the in-memory provider/model graph differs from the last
    /// successful save. A failed save never clears this flag.
    dirty: bool,
    /// The previous mutation was confirmed, but its replacement snapshot was
    /// not. No edit or save is safe until the editor is reopened.
    refresh_required: bool,
    save_now: bool,
    after_save: AfterSave,
    /// Set by the set-model wizard when it enters `Fetching`; the run loop does
    /// the blocking `/models` fetch (I/O belongs there, not in `on_key`) and
    /// feeds the result back via [`ModelWizard::apply_fetch`].
    fetch_models_now: bool,
    /// Set when the user picks an OAuth action for a provider: the editor saves
    /// and quits, and the caller runs the action (browser flow) after the
    /// full-screen UI is torn down.
    auth_request: Option<AuthRequest>,
    /// Last concurrent-edit conflict. It remains available to the caller if the
    /// user discards the staged edit and exits, preserving the existing reload
    /// contract without forcing an immediate, lossy exit on conflict.
    conflict: Option<String>,
    quit: bool,
}

/// Describe endpoint/key mutations without exposing either key value. Provider
/// additions/removals are included when they introduce or remove one of these
/// sensitive fields; model-only edits return an empty list.
fn sensitive_provider_changes(
    before: &ProvidersConfig,
    after: &ProvidersConfig,
    clear_keys: &BTreeSet<String>,
) -> Vec<String> {
    let names = before
        .providers
        .keys()
        .chain(after.providers.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut changes = Vec::new();
    for name in names {
        let old = before.providers.get(&name);
        let new = after.providers.get(&name);
        if old.and_then(|provider| provider.base_url.as_ref())
            != new.and_then(|provider| provider.base_url.as_ref())
        {
            changes.push(format!(
                "endpoint for '{}'",
                crate::terminal_safe_text(&name)
            ));
        }
        if old.and_then(|provider| provider.key.as_ref())
            != new.and_then(|provider| provider.key.as_ref())
        {
            changes.push(format!(
                "API key for '{}'",
                crate::terminal_safe_text(&name)
            ));
        }
    }
    for name in clear_keys {
        let description = format!("API key for '{}'", crate::terminal_safe_text(name));
        if !changes.contains(&description) {
            changes.push(description);
        }
    }
    changes
}

enum AfterSave {
    Stay,
    Quit,
    Auth(AuthRequest),
}

impl App {
    #[cfg(test)]
    fn new(cfg: ProvidersConfig) -> Self {
        Self::with_auth_states(
            cfg,
            BTreeSet::new(),
            ProviderAuthStates::new(),
            "test-server".to_string(),
            4,
        )
    }

    fn with_auth_states(
        cfg: ProvidersConfig,
        key_present: BTreeSet<String>,
        auth_states: ProviderAuthStates,
        connection_id: String,
        config_protocol: u32,
    ) -> Self {
        Self {
            ed: ProviderEditor::with_key_presence(cfg.clone(), key_present),
            persisted_cfg: cfg,
            sel: 0,
            mode: Mode::Browse,
            // The key hints live on their own persistent footer line (see
            // `hints_for`); the status line starts empty and shows action results.
            status: "editing Server provider configuration".to_string(),
            auth_states,
            connection_id,
            config_protocol,
            dirty: false,
            refresh_required: false,
            save_now: false,
            after_save: AfterSave::Stay,
            fetch_models_now: false,
            auth_request: None,
            conflict: None,
            quit: false,
        }
    }

    /// Apply the submitted input buffer for the active single-line action.
    fn submit(&mut self, action: &Action, buf: &str) {
        let parts: Vec<String> = buf.split(',').map(|s| s.trim().to_string()).collect();
        let res = match action {
            Action::UnsetModel => match parts.as_slice() {
                [role] if !role.is_empty() => self.ed.unset_model(role),
                _ => Err(CoreError::Message("expected: model role".to_string())),
            },
        };
        self.status = match res {
            Ok(()) => {
                self.dirty = true;
                "model change staged, not saved".to_string()
            }
            Err(e) => format!("error: {e}"),
        };
    }

    fn mark_staged(&mut self, message: impl Into<String>) {
        self.dirty = true;
        self.status = format!("{} — staged, not saved", message.into());
    }

    fn begin_save(&mut self, after_save: AfterSave) {
        self.save_now = true;
        self.after_save = after_save;
    }

    fn request_save(&mut self, after_save: AfterSave) {
        let sensitive =
            sensitive_provider_changes(&self.persisted_cfg, self.ed.config(), self.ed.clear_keys());
        if sensitive.is_empty() {
            self.begin_save(after_save);
        } else {
            self.status = "sensitive Provider changes require confirmation".to_string();
            self.mode = Mode::ConfirmSensitiveSave {
                summary: sensitive.join(", "),
                after_save,
            };
        }
    }

    fn request_auth(&mut self, request: AuthRequest) {
        self.request_save(AfterSave::Auth(request));
    }

    /// Apply one saver result to the editor state. OAuth is deliberately held
    /// in `after_save` and is only released into the public outcome after a
    /// confirmed `Saved` result.
    fn finish_save(&mut self, result: Result<SaveOutcome>) {
        let after_save = std::mem::replace(&mut self.after_save, AfterSave::Stay);
        match result {
            Ok(SaveOutcome::Saved) => {
                self.ed.finish_save();
                self.persisted_cfg = self.ed.config().clone();
                self.dirty = false;
                self.conflict = None;
                self.status = "saved".to_string();
                match after_save {
                    AfterSave::Stay => {}
                    AfterSave::Quit => self.quit = true,
                    AfterSave::Auth(request) => {
                        self.auth_request = Some(request);
                        self.quit = true;
                    }
                }
            }
            Ok(SaveOutcome::SavedRefreshRequired(reason)) => {
                let reason = self.ed.redact_secrets(&reason);
                self.ed.finish_save();
                self.persisted_cfg = self.ed.config().clone();
                self.dirty = false;
                self.refresh_required = true;
                self.mode = Mode::Browse;
                self.auth_request = None;
                self.conflict = None;
                self.status = format!(
                    "changes applied, but refresh failed: {reason} — exit and reopen before editing"
                );
            }
            Ok(SaveOutcome::Conflict(message)) => {
                self.after_save = after_save;
                let message = self.ed.redact_secrets(&message);
                self.dirty = true;
                self.mode = Mode::Browse;
                self.auth_request = None;
                self.conflict = Some(message.clone());
                self.status = format!("save conflict: {message} — staged changes kept, not saved");
            }
            Err(error) => {
                self.after_save = after_save;
                self.dirty = true;
                self.mode = Mode::Browse;
                self.auth_request = None;
                let message = self.ed.redact_secrets(&error.report().message);
                self.status = format!("save failed: {message} — staged changes kept, not saved");
            }
        }
    }
}

/// Drive the guided add-provider wizard. Computes the outcome under a scoped
/// borrow of the wizard, then mutates `app` (so completing can touch `app.ed`).
fn on_key_add_wizard(app: &mut App, code: KeyCode) {
    enum WizAction {
        Stay,
        Cancel,
        Complete,
    }
    let action = {
        let Mode::AddProvider(wiz) = &mut app.mode else {
            return;
        };
        match wiz.step {
            AddStep::PickType => match code {
                KeyCode::Left | KeyCode::Up => {
                    wiz.type_sel = wiz.type_sel.saturating_sub(1);
                    WizAction::Stay
                }
                KeyCode::Right | KeyCode::Down => {
                    let max = pc::provider_types().len().saturating_sub(1);
                    wiz.type_sel = (wiz.type_sel + 1).min(max);
                    WizAction::Stay
                }
                KeyCode::Enter => {
                    wiz.kind = wiz
                        .selected_type()
                        .map(|t| t.name.to_string())
                        .unwrap_or_default();
                    wiz.step = AddStep::Name;
                    wiz.buffer = LineEditor::default();
                    WizAction::Stay
                }
                KeyCode::Esc => WizAction::Cancel,
                _ => WizAction::Stay,
            },
            AddStep::Name | AddStep::BaseUrl | AddStep::Key => match code {
                KeyCode::Char(c) => {
                    wiz.buffer.insert(c);
                    WizAction::Stay
                }
                KeyCode::Backspace => {
                    wiz.buffer.backspace();
                    WizAction::Stay
                }
                KeyCode::Left => {
                    wiz.buffer.left();
                    WizAction::Stay
                }
                KeyCode::Right => {
                    wiz.buffer.right();
                    WizAction::Stay
                }
                KeyCode::Esc => WizAction::Cancel,
                KeyCode::Enter => {
                    let text = wiz.buffer.take();
                    if wiz.advance_text(text) {
                        WizAction::Complete
                    } else {
                        WizAction::Stay
                    }
                }
                _ => WizAction::Stay,
            },
        }
    };
    match action {
        WizAction::Stay => {}
        WizAction::Cancel => {
            app.mode = Mode::Browse;
            app.status = "cancelled".to_string();
        }
        WizAction::Complete => {
            if let Mode::AddProvider(w) = std::mem::replace(&mut app.mode, Mode::Browse) {
                let base_url = (!w.base_url.is_empty()).then_some(w.base_url);
                let key = (!w.key.is_empty()).then_some(w.key);
                let is_oauth = w.kind.eq_ignore_ascii_case("oauth:codex");
                match app
                    .ed
                    .add_provider(w.name.clone(), w.kind.clone(), base_url, key)
                {
                    Ok(()) => {
                        app.mark_staged(format!("added provider '{}'", w.name));
                        // An oauth:codex provider is useless until it is signed in,
                        // so go straight into login: save the new provider (to the
                        // server, via the caller) then run the sign-in flow —
                        // exactly what the OAuth-action menu does.
                        if is_oauth {
                            app.request_auth(AuthRequest {
                                action: AuthAction::Login,
                                provider: w.name.clone(),
                            });
                            app.status = format!(
                                "added provider '{}' — staged, saving before sign-in…",
                                w.name
                            );
                        }
                    }
                    Err(e) => app.status = format!("error: {e}"),
                }
            }
        }
    }
}

/// Drive the guided two-level set-model wizard. Like the add wizard, the
/// outcome is computed under a scoped borrow, then applied to `app` (Complete
/// touches `app.ed`; Fetch flags the run loop). Pure w.r.t. the network — the
/// `/models` fetch happens in the run loop, not here.
fn on_key_model_wizard(app: &mut App, code: KeyCode) {
    enum MdAction {
        Stay,
        Cancel,
        Fetch,
        Login(String),
        Complete {
            role: String,
            provider: String,
            model: String,
        },
    }
    let connection_id = app.connection_id.clone();
    let action = {
        let Mode::SetModel(w) = &mut app.mode else {
            return;
        };
        match w.step {
            ModelStep::PickRole => match code {
                KeyCode::Left | KeyCode::Up => {
                    w.role_sel = w.role_sel.saturating_sub(1);
                    MdAction::Stay
                }
                KeyCode::Right | KeyCode::Down => {
                    w.role_sel = (w.role_sel + 1).min(MODEL_ROLES.len() - 1);
                    MdAction::Stay
                }
                KeyCode::Enter => {
                    w.role = MODEL_ROLES[w.role_sel].to_string();
                    w.step = ModelStep::PickProvider;
                    w.prov_sel = 0;
                    MdAction::Stay
                }
                KeyCode::Esc => MdAction::Cancel,
                _ => MdAction::Stay,
            },
            ModelStep::PickProvider => match code {
                KeyCode::Up => {
                    w.prov_sel = w.prov_sel.saturating_sub(1);
                    MdAction::Stay
                }
                KeyCode::Down => {
                    w.prov_sel = (w.prov_sel + 1).min(w.prov_names.len().saturating_sub(1));
                    MdAction::Stay
                }
                KeyCode::Enter => {
                    w.provider = w.prov_names.get(w.prov_sel).cloned().unwrap_or_default();
                    w.selection = Some(ModelSelection::loading(
                        connection_id,
                        w.provider.clone(),
                        w.role.clone(),
                    ));
                    w.step = ModelStep::Fetching;
                    MdAction::Fetch
                }
                KeyCode::Esc => {
                    w.step = ModelStep::PickRole;
                    MdAction::Stay
                }
                _ => MdAction::Stay,
            },
            // The run loop owns this step; only allow bailing out.
            ModelStep::Fetching => match code {
                KeyCode::Esc => MdAction::Cancel,
                _ => MdAction::Stay,
            },
            ModelStep::CatalogFailed => match code {
                KeyCode::Char('r') => {
                    if w.selection
                        .as_mut()
                        .and_then(ModelSelection::retry)
                        .is_some()
                    {
                        w.step = ModelStep::Fetching;
                        MdAction::Fetch
                    } else {
                        MdAction::Stay
                    }
                }
                KeyCode::Char('m') => {
                    if let Some(selection) = &mut w.selection {
                        selection.enter_manual();
                    }
                    w.manual = true;
                    w.filter = LineEditor::default();
                    w.step = ModelStep::PickModel;
                    MdAction::Stay
                }
                KeyCode::Char('l') => MdAction::Login(w.provider.clone()),
                KeyCode::Char('d') => {
                    w.show_catalog_details = !w.show_catalog_details;
                    MdAction::Stay
                }
                KeyCode::Esc => {
                    w.step = ModelStep::PickProvider;
                    MdAction::Stay
                }
                _ => MdAction::Stay,
            },
            ModelStep::PickModel => {
                // Manual entry: the filter box IS the model id.
                if w.manual {
                    match code {
                        KeyCode::Char(c) => {
                            w.filter.insert(c);
                            MdAction::Stay
                        }
                        KeyCode::Backspace => {
                            w.filter.backspace();
                            MdAction::Stay
                        }
                        KeyCode::Left => {
                            w.filter.left();
                            MdAction::Stay
                        }
                        KeyCode::Right => {
                            w.filter.right();
                            MdAction::Stay
                        }
                        KeyCode::Esc => {
                            w.step = ModelStep::PickProvider;
                            MdAction::Stay
                        }
                        KeyCode::Enter => {
                            let model = w.filter.text().trim().to_string();
                            if model.is_empty() {
                                MdAction::Stay
                            } else {
                                MdAction::Complete {
                                    role: w.role.clone(),
                                    provider: w.provider.clone(),
                                    model,
                                }
                            }
                        }
                        _ => MdAction::Stay,
                    }
                } else {
                    // Searchable list: type to filter, ↑↓ to move the highlight.
                    match code {
                        KeyCode::Up => {
                            w.model_sel = w.model_sel.saturating_sub(1);
                            MdAction::Stay
                        }
                        KeyCode::Down => {
                            let max = w.filtered().len().saturating_sub(1);
                            w.model_sel = (w.model_sel + 1).min(max);
                            MdAction::Stay
                        }
                        KeyCode::Char(c) => {
                            w.filter.insert(c);
                            w.model_sel = 0;
                            MdAction::Stay
                        }
                        KeyCode::Backspace => {
                            w.filter.backspace();
                            w.model_sel = 0;
                            MdAction::Stay
                        }
                        KeyCode::Esc => {
                            w.step = ModelStep::PickProvider;
                            MdAction::Stay
                        }
                        KeyCode::Enter => {
                            // Highlighted match wins; otherwise a non-empty
                            // filter is taken as a typed-in id (manual override).
                            let picked = w.filtered().get(w.model_sel).map(|m| (*m).clone());
                            let model =
                                picked.unwrap_or_else(|| w.filter.text().trim().to_string());
                            if model.is_empty() {
                                MdAction::Stay
                            } else {
                                MdAction::Complete {
                                    role: w.role.clone(),
                                    provider: w.provider.clone(),
                                    model,
                                }
                            }
                        }
                        _ => MdAction::Stay,
                    }
                }
            }
        }
    };
    match action {
        MdAction::Stay => {}
        MdAction::Cancel => {
            app.mode = Mode::Browse;
            app.status = "cancelled".to_string();
        }
        MdAction::Fetch => {
            let gate = match &app.mode {
                Mode::SetModel(w) => app
                    .ed
                    .cfg
                    .provider(&w.provider)
                    .map(|provider| {
                        let auth = app.auth_states.get(&w.provider).cloned().unwrap_or(
                            AuthState::Unavailable(ProviderIssue::new(
                                "unknown",
                                "Provider authentication state is unavailable",
                                Some("Refresh Provider status"),
                            )),
                        );
                        catalog_gate(&provider.kind, &auth, app.config_protocol)
                    })
                    .unwrap_or_else(|| {
                        CatalogState::Unavailable(ProviderIssue::new(
                            "not_found",
                            format!("No Provider named '{}'", w.provider),
                            Some("Select another Provider"),
                        ))
                    }),
                _ => CatalogState::Unavailable(ProviderIssue::new(
                    "state",
                    "Model selection is no longer active",
                    Some("Start model selection again"),
                )),
            };
            if matches!(gate, CatalogState::Idle) {
                app.fetch_models_now = true;
            } else if let Mode::SetModel(w) = &mut app.mode {
                if let Some(selection) = &mut w.selection {
                    selection.catalog = gate;
                }
                w.apply_fetch_state();
            }
        }
        MdAction::Login(provider) => app.request_auth(AuthRequest {
            action: AuthAction::Login,
            provider,
        }),
        MdAction::Complete {
            role,
            provider,
            model,
        } => {
            let result = app.ed.set_model(
                role.clone(),
                vec![Member {
                    provider,
                    model: model.clone(),
                    stream: false,
                    modalities: None,
                    effort: None,
                }],
                Strategy::Single,
            );
            app.mode = Mode::Browse;
            match result {
                Ok(()) => app.mark_staged(format!("set {role} = {model} (single)")),
                Err(error) => app.status = format!("error: {error}"),
            }
        }
    }
}

/// Drive the edit-existing-api-provider wizard (base_url then api key). Completing
/// upserts the provider via [`ProviderEditor::set_provider`].
fn on_key_edit_wizard(app: &mut App, code: KeyCode) {
    enum EdAction {
        Stay,
        Cancel,
        Complete {
            name: String,
            kind: String,
            base_url: Option<String>,
            key: Option<String>,
        },
    }
    let action = {
        let Mode::EditProvider(w) = &mut app.mode else {
            return;
        };
        match code {
            KeyCode::Char(c) => {
                w.buffer.insert(c);
                EdAction::Stay
            }
            KeyCode::Backspace => {
                w.buffer.backspace();
                EdAction::Stay
            }
            KeyCode::Left => {
                w.buffer.left();
                EdAction::Stay
            }
            KeyCode::Right => {
                w.buffer.right();
                EdAction::Stay
            }
            KeyCode::Esc => EdAction::Cancel,
            KeyCode::Enter => {
                let text = w.buffer.take();
                match w.advance(text) {
                    Some((base_url, key)) => EdAction::Complete {
                        name: w.name.clone(),
                        kind: w.kind.clone(),
                        base_url,
                        key,
                    },
                    None => EdAction::Stay,
                }
            }
            _ => EdAction::Stay,
        }
    };
    match action {
        EdAction::Stay => {}
        EdAction::Cancel => {
            app.mode = Mode::Browse;
            app.status = "cancelled".to_string();
        }
        EdAction::Complete {
            name,
            kind,
            base_url,
            key,
        } => match app.ed.set_provider(name.clone(), kind, base_url, key) {
            Ok(()) => {
                app.mode = Mode::Browse;
                app.mark_staged(format!("edited provider '{name}'"));
            }
            Err(error) => {
                app.mode = Mode::Browse;
                app.status = format!("error: {error}");
            }
        },
    }
}

/// Drive the OAuth action menu for an `oauth:codex` provider. Enter records the
/// chosen action and requests a save. The action is released to the caller only
/// after that save succeeds, once the full-screen UI is torn down.
fn on_key_oauth_menu(app: &mut App, code: KeyCode) {
    let Mode::OauthActions(m) = &mut app.mode else {
        return;
    };
    match code {
        KeyCode::Left | KeyCode::Up => m.sel = m.sel.saturating_sub(1),
        KeyCode::Right | KeyCode::Down => m.sel = (m.sel + 1).min(AUTH_ACTIONS.len() - 1),
        KeyCode::Esc => {
            app.mode = Mode::Browse;
            app.status = "cancelled".to_string();
        }
        KeyCode::Enter => {
            let action = AUTH_ACTIONS[m.sel].1;
            let provider = m.provider.clone();
            // Save first (so the provider entry exists on the server the login
            // pushes to), then leave the editor to run the action.
            app.request_auth(AuthRequest { action, provider });
        }
        _ => {}
    }
}

fn on_key_exit_confirm(app: &mut App, code: KeyCode) {
    let Mode::ConfirmExit(confirm) = &mut app.mode else {
        return;
    };
    let choice = match code {
        KeyCode::Left | KeyCode::Up => {
            confirm.sel = confirm.sel.saturating_sub(1);
            return;
        }
        KeyCode::Right | KeyCode::Down => {
            confirm.sel = (confirm.sel + 1).min(EXIT_CHOICES.len() - 1);
            return;
        }
        KeyCode::Char('s') => Some(ExitChoice::Save),
        KeyCode::Char('d') => Some(ExitChoice::Discard),
        KeyCode::Char('c') | KeyCode::Esc => Some(ExitChoice::Cancel),
        KeyCode::Enter => Some(EXIT_CHOICES[confirm.sel].1),
        _ => None,
    };
    match choice {
        Some(ExitChoice::Save) => {
            app.mode = Mode::Browse;
            app.request_save(AfterSave::Quit);
            app.status = "saving staged changes…".to_string();
        }
        Some(ExitChoice::Discard) => {
            app.quit = true;
            app.auth_request = None;
        }
        Some(ExitChoice::Cancel) => {
            app.mode = Mode::Browse;
            app.status = "exit cancelled — staged changes not saved".to_string();
        }
        None => {}
    }
}

fn on_key(app: &mut App, code: KeyCode) {
    if app.refresh_required {
        if matches!(code, KeyCode::Char('q') | KeyCode::Esc) {
            app.quit = true;
        }
        return;
    }
    if matches!(app.mode, Mode::ConfirmSensitiveSave { .. }) {
        match code {
            KeyCode::Enter => {
                if let Mode::ConfirmSensitiveSave { after_save, .. } =
                    std::mem::replace(&mut app.mode, Mode::Browse)
                {
                    app.status = "saving confirmed sensitive Provider changes…".to_string();
                    app.begin_save(after_save);
                }
            }
            KeyCode::Esc => {
                app.mode = Mode::Browse;
                app.status =
                    "sensitive Provider save cancelled — changes remain staged".to_string();
            }
            _ => {}
        }
        return;
    }
    if matches!(app.mode, Mode::ConfirmExit(_)) {
        on_key_exit_confirm(app, code);
        return;
    }
    if matches!(app.mode, Mode::AddProvider(_)) {
        on_key_add_wizard(app, code);
        return;
    }
    if matches!(app.mode, Mode::SetModel(_)) {
        on_key_model_wizard(app, code);
        return;
    }
    if matches!(app.mode, Mode::EditProvider(_)) {
        on_key_edit_wizard(app, code);
        return;
    }
    if matches!(app.mode, Mode::OauthActions(_)) {
        on_key_oauth_menu(app, code);
        return;
    }
    match &mut app.mode {
        Mode::AddProvider(_)
        | Mode::SetModel(_)
        | Mode::EditProvider(_)
        | Mode::OauthActions(_)
        | Mode::ConfirmExit(_)
        | Mode::ConfirmSensitiveSave { .. } => {} // handled above
        Mode::Input { buffer, .. } => match code {
            KeyCode::Char(c) => buffer.insert(c),
            KeyCode::Backspace => buffer.backspace(),
            KeyCode::Delete => buffer.delete(),
            KeyCode::Left => buffer.left(),
            KeyCode::Right => buffer.right(),
            KeyCode::Home => buffer.home(),
            KeyCode::End => buffer.end(),
            KeyCode::Esc => {
                app.mode = Mode::Browse;
                app.status = "cancelled".to_string();
            }
            KeyCode::Enter => {
                if let Mode::Input {
                    action, mut buffer, ..
                } = std::mem::replace(&mut app.mode, Mode::Browse)
                {
                    app.submit(&action, &buffer.take());
                }
            }
            _ => {}
        },
        Mode::Browse => match code {
            KeyCode::Char('q') => {
                if app.dirty {
                    app.mode = Mode::ConfirmExit(ExitConfirm { sel: 0 });
                } else {
                    app.quit = true;
                }
            }
            KeyCode::Char('s') => {
                if app.dirty {
                    app.request_save(AfterSave::Stay);
                    app.status = "saving staged changes…".to_string();
                } else {
                    app.status = "no staged changes".to_string();
                }
            }
            KeyCode::Up => app.sel = app.sel.saturating_sub(1),
            KeyCode::Down => {
                let max = app.ed.provider_names().len().saturating_sub(1);
                app.sel = (app.sel + 1).min(max);
            }
            KeyCode::Char('a') => {
                app.mode = Mode::AddProvider(AddWizard::new());
            }
            KeyCode::Char('d') => match app.ed.provider_names().get(app.sel).cloned() {
                Some(name) => match app.ed.remove_provider(&name) {
                    Ok(()) => app.mark_staged(format!("removed '{name}'")),
                    Err(e) => app.status = format!("error: {e}"),
                },
                None => app.status = "nothing selected".to_string(),
            },
            KeyCode::Char('k') => match app.ed.provider_names().get(app.sel).cloned() {
                Some(name) => match app.ed.clear_provider_key(&name) {
                    Ok(()) => app.mark_staged(format!("cleared API key for '{name}'")),
                    Err(error) => app.status = format!("error: {error}"),
                },
                None => app.status = "nothing selected".to_string(),
            },
            KeyCode::Char('e') => match app.ed.provider_names().get(app.sel).cloned() {
                Some(name) => match app.ed.cfg.provider(&name) {
                    // oauth:codex → the sign-in/out/switch menu; api → field edit.
                    Some(p) if p.kind.eq_ignore_ascii_case("oauth:codex") => {
                        app.mode = Mode::OauthActions(OauthMenu {
                            provider: name,
                            sel: 0,
                        });
                    }
                    Some(p) => {
                        app.mode = Mode::EditProvider(EditWizard::new(
                            name,
                            p.kind.clone(),
                            p.base_url.clone(),
                            p.key.clone(),
                        ));
                    }
                    None => app.status = "nothing selected".to_string(),
                },
                None => app.status = "nothing selected".to_string(),
            },
            KeyCode::Char('m') => {
                let names = app.ed.provider_names();
                if names.is_empty() {
                    app.status = "add a provider first".to_string();
                } else {
                    app.mode = Mode::SetModel(ModelWizard::new(names));
                }
            }
            KeyCode::Char('u') => {
                app.mode = Mode::Input {
                    action: Action::UnsetModel,
                    prompt: "unset model — role",
                    buffer: LineEditor::default(),
                };
            }
            _ => {}
        },
    }
}

fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(4)])
        .split(f.area());

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from("Providers:"));
    let provider_views = provider_views(
        &app.ed.cfg,
        app.ed.key_present(),
        &app.auth_states,
        app.config_protocol,
    );
    for (i, provider) in provider_views.iter().enumerate() {
        let marker = if i == app.sel { "▶" } else { " " };
        let roles = provider.roles.join(",");
        lines.push(Line::from(crate::terminal_safe_text(&format!(
            "{marker} {:<14} [{}] endpoint={}{} auth={} catalog={} roles={}",
            provider.name,
            provider.kind,
            provider.endpoint.label(),
            provider
                .key
                .map(|state| format!(" key={}", state.label()))
                .unwrap_or_default(),
            provider.auth.label(),
            catalog_label(&provider.catalog),
            if roles.is_empty() { "(none)" } else { &roles },
        ))));
    }
    lines.push(Line::from(""));
    lines.push(Line::from("Model roles:"));
    for (role, pool) in &app.ed.cfg.models {
        let members = pool
            .members
            .iter()
            .map(|m| format!("{}/{}", m.provider, m.model))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(Line::from(crate::terminal_safe_text(&format!(
            "  {:<8} [{:?}] {}",
            role, pool.strategy, members
        ))));
    }
    let inner_h = chunks[0].height.saturating_sub(2);
    let sel_line = 1 + app.sel as u16;
    let offset = (sel_line + 1).saturating_sub(inner_h);
    let config_title = if app.refresh_required {
        "Provider configuration — reload required"
    } else if app.dirty {
        "Provider configuration — pending Server save"
    } else {
        "Provider configuration — Server snapshot"
    };
    f.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(config_title))
            .scroll((offset, 0)),
        chunks[0],
    );

    // The footer is two lines inside one box: line 1 is the mode's content
    // (status / input / picker), line 2 is the persistent key hints. Because the
    // hints live on their own line, an action's status output (e.g. "added
    // provider 'X'") can never overwrite them.
    let max_w = chunks[1].width.saturating_sub(2) as usize;
    let cursor_col = |x: u16| -> u16 { (2 + x as usize).min(max_w) as u16 };
    let (title, body, cursor_x): (String, String, Option<u16>) = match &app.mode {
        Mode::Browse => (
            "Server provider configuration".to_string(),
            app.status.clone(),
            None,
        ),
        Mode::ConfirmExit(confirm) => {
            let choices = EXIT_CHOICES
                .iter()
                .enumerate()
                .map(|(index, (label, _))| {
                    if index == confirm.sel {
                        format!("▶{label}")
                    } else {
                        format!(" {label}")
                    }
                })
                .collect::<Vec<_>>()
                .join("   ");
            (
                "unsaved changes".to_string(),
                format!("Save / Discard / Cancel: {choices}"),
                None,
            )
        }
        Mode::ConfirmSensitiveSave { summary, .. } => (
            "confirm sensitive Server changes".to_string(),
            format!("{summary} — Enter: apply to Server · Esc: cancel"),
            None,
        ),
        Mode::AddProvider(wiz) => match wiz.step {
            AddStep::PickType => {
                let sel = pc::provider_types()
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        if i == wiz.type_sel {
                            format!("▶{}", t.name)
                        } else {
                            format!(" {}", t.name)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("   ");
                ("add provider".to_string(), format!("type: {sel}"), None)
            }
            AddStep::Name | AddStep::BaseUrl | AddStep::Key => {
                let (label, masked) = match wiz.step {
                    AddStep::BaseUrl => ("base_url", false),
                    AddStep::Key => ("api key", true),
                    _ => ("name", false),
                };
                let (view, x) = wiz.buffer.display_window(max_w.saturating_sub(2));
                let shown = if masked {
                    "*".repeat(view.chars().count())
                } else {
                    view.to_string()
                };
                (
                    format!("add {} provider — {label}", wiz.kind),
                    format!("> {shown}"),
                    (!masked).then(|| cursor_col(x)),
                )
            }
        },
        Mode::SetModel(w) => match w.step {
            ModelStep::PickRole => {
                let sel = MODEL_ROLES
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        if i == w.role_sel {
                            format!("▶{r}")
                        } else {
                            format!(" {r}")
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("   ");
                ("set model".to_string(), format!("role: {sel}"), None)
            }
            ModelStep::PickProvider => {
                let n = w.prov_names.len();
                let name = w
                    .prov_names
                    .get(w.prov_sel)
                    .map(String::as_str)
                    .unwrap_or("");
                (
                    format!("set model [{}]", w.role),
                    format!("provider [{}/{n}]: {name}", w.prov_sel + 1),
                    None,
                )
            }
            ModelStep::Fetching => {
                let previous = w
                    .selection
                    .as_ref()
                    .and_then(|selection| match &selection.catalog {
                        CatalogState::Loading { previous_error } => previous_error.as_ref(),
                        _ => None,
                    })
                    .map(|error| format!(" · previous: {}", error.message))
                    .unwrap_or_default();
                (
                    format!("set model [{}]", w.role),
                    format!("fetching {}/models …{previous}", w.provider),
                    None,
                )
            }
            ModelStep::CatalogFailed => {
                let issue = w
                    .selection
                    .as_ref()
                    .and_then(|selection| match &selection.catalog {
                        CatalogState::Failed(issue) | CatalogState::Unavailable(issue) => {
                            Some(issue)
                        }
                        _ => None,
                    });
                let summary = issue
                    .map(|issue| issue.message.as_str())
                    .unwrap_or("Catalog is unavailable");
                let line = if w.show_catalog_details {
                    issue
                        .map(|issue| {
                            format!(
                                "{} · kind={} · fix={}",
                                issue.message,
                                issue.kind,
                                issue.remediation.as_deref().unwrap_or("none")
                            )
                        })
                        .unwrap_or_else(|| "No additional details".to_string())
                } else {
                    summary.to_string()
                };
                (
                    format!("catalog failed [{}] {}", w.role, w.provider),
                    line,
                    None,
                )
            }
            ModelStep::PickModel => {
                if w.manual {
                    let (view, x) = w.filter.display_window(max_w.saturating_sub(2));
                    (
                        format!("set model [{}] {} — {}", w.role, w.provider, w.note),
                        format!("> {view}"),
                        Some(cursor_col(x)),
                    )
                } else {
                    let filt = w.filtered();
                    let n = filt.len();
                    let highlighted = filt
                        .get(w.model_sel)
                        .map(|m| m.as_str())
                        .unwrap_or("(no match — Enter uses the filter as id)");
                    let idx = if n == 0 { 0 } else { w.model_sel + 1 };
                    let ftext = w.filter.text();
                    let filt_hint = if ftext.is_empty() {
                        String::new()
                    } else {
                        format!("   filter: {ftext}")
                    };
                    (
                        format!("set model [{}] {}", w.role, w.provider),
                        format!("[{idx}/{n}] {highlighted}{filt_hint}"),
                        None,
                    )
                }
            }
        },
        Mode::EditProvider(w) => {
            let (label, masked) = match w.step {
                EditStep::BaseUrl => ("base_url", false),
                EditStep::Key => ("api key (blank keeps current)", true),
            };
            let (view, x) = w.buffer.display_window(max_w.saturating_sub(2));
            let shown = if masked {
                "*".repeat(view.chars().count())
            } else {
                view.to_string()
            };
            (
                format!("edit {} provider '{}' — {label}", w.kind, w.name),
                format!("> {shown}"),
                (!masked).then(|| cursor_col(x)),
            )
        }
        Mode::OauthActions(m) => {
            let sel = AUTH_ACTIONS
                .iter()
                .enumerate()
                .map(|(i, (label, _))| {
                    if i == m.sel {
                        format!("▶{label}")
                    } else {
                        format!(" {label}")
                    }
                })
                .collect::<Vec<_>>()
                .join("   ");
            (format!("Codex sign-in — '{}'", m.provider), sel, None)
        }
        Mode::Input { prompt, buffer, .. } => {
            let (view, x) = buffer.display_window(max_w.saturating_sub(2));
            (prompt.to_string(), format!("> {view}"), Some(cursor_col(x)))
        }
    };
    let title = crate::terminal_safe_text(&title);
    let body = crate::terminal_safe_text(&body);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(body),
            Line::from(if app.refresh_required {
                "q/Esc: exit and reopen to reload the Server snapshot"
            } else {
                hints_for(&app.mode)
            }),
        ])
        .block(Block::bordered().title(title)),
        chunks[1],
    );
    if let Some(cx) = cursor_x {
        f.set_cursor_position((chunks[1].x + 1 + cx, chunks[1].y + 1));
    }
}

/// The persistent key-hint line for the current screen. Rendered on its own
/// footer line so an action's status output never hides it (spec: the key hints
/// stay visible). Pure — a non-empty hint exists for every screen.
fn hints_for(mode: &Mode) -> &'static str {
    match mode {
        Mode::Browse => {
            "a: add · e: edit · k: clear-key · d: del · m: set-model · u: unset · s: save · q: quit"
        }
        Mode::ConfirmExit(_) => {
            "←→: choose · Enter: confirm · s: save · d: discard · c/Esc: cancel"
        }
        Mode::ConfirmSensitiveSave { .. } => {
            "Enter: confirm endpoint/key changes · Esc: keep staged without saving"
        }
        Mode::AddProvider(w) => match w.step {
            AddStep::PickType => "←→: pick type · Enter: next · Esc: cancel",
            _ => "type the value · Enter: next · Esc: cancel",
        },
        Mode::SetModel(w) => match w.step {
            ModelStep::PickRole => "←→: pick role · Enter: next · Esc: cancel",
            ModelStep::PickProvider => "↑↓: provider · Enter: fetch models · Esc: back",
            ModelStep::Fetching => "fetching the provider's models… · Esc: cancel",
            ModelStep::CatalogFailed => {
                "r: retry · m: enter model ID · l: login · d: details · Esc: back"
            }
            ModelStep::PickModel => "↑↓: pick · type: filter · Enter: set · Esc: back",
        },
        Mode::EditProvider(_) => "type the value · Enter: next · Esc: cancel",
        Mode::OauthActions(_) => "←→: action · Enter: run · Esc: back",
        Mode::Input { .. } => "Enter: save · Esc: cancel",
    }
}

/// Why the editor exited (beyond a normal quit): the last concurrent-edit
/// conflict, retained until the user decides to leave, and/or an OAuth action
/// released only by a successful save.
pub struct EditorOutcome {
    pub conflict: Option<String>,
    pub auth_request: Option<AuthRequest>,
}

pub struct ProviderEditorInput {
    pub config: ProvidersConfig,
    pub key_present: BTreeSet<String>,
}

type CatalogFetchResult = std::result::Result<Vec<String>, ProviderIssue>;
type ActiveCatalogFetch = (
    CatalogRequest,
    std::sync::mpsc::Receiver<CatalogFetchResult>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
);

/// Server-bound state shared by one Provider editor session.
pub struct ProviderEditorContext {
    pub auth_states: ProviderAuthStates,
    pub connection_id: String,
    pub config_protocol: u32,
}

fn cancel_hidden_catalog_fetch(app: &App, active_fetch: &mut Option<ActiveCatalogFetch>) {
    use std::sync::atomic::Ordering;

    let active_request_still_visible = match active_fetch.as_ref() {
        None => true,
        Some((request, _, _)) => {
            matches!(
                &app.mode,
                Mode::SetModel(w)
                    if w.step == ModelStep::Fetching
                        && w.selection.as_ref().map(ModelSelection::request)
                            == Some(request.clone())
            )
        }
    };
    if !active_request_still_visible {
        if let Some((_, _, cancellation)) = active_fetch.take() {
            cancellation.store(true, Ordering::Release);
        }
    }
}

/// Open the editor with caller-owned model discovery and credential-status
/// providers. The remote config editor uses this to keep every operation bound
/// to its existing authenticated Server target.
pub fn run_with_saver_and_fetcher(
    initial: ProviderEditorInput,
    save: impl FnMut(&ProvidersConfig, &BTreeSet<String>) -> Result<SaveOutcome>,
    fetch_models: impl Fn(&CatalogRequest, std::sync::Arc<std::sync::atomic::AtomicBool>) -> CatalogFetchResult
        + Send
        + Sync
        + 'static,
    auth_states: ProviderAuthStates,
    connection_id: String,
    config_protocol: u32,
    terminal_input: &mut crate::workspace::WorkspaceInput,
) -> Result<EditorOutcome> {
    let mut terminal = ratatui::init();
    let result = run_with_terminal(
        &mut terminal,
        initial,
        save,
        fetch_models,
        ProviderEditorContext {
            auth_states,
            connection_id,
            config_protocol,
        },
        terminal_input,
    );
    ratatui::restore();
    result
}

/// Run the Provider editor on a terminal owned by the caller. This core entry
/// point never initializes or restores the terminal; embedded Settings flows
/// therefore keep one continuous alternate-screen session.
pub fn run_with_terminal<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
    initial: ProviderEditorInput,
    mut save: impl FnMut(&ProvidersConfig, &BTreeSet<String>) -> Result<SaveOutcome>,
    fetch_models: impl Fn(&CatalogRequest, std::sync::Arc<std::sync::atomic::AtomicBool>) -> CatalogFetchResult
        + Send
        + Sync
        + 'static,
    context: ProviderEditorContext,
    terminal_input: &mut crate::workspace::WorkspaceInput,
) -> Result<EditorOutcome> {
    use std::sync::atomic::AtomicBool;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    let mut app = App::with_auth_states(
        initial.config,
        initial.key_present,
        context.auth_states,
        context.connection_id,
        context.config_protocol,
    );
    let fetch_models = Arc::new(fetch_models);
    let mut active_fetch: Option<ActiveCatalogFetch> = None;
    let result = (|| -> Result<()> {
        loop {
            cancel_hidden_catalog_fetch(&app, &mut active_fetch);
            if let Some((request, receiver, _)) = &active_fetch {
                match receiver.try_recv() {
                    Ok(fetch_result) => {
                        let matches_active_request = matches!(
                            &app.mode,
                            Mode::SetModel(w)
                                if w.step == ModelStep::Fetching
                                    && w.selection.as_ref().map(ModelSelection::request)
                                        == Some(request.clone())
                        );
                        if matches_active_request {
                            if let Mode::SetModel(w) = &mut app.mode {
                                w.apply_fetch(fetch_result);
                            }
                        }
                        active_fetch = None;
                    }
                    Err(mpsc::TryRecvError::Empty) => {}
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if let Mode::SetModel(w) = &mut app.mode {
                            w.apply_fetch(Err(ProviderIssue::new(
                                "catalog_worker",
                                "Provider catalog worker stopped unexpectedly",
                                Some("Retry the catalog request"),
                            )));
                        }
                        active_fetch = None;
                    }
                }
            }
            terminal
                .draw(|f| render(f, &app))
                .map_err(|e| CoreError::Message(format!("draw failed: {e}")))?;
            if app.quit {
                break;
            }
            match terminal_input.try_recv() {
                Ok(key) => {
                    on_key(&mut app, key.code);
                    if app.save_now {
                        app.save_now = false;
                        let save_result = save(app.ed.config(), app.ed.clear_keys());
                        app.finish_save(save_result);
                    }
                    if app.fetch_models_now {
                        app.fetch_models_now = false;
                        let request = match &app.mode {
                            Mode::SetModel(w) => w.selection.as_ref().map(ModelSelection::request),
                            _ => None,
                        };
                        if let Some(request) = request {
                            let (result_tx, result_rx) = mpsc::channel();
                            let worker = Arc::clone(&fetch_models);
                            let worker_request = request.clone();
                            let cancellation = Arc::new(AtomicBool::new(false));
                            let worker_cancellation = Arc::clone(&cancellation);
                            std::thread::spawn(move || {
                                let result = worker(&worker_request, worker_cancellation);
                                let _ = result_tx.send(result);
                            });
                            active_fetch = Some((request, result_rx, cancellation));
                        } else if let Mode::SetModel(w) = &mut app.mode {
                            w.apply_fetch(Err(ProviderIssue::new(
                                "invalid_state",
                                "No Provider catalog request is active",
                                Some("Go back and select the Provider again"),
                            )));
                        }
                    }
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        Ok(())
    })();
    let conflict = app.conflict.take();
    let auth_request = app.auth_request.take();
    result.map(|()| EditorOutcome {
        conflict,
        auth_request,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_model_target_uses_remote_discovery_without_base_url() {
        let provider = pc::Provider {
            kind: "oauth:codex".into(),
            base_url: None,
            key: None,
        };
        assert_eq!(
            model_fetch_target("tingzhen-codex", &provider, true),
            Ok(ModelFetchTarget::Server {
                provider: "tingzhen-codex".into()
            })
        );
    }

    #[test]
    fn oauth_provider_rows_render_signed_in_signed_out_and_unavailable_states() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut cfg = ProvidersConfig::default();
        cfg.providers.insert(
            "signed".into(),
            pc::Provider {
                kind: "oauth:codex".into(),
                base_url: None,
                key: None,
            },
        );
        cfg.providers.insert(
            "signed-out".into(),
            pc::Provider {
                kind: "oauth:codex".into(),
                base_url: None,
                key: None,
            },
        );
        cfg.providers.insert(
            "unknown".into(),
            pc::Provider {
                kind: "oauth:codex".into(),
                base_url: None,
                key: None,
            },
        );
        let mut states = ProviderAuthStates::new();
        states.insert("signed".into(), ProviderAuthState::SignedIn);
        states.insert("signed-out".into(), ProviderAuthState::NotSignedIn);
        let app = App::with_auth_states(cfg, BTreeSet::new(), states, "test-server".into(), 4);
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("term");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("Signed in"));
        assert!(content.contains("Not signed in"));
        assert!(content.contains("auth=Unavailable"));
    }

    #[test]
    fn signed_in_codex_catalog_is_queryable_before_fetch_not_ready() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut cfg = ProvidersConfig::default();
        cfg.providers.insert(
            "codex".into(),
            pc::Provider {
                kind: "oauth:codex".into(),
                base_url: None,
                key: None,
            },
        );
        let states = ProviderAuthStates::from([("codex".into(), ProviderAuthState::SignedIn)]);
        let app = App::with_auth_states(cfg, BTreeSet::new(), states, "test-server".into(), 4);
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("term");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(content.contains("catalog=Queryable"), "{content}");
        assert!(!content.contains("catalog=Ready"), "{content}");
    }

    #[test]
    fn loaded_catalog_is_rendered_with_models_after_non_empty_fetch() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut wizard = ModelWizard::new(vec!["codex".into()]);
        wizard.provider = "codex".into();
        wizard.role = "main".into();
        wizard.selection = Some(ModelSelection::loading("test-server", "codex", "main"));
        wizard.apply_fetch(Ok(vec!["gpt-5-codex".into()]));
        assert!(matches!(
            wizard.selection.as_ref().map(|selection| &selection.catalog),
            Some(CatalogState::Available(models)) if models == &vec!["gpt-5-codex".to_string()]
        ));
        let mut app = App::new(ProvidersConfig::default());
        app.mode = Mode::SetModel(wizard);

        let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("term");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(content.contains("gpt-5-codex"), "{content}");
    }

    #[test]
    fn failed_catalog_render_preserves_reason_and_manual_fallback() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut wizard = ModelWizard::new(vec!["codex".into()]);
        wizard.provider = "codex".into();
        wizard.role = "main".into();
        wizard.selection = Some(ModelSelection::loading("test-server", "codex", "main"));
        wizard.apply_fetch(Err(ProviderIssue::new(
            "empty_catalog",
            "Server returned no model IDs",
            Some("Retry or enter a model ID"),
        )));
        let mut app = App::new(ProvidersConfig::default());
        app.mode = Mode::SetModel(wizard);

        let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("term");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let failed: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(failed.contains("catalog failed"), "{failed}");
        assert!(failed.contains("Server returned no model IDs"), "{failed}");
        assert!(!failed.contains("catalog=Ready"), "{failed}");

        on_key(&mut app, KeyCode::Char('m'));
        let Mode::SetModel(wizard) = &app.mode else {
            panic!("manual fallback leaves model wizard active");
        };
        assert!(wizard.manual);
        assert!(matches!(
            wizard
                .selection
                .as_ref()
                .map(|selection| &selection.catalog),
            Some(CatalogState::Manual { .. })
        ));

        let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("term");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let manual: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(manual.contains("Server returned no model IDs"), "{manual}");
    }

    #[test]
    fn api_provider_rows_render_key_state_without_secret_bytes() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut cfg = ProvidersConfig::default();
        cfg.providers.insert(
            "without-key".into(),
            pc::Provider {
                kind: "api".into(),
                base_url: Some("https://api.example.test/v1".into()),
                key: None,
            },
        );
        cfg.providers.insert(
            "with-key".into(),
            pc::Provider {
                kind: "api".into(),
                base_url: Some("https://api.example.test/v1".into()),
                key: None,
            },
        );
        let mut app = App::with_auth_states(
            cfg,
            BTreeSet::from(["with-key".to_string()]),
            ProviderAuthStates::new(),
            "test-server".into(),
            5,
        );
        app.ed
            .set_provider(
                "with-key".into(),
                "api".into(),
                None,
                Some("tui-secret-must-not-render".into()),
            )
            .expect("stage replacement key");
        let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("term");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(content.contains("without-key"), "{content}");
        assert!(content.contains("key=Not set"), "{content}");
        assert!(content.contains("with-key"), "{content}");
        assert!(content.contains("key=Set"), "{content}");
        assert!(!content.contains("tui-secret-must-not-render"), "{content}");
    }

    #[test]
    fn successful_save_converts_pending_key_to_presence_and_redacts_editor_state() {
        let mut cfg = ProvidersConfig::default();
        cfg.providers.insert(
            "openai".into(),
            pc::Provider {
                kind: "api".into(),
                base_url: Some("https://api.example.test/v1".into()),
                key: None,
            },
        );
        let mut app = App::with_auth_states(
            cfg,
            BTreeSet::new(),
            ProviderAuthStates::new(),
            "test-server".into(),
            5,
        );
        app.ed
            .set_provider(
                "openai".into(),
                "api".into(),
                None,
                Some("pending-secret".into()),
            )
            .expect("stage key");
        assert!(app.ed.key_present().contains("openai"));

        app.finish_save(Ok(SaveOutcome::Saved));

        assert!(app.ed.key_present().contains("openai"));
        assert!(app
            .ed
            .config()
            .provider("openai")
            .expect("openai")
            .key
            .is_none());
        assert!(app
            .persisted_cfg
            .provider("openai")
            .expect("persisted openai")
            .key
            .is_none());
    }

    #[test]
    fn provider_render_redacts_endpoint_secrets_and_terminal_controls() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut cfg = ProvidersConfig::default();
        cfg.providers.insert(
            "api\u{1b}]52;c;STEAL\u{7}\nnext".into(),
            pc::Provider {
                kind: "api".into(),
                base_url: Some("https://user:pass@example.test/v1?token=SECRET#fragment".into()),
                key: None,
            },
        );
        let app = App::new(cfg);
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).expect("term");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        for secret in ["pass", "SECRET", "#fragment"] {
            assert!(!content.contains(secret), "leaked {secret}: {content}");
        }
        assert!(!content.contains('\u{1b}'), "{content}");
        assert!(!content.contains('\u{7}'), "{content}");
        assert!(
            content.contains("\\u{1b}]52;c;STEAL\\u{7}\\nnext"),
            "{content}"
        );

        let hostile = "wss://user:pass@host/x?token=SECRET#tail\u{1b}]52;c;STEAL\u{7}\r\nnext";
        let mut app = App::new(ProvidersConfig::default());
        app.status = hostile.into();
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).expect("term");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let status: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for forbidden in ["pass", "SECRET", "#tail", "\u{1b}", "\u{7}", "\r"] {
            assert!(
                !status.contains(forbidden),
                "leaked {forbidden:?}: {status}"
            );
        }

        let mut wizard = ModelWizard::new(vec!["codex".into()]);
        wizard.provider = "codex".into();
        wizard.role = "main".into();
        wizard.step = ModelStep::CatalogFailed;
        wizard.selection = Some(ModelSelection {
            connection_id: "server".into(),
            provider: "codex".into(),
            role: "main".into(),
            catalog: CatalogState::Failed(ProviderIssue::new(hostile, hostile, Some(hostile))),
        });
        app.mode = Mode::SetModel(wizard);
        let mut terminal = Terminal::new(TestBackend::new(120, 12)).expect("term");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let catalog: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for forbidden in ["pass", "SECRET", "#tail", "\u{1b}", "\u{7}", "\r"] {
            assert!(
                !catalog.contains(forbidden),
                "leaked {forbidden:?}: {catalog}"
            );
        }
    }

    fn api(base: &str) -> (String, Option<String>, Option<String>) {
        ("api".to_string(), Some(base.to_string()), None)
    }

    #[test]
    fn add_and_dup_provider() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        let (kind, base, key) = api("https://u/v1");
        ed.add_provider("p1".into(), kind, base, key).unwrap();
        assert_eq!(ed.provider_names(), vec!["p1".to_string()]);
        // Duplicate name is rejected.
        assert!(ed
            .add_provider("p1".into(), "api".into(), Some("https://x/v1".into()), None)
            .is_err());
    }

    #[test]
    fn command_and_tui_provider_validation_have_identical_kind_and_remediation() {
        let command_error = fleety_tools::provider_service::validate_provider_input(
            "../escape",
            "api",
            Some("not-an-absolute-url"),
            None,
        )
        .expect_err("command validation");
        let mut editor = ProviderEditor::new(ProvidersConfig::default());
        let tui_error = editor
            .add_provider(
                "../escape".into(),
                "api".into(),
                Some("not-an-absolute-url".into()),
                None,
            )
            .expect_err("TUI validation");
        assert_eq!(tui_error.kind, command_error.kind);
        assert_eq!(tui_error.remediation, command_error.remediation);
        assert!(editor.config().providers.is_empty());
    }

    #[test]
    fn remove_guarded_by_model_reference() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .unwrap();
        ed.set_model(
            "main".into(),
            vec![Member {
                provider: "p1".into(),
                model: "gpt-4o".into(),
                stream: false,
                modalities: None,
                effort: None,
            }],
            Strategy::Single,
        )
        .unwrap();
        // Referenced by the main role → cannot remove (error names the role).
        assert!(ed
            .remove_provider("p1")
            .unwrap_err()
            .to_string()
            .contains("main"));
        // Unreferenced provider removes fine.
        ed.add_provider(
            "p2".into(),
            "api".into(),
            Some("https://u2/v1".into()),
            None,
        )
        .unwrap();
        ed.remove_provider("p2").unwrap();
        assert_eq!(ed.provider_names(), vec!["p1".to_string()]);
    }

    #[test]
    fn set_model_then_save_roundtrips() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .unwrap();
        ed.set_model(
            "main".into(),
            parse_members("p1/gpt-4o|p1/gpt-4o-2").unwrap(),
            Strategy::RoundRobin,
        )
        .unwrap();
        let path = std::env::temp_dir().join(format!("fleety-tui-{}.toml", uuid::Uuid::new_v4()));
        pc::write_providers(&path, ed.config()).expect("save");
        let back = pc::load_from(&path).expect("re-read");
        assert_eq!(back.model("main").unwrap().members.len(), 2);
        assert_eq!(back.model("main").unwrap().strategy, Strategy::RoundRobin);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn model_edit_rejects_dangling_member_before_staging() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        let error = ed
            .set_model(
                "main".into(),
                parse_members("ghost/m").unwrap(),
                Strategy::Single,
            )
            .expect_err("dangling member");
        assert_eq!(error.kind, "invalid_provider");
        assert!(ed.config().models.is_empty());
    }

    #[test]
    fn unset_model_removes_and_rejects_unknown() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .unwrap();
        ed.set_model(
            "main".into(),
            parse_members("p1/gpt-4o").unwrap(),
            Strategy::Single,
        )
        .unwrap();
        ed.unset_model("main").unwrap();
        assert!(ed.cfg.model("main").is_none());
        assert!(ed
            .unset_model("ghost")
            .unwrap_err()
            .to_string()
            .contains("ghost"));
    }

    /// Feed each char of `s` to the app as a key press (input typing helper).
    fn type_str(app: &mut App, s: &str) {
        for c in s.chars() {
            on_key(app, KeyCode::Char(c));
        }
    }

    #[test]
    fn add_next_step_skips_unused_fields() {
        // api (base_url + key): Name → BaseUrl → Key → done.
        assert_eq!(
            add_next_step(AddStep::Name, true, true),
            Some(AddStep::BaseUrl)
        );
        assert_eq!(
            add_next_step(AddStep::BaseUrl, true, true),
            Some(AddStep::Key)
        );
        assert_eq!(add_next_step(AddStep::Key, true, true), None);
        // oauth (no base_url, no key): Name → done.
        assert_eq!(add_next_step(AddStep::Name, false, false), None);
        // base_url only: Name → BaseUrl → done.
        assert_eq!(add_next_step(AddStep::BaseUrl, true, false), None);
    }

    #[test]
    fn add_provider_via_wizard() {
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a')); // wizard: PickType (api at index 0)
        on_key(&mut app, KeyCode::Enter); // choose api → Name step
        type_str(&mut app, "openai1");
        on_key(&mut app, KeyCode::Enter); // → base_url
        type_str(&mut app, "https://u/v1");
        on_key(&mut app, KeyCode::Enter); // → api key
        type_str(&mut app, "sk");
        on_key(&mut app, KeyCode::Enter); // done → add
        assert_eq!(app.ed.provider_names(), vec!["openai1".to_string()]);
        let p = app.ed.cfg.provider("openai1").unwrap();
        assert_eq!(p.kind, "api");
        assert_eq!(p.base_url.as_deref(), Some("https://u/v1"));
        assert_eq!(p.key.as_deref(), Some("sk"));
    }

    #[test]
    fn apply_fetch_picks_list_or_offers_recovery() {
        let mut w = ModelWizard::new(vec!["p1".to_string()]);
        w.selection = Some(ModelSelection::loading("server", "p1", "main"));
        // A non-empty list → searchable pick (not manual), models retained.
        w.apply_fetch(Ok(vec!["gpt-4o".into(), "gpt-4o-mini".into()]));
        assert!(!w.manual);
        assert_eq!(w.step, ModelStep::PickModel);
        assert_eq!(w.filtered().len(), 2);
        // An empty list keeps the failure visible and offers recovery.
        let mut w = ModelWizard::new(vec!["p1".to_string()]);
        w.selection = Some(ModelSelection::loading("server", "p1", "main"));
        w.apply_fetch(Ok(vec![]));
        assert_eq!(w.step, ModelStep::CatalogFailed);
        assert!(!w.manual);
        // A typed error remains inspectable until the user chooses recovery.
        let mut w = ModelWizard::new(vec!["p1".to_string()]);
        w.selection = Some(ModelSelection::loading("server", "p1", "main"));
        w.apply_fetch(Err(ProviderIssue::new("transport", "boom", Some("Retry"))));
        assert_eq!(w.step, ModelStep::CatalogFailed);
        assert!(!w.manual);
        assert!(w.note.contains("boom"));
    }

    /// 'm' with no providers is a no-op with a helpful hint (guard, not a crash).
    #[test]
    fn set_model_needs_a_provider_first() {
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('m'));
        assert!(matches!(app.mode, Mode::Browse));
        assert!(app.status.contains("add a provider"));
    }

    #[test]
    fn horizontal_choices_use_left_right_with_vertical_aliases_and_bounded_edges() {
        let mut add = App::new(ProvidersConfig::default());
        on_key(&mut add, KeyCode::Char('a'));
        assert!(matches!(add.mode, Mode::AddProvider(_)));
        on_key(&mut add, KeyCode::Left);
        assert!(matches!(&add.mode, Mode::AddProvider(w) if w.type_sel == 0));
        on_key(&mut add, KeyCode::Right);
        assert!(matches!(&add.mode, Mode::AddProvider(w) if w.type_sel == 1));
        on_key(&mut add, KeyCode::Up);
        assert!(matches!(&add.mode, Mode::AddProvider(w) if w.type_sel == 0));
        on_key(&mut add, KeyCode::Down);
        assert!(matches!(&add.mode, Mode::AddProvider(w) if w.type_sel == 1));

        let mut role = App::new(ProvidersConfig::default());
        role.ed
            .add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .expect("provider");
        on_key(&mut role, KeyCode::Char('m'));
        on_key(&mut role, KeyCode::Left);
        assert!(matches!(&role.mode, Mode::SetModel(w) if w.role_sel == 0));
        on_key(&mut role, KeyCode::Right);
        assert!(matches!(&role.mode, Mode::SetModel(w) if w.role_sel == 1));
        on_key(&mut role, KeyCode::Up);
        assert!(matches!(&role.mode, Mode::SetModel(w) if w.role_sel == 0));
        on_key(&mut role, KeyCode::Down);
        assert!(matches!(&role.mode, Mode::SetModel(w) if w.role_sel == 1));

        let mut vertical = App::new(ProvidersConfig::default());
        vertical
            .ed
            .add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .expect("provider 1");
        vertical
            .ed
            .add_provider(
                "p2".into(),
                "api".into(),
                Some("https://u2/v1".into()),
                None,
            )
            .expect("provider 2");
        on_key(&mut vertical, KeyCode::Char('m'));
        on_key(&mut vertical, KeyCode::Enter);
        on_key(&mut vertical, KeyCode::Right);
        assert!(matches!(&vertical.mode, Mode::SetModel(w) if w.prov_sel == 0));
        on_key(&mut vertical, KeyCode::Down);
        assert!(matches!(&vertical.mode, Mode::SetModel(w) if w.prov_sel == 1));
        on_key(&mut vertical, KeyCode::Up);
        assert!(matches!(&vertical.mode, Mode::SetModel(w) if w.prov_sel == 0));

        let mut oauth = ProvidersConfig::default();
        oauth.providers.insert(
            "codex".into(),
            pc::Provider {
                kind: "oauth:codex".into(),
                base_url: None,
                key: None,
            },
        );
        let mut oauth_app = App::new(oauth);
        on_key(&mut oauth_app, KeyCode::Char('e'));
        on_key(&mut oauth_app, KeyCode::Right);
        assert!(matches!(&oauth_app.mode, Mode::OauthActions(menu) if menu.sel == 1));
        on_key(&mut oauth_app, KeyCode::Left);
        assert!(matches!(&oauth_app.mode, Mode::OauthActions(menu) if menu.sel == 0));
        on_key(&mut oauth_app, KeyCode::Down);
        assert!(matches!(&oauth_app.mode, Mode::OauthActions(menu) if menu.sel == 1));
        on_key(&mut oauth_app, KeyCode::Up);
        assert!(matches!(&oauth_app.mode, Mode::OauthActions(menu) if menu.sel == 0));

        let mut exit = App::new(ProvidersConfig::default());
        exit.dirty = true;
        on_key(&mut exit, KeyCode::Char('q'));
        on_key(&mut exit, KeyCode::Right);
        assert!(matches!(&exit.mode, Mode::ConfirmExit(confirm) if confirm.sel == 1));
        on_key(&mut exit, KeyCode::Left);
        assert!(matches!(&exit.mode, Mode::ConfirmExit(confirm) if confirm.sel == 0));
        on_key(&mut exit, KeyCode::Down);
        assert!(matches!(&exit.mode, Mode::ConfirmExit(confirm) if confirm.sel == 1));
        on_key(&mut exit, KeyCode::Up);
        assert!(matches!(&exit.mode, Mode::ConfirmExit(confirm) if confirm.sel == 0));
    }

    #[test]
    fn horizontal_choice_hints_show_left_right_and_vertical_lists_keep_up_down() {
        assert!(hints_for(&Mode::ConfirmExit(ExitConfirm { sel: 0 })).contains("←→"));
        assert!(hints_for(&Mode::AddProvider(AddWizard::new())).contains("←→"));
        assert!(hints_for(&Mode::SetModel(ModelWizard::new(vec!["p1".into()]))).contains("←→"));
        assert!(hints_for(&Mode::OauthActions(OauthMenu {
            provider: "codex".into(),
            sel: 0,
        }))
        .contains("←→"));
        assert!(hints_for(&Mode::SetModel(ModelWizard {
            step: ModelStep::PickProvider,
            ..ModelWizard::new(vec!["p1".into()])
        }))
        .contains("↑↓"));
    }

    #[test]
    fn set_model_via_wizard_manual_when_fetch_fails() {
        let mut app = App::new(ProvidersConfig::default());
        app.ed
            .add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .unwrap();
        on_key(&mut app, KeyCode::Char('m')); // PickRole (main at 0)
        on_key(&mut app, KeyCode::Enter); // role=main → PickProvider
        on_key(&mut app, KeyCode::Enter); // provider=p1 → Fetching (flags fetch)
        assert!(app.fetch_models_now);
        // Simulate the run loop's fetch failing, then explicitly choose manual entry.
        if let Mode::SetModel(w) = &mut app.mode {
            w.apply_fetch(Err(ProviderIssue::new(
                "transport",
                "no network",
                Some("Retry"),
            )));
        }
        on_key(&mut app, KeyCode::Char('m'));
        type_str(&mut app, "gpt-4o");
        on_key(&mut app, KeyCode::Enter); // save
        let pool = app.ed.cfg.model("main").unwrap();
        assert_eq!(pool.strategy, Strategy::Single);
        assert_eq!(pool.members.len(), 1);
        assert_eq!(pool.members[0].provider, "p1");
        assert_eq!(pool.members[0].model, "gpt-4o");
    }

    #[test]
    fn set_model_via_wizard_picks_from_fetched_list() {
        let mut app = App::new(ProvidersConfig::default());
        app.ed
            .add_provider("p1".into(), "api".into(), Some("https://u/v1".into()), None)
            .unwrap();
        on_key(&mut app, KeyCode::Char('m'));
        on_key(&mut app, KeyCode::Enter); // role=main
        on_key(&mut app, KeyCode::Enter); // provider=p1
        if let Mode::SetModel(w) = &mut app.mode {
            w.apply_fetch(Ok(vec!["gpt-4o".into(), "gpt-4o-mini".into()]));
        }
        type_str(&mut app, "mini"); // filter down to the one match
        on_key(&mut app, KeyCode::Enter); // pick the highlighted match
        let pool = app.ed.cfg.model("main").unwrap();
        assert_eq!(pool.members.len(), 1);
        assert_eq!(pool.members[0].model, "gpt-4o-mini");
    }

    #[test]
    fn set_provider_upserts_existing_fields() {
        let mut ed = ProviderEditor::new(ProvidersConfig::default());
        ed.add_provider(
            "p1".into(),
            "api".into(),
            Some("https://old/v1".into()),
            Some("k1".into()),
        )
        .unwrap();
        // Upsert replaces the fields without the add-time duplicate guard.
        ed.set_provider(
            "p1".into(),
            "api".into(),
            Some("https://new/v1".into()),
            Some("k2".into()),
        )
        .unwrap();
        let p = ed.cfg.provider("p1").unwrap();
        assert_eq!(p.base_url.as_deref(), Some("https://new/v1"));
        assert_eq!(p.key.as_deref(), Some("k2"));
        assert_eq!(ed.provider_names(), vec!["p1".to_string()]);
    }

    #[test]
    fn changing_provider_type_clears_stale_key_presence() {
        let mut config = ProvidersConfig::default();
        config.providers.insert(
            "p1".into(),
            pc::Provider {
                kind: "api".into(),
                base_url: Some("https://old/v1".into()),
                key: None,
            },
        );
        let mut ed = ProviderEditor::with_key_presence(config, BTreeSet::from(["p1".to_string()]));

        ed.set_provider("p1".into(), "oauth:codex".into(), None, None)
            .expect("change to OAuth");
        ed.finish_save();
        ed.set_provider(
            "p1".into(),
            "api".into(),
            Some("https://new/v1".into()),
            None,
        )
        .expect("change back to API");

        assert!(!ed.key_present().contains("p1"));
    }

    #[test]
    fn clear_key_action_stages_explicit_intent_and_requires_sensitive_confirmation() {
        let mut cfg = ProvidersConfig::default();
        cfg.providers.insert(
            "p1".into(),
            pc::Provider {
                kind: "api".into(),
                base_url: Some("https://api.example.test/v1".into()),
                key: None,
            },
        );
        let mut app = App::new(cfg);
        on_key(&mut app, KeyCode::Char('k'));
        assert_eq!(app.ed.clear_keys(), &BTreeSet::from(["p1".to_string()]));
        assert!(app.dirty);
        on_key(&mut app, KeyCode::Char('s'));
        assert!(matches!(app.mode, Mode::ConfirmSensitiveSave { .. }));
    }

    #[test]
    fn edit_wizard_updates_base_url_and_keeps_blank_key() {
        let mut app = App::new(ProvidersConfig::default());
        app.ed
            .add_provider(
                "p1".into(),
                "api".into(),
                Some("https://old/v1".into()),
                Some("orig".into()),
            )
            .unwrap();
        on_key(&mut app, KeyCode::Char('e')); // api → EditProvider, base_url prefilled
        assert!(matches!(app.mode, Mode::EditProvider(_)));
        // Replace the base_url, then leave the key blank to keep the current one.
        if let Mode::EditProvider(w) = &mut app.mode {
            w.buffer = LineEditor::default();
        }
        type_str(&mut app, "https://new/v1");
        on_key(&mut app, KeyCode::Enter); // → key step
        on_key(&mut app, KeyCode::Enter); // blank key → complete
        let p = app.ed.cfg.provider("p1").unwrap();
        assert_eq!(p.base_url.as_deref(), Some("https://new/v1"));
        assert_eq!(
            p.key.as_deref(),
            Some("orig"),
            "blank key keeps the current one"
        );
    }

    #[test]
    fn adding_an_oauth_provider_goes_straight_to_login() {
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a')); // add wizard: PickType (api at 0)
        on_key(&mut app, KeyCode::Down); // move to oauth:codex (index 1)
        on_key(&mut app, KeyCode::Enter); // pick oauth:codex → Name step
        type_str(&mut app, "tingzhen-codex");
        on_key(&mut app, KeyCode::Enter); // oauth has no base_url/key → Complete
                                          // The provider is staged and a sign-in is pending behind its save.
        assert!(app.ed.cfg.provider("tingzhen-codex").is_some());
        assert!(
            app.auth_request.is_none(),
            "OAuth waits for SaveOutcome::Saved"
        );
        assert!(app.save_now, "saves the new provider before signing in");
        assert!(!app.quit, "does not leave before the saver succeeds");
        app.finish_save(Ok(SaveOutcome::Saved));
        let req = app
            .auth_request
            .as_ref()
            .expect("login released after save");
        assert_eq!(req.action, AuthAction::Login);
        assert_eq!(req.provider, "tingzhen-codex");
        assert!(app.quit, "leaves the editor to run the browser flow");
    }

    #[test]
    fn provider_mutation_is_staged_and_quit_requires_a_decision() {
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a'));
        on_key(&mut app, KeyCode::Enter);
        type_str(&mut app, "openai1");
        on_key(&mut app, KeyCode::Enter);
        type_str(&mut app, "https://u/v1");
        on_key(&mut app, KeyCode::Enter);
        type_str(&mut app, "sk");
        on_key(&mut app, KeyCode::Enter);

        assert!(app.dirty);
        assert!(app.status.contains("staged"));
        on_key(&mut app, KeyCode::Char('q'));
        assert!(matches!(app.mode, Mode::ConfirmExit(_)));
        assert!(!app.quit);
    }

    #[test]
    fn dirty_exit_supports_save_discard_and_cancel() {
        let mut cancel = App::new(ProvidersConfig::default());
        cancel.dirty = true;
        on_key(&mut cancel, KeyCode::Char('q'));
        on_key(&mut cancel, KeyCode::Char('c'));
        assert!(matches!(cancel.mode, Mode::Browse));
        assert!(cancel.dirty);
        assert!(!cancel.quit);

        let mut discard = App::new(ProvidersConfig::default());
        discard.dirty = true;
        on_key(&mut discard, KeyCode::Char('q'));
        on_key(&mut discard, KeyCode::Char('d'));
        assert!(discard.quit);
        assert!(!discard.save_now);

        let mut save = App::new(ProvidersConfig::default());
        save.dirty = true;
        on_key(&mut save, KeyCode::Char('q'));
        on_key(&mut save, KeyCode::Char('s'));
        assert!(save.save_now);
        assert!(!save.quit, "quit waits for a successful save");
    }

    #[test]
    fn endpoint_and_key_changes_require_a_secret_free_second_confirmation() {
        let mut cfg = ProvidersConfig::default();
        cfg.providers.insert(
            "private-api".to_string(),
            pc::Provider {
                kind: "api".to_string(),
                base_url: Some("https://old.example/v1".to_string()),
                key: Some("old-secret".to_string()),
            },
        );
        let mut app = App::new(cfg);
        let provider = app
            .ed
            .cfg
            .providers
            .get_mut("private-api")
            .expect("provider");
        provider.base_url = Some("https://new.example/v1".to_string());
        provider.key = Some("new-secret".to_string());
        app.dirty = true;

        on_key(&mut app, KeyCode::Char('s'));

        assert!(
            !app.save_now,
            "the first save request must not reach ConfigApply"
        );
        let Mode::ConfirmSensitiveSave { summary, .. } = &app.mode else {
            panic!("expected sensitive confirmation");
        };
        assert!(summary.contains("endpoint for 'private-api'"));
        assert!(summary.contains("API key for 'private-api'"));
        assert!(!summary.contains("old-secret"));
        assert!(!summary.contains("new-secret"));

        on_key(&mut app, KeyCode::Enter);
        assert!(
            app.save_now,
            "the confirmed request may now reach ConfigApply"
        );
    }

    #[test]
    fn save_conflict_or_error_keeps_staged_changes_and_blocks_oauth() {
        for result in [
            Ok(SaveOutcome::Conflict("revision changed".into())),
            Err(CoreError::Message("offline".into())),
        ] {
            let mut app = App::new(ProvidersConfig::default());
            app.dirty = true;
            app.request_auth(AuthRequest {
                action: AuthAction::Login,
                provider: "codex1".into(),
            });

            app.finish_save(result);

            assert!(app.dirty);
            assert!(!app.quit);
            assert!(app.auth_request.is_none());
            assert!(app.status.contains("staged"));

            app.finish_save(Ok(SaveOutcome::Saved));

            assert!(app.quit);
            let request = app.auth_request.expect("OAuth action survives save retry");
            assert_eq!(request.action, AuthAction::Login);
        }
    }

    #[test]
    fn save_results_cannot_echo_a_pending_provider_secret() {
        for result in [
            Err(CoreError::Message(
                "Server echoed tui-save-error-secret".into(),
            )),
            Ok(SaveOutcome::Conflict(
                "Server echoed tui-save-error-secret".into(),
            )),
            Ok(SaveOutcome::SavedRefreshRequired(
                "Server echoed tui-save-error-secret".into(),
            )),
        ] {
            let mut cfg = ProvidersConfig::default();
            cfg.providers.insert(
                "openai".into(),
                pc::Provider {
                    kind: "api".into(),
                    base_url: Some("https://api.example.test/v1".into()),
                    key: None,
                },
            );
            let mut app = App::new(cfg);
            app.ed
                .set_provider(
                    "openai".into(),
                    "api".into(),
                    None,
                    Some("tui-save-error-secret".into()),
                )
                .expect("stage key");
            app.dirty = true;

            app.finish_save(result);

            assert!(
                !app.status.contains("tui-save-error-secret"),
                "{}",
                app.status
            );
            assert!(app.status.contains("<redacted>"), "{}", app.status);
        }
    }

    #[test]
    fn only_saved_outcome_releases_pending_oauth_request() {
        let mut app = App::new(ProvidersConfig::default());
        app.dirty = true;
        app.request_auth(AuthRequest {
            action: AuthAction::Switch,
            provider: "codex1".into(),
        });

        app.finish_save(Ok(SaveOutcome::Saved));

        assert!(!app.dirty);
        assert!(app.quit);
        let req = app.auth_request.expect("OAuth released after save");
        assert_eq!(req.action, AuthAction::Switch);
        assert_eq!(req.provider, "codex1");
    }

    #[test]
    fn applied_but_refresh_failed_clears_dirty_and_blocks_every_action_until_exit() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(ProvidersConfig::default());
        app.dirty = true;
        app.finish_save(Ok(SaveOutcome::SavedRefreshRequired(
            "the Server closed before returning a fresh snapshot".into(),
        )));

        assert!(!app.dirty, "the mutation was confirmed by ConfigApply");
        assert!(app.refresh_required);
        assert!(app.status.contains("applied"));
        assert!(app.status.contains("refresh failed"));
        assert!(app.status.contains("reopen"));

        let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("term");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let content = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(content.contains("reload required"), "{content}");
        assert!(content.contains("exit and reopen"), "{content}");

        for code in [
            KeyCode::Char('a'),
            KeyCode::Char('e'),
            KeyCode::Char('d'),
            KeyCode::Char('m'),
            KeyCode::Char('u'),
            KeyCode::Char('s'),
            KeyCode::Enter,
        ] {
            on_key(&mut app, code);
            assert!(matches!(app.mode, Mode::Browse));
            assert!(!app.save_now);
            assert!(!app.dirty);
            assert!(!app.quit);
        }

        on_key(&mut app, KeyCode::Char('q'));
        assert!(app.quit, "reopen is the only recovery path");
    }

    #[test]
    fn oauth_provider_edit_opens_menu_and_records_request() {
        let mut app = App::new(ProvidersConfig::default());
        app.ed
            .add_provider("codex1".into(), "oauth:codex".into(), None, None)
            .unwrap();
        on_key(&mut app, KeyCode::Char('e')); // oauth:codex → OauthActions menu
        assert!(matches!(app.mode, Mode::OauthActions(_)));
        on_key(&mut app, KeyCode::Down); // Sign in → Sign out
        on_key(&mut app, KeyCode::Down); // → Switch account
        on_key(&mut app, KeyCode::Enter);
        assert!(
            app.auth_request.is_none(),
            "OAuth waits for a successful save"
        );
        assert!(app.save_now, "saves before running the action");
        assert!(!app.quit, "does not leave before save succeeds");
        app.finish_save(Ok(SaveOutcome::Saved));
        let req = app.auth_request.as_ref().expect("auth request released");
        assert_eq!(req.action, AuthAction::Switch);
        assert_eq!(req.provider, "codex1");
        assert!(app.quit, "leaves the editor to run the action");
    }

    #[test]
    fn wizard_shows_the_typed_buffer() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a')); // PickType
        on_key(&mut app, KeyCode::Enter); // → Name step (text input)
        for c in "abc".chars() {
            on_key(&mut app, KeyCode::Char(c));
        }
        on_key(&mut app, KeyCode::Left);
        on_key(&mut app, KeyCode::Char('X'));
        let mut terminal = Terminal::new(TestBackend::new(70, 10)).expect("term");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("> abXc"), "typed buffer visible");
        assert!(
            content.contains("add api provider"),
            "wizard prompt visible in title"
        );
    }

    #[test]
    fn fetching_screen_keeps_previous_error_visible_and_can_be_cancelled() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(ProvidersConfig::default());
        let mut wizard = ModelWizard::new(vec!["codex".into()]);
        wizard.provider = "codex".into();
        wizard.role = "main".into();
        wizard.step = ModelStep::Fetching;
        wizard.selection = Some(ModelSelection {
            connection_id: "ws://server.test".into(),
            provider: "codex".into(),
            role: "main".into(),
            catalog: CatalogState::Loading {
                previous_error: Some(ProviderIssue::new(
                    "backend",
                    "catalog temporarily unavailable",
                    Some("Retry"),
                )),
            },
        });
        app.mode = Mode::SetModel(wizard);

        let mut terminal = Terminal::new(TestBackend::new(100, 12)).expect("term");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("fetching codex/models"), "{content}");
        assert!(
            content.contains("previous: catalog temporarily unavailable"),
            "{content}"
        );
        assert!(content.contains("Esc"), "{content}");

        on_key(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Browse));
    }

    #[test]
    fn leaving_fetching_signals_worker_cancellation_and_releases_the_slot() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc};

        let app = App::new(ProvidersConfig::default());
        let cancellation = Arc::new(AtomicBool::new(false));
        let (_sender, receiver) = mpsc::channel();
        let mut active = Some((
            CatalogRequest {
                connection_id: "ws://server.test".into(),
                provider: "codex".into(),
                role: "main".into(),
            },
            receiver,
            Arc::clone(&cancellation),
        ));

        cancel_hidden_catalog_fetch(&app, &mut active);

        assert!(cancellation.load(Ordering::Acquire));
        assert!(active.is_none(), "a second fetch can start immediately");
    }

    #[test]
    fn hints_for_is_nonempty_on_every_screen() {
        // Browse, both add-wizard step kinds, every set-model step, and the input
        // line each expose a non-empty key-hint line.
        assert!(!hints_for(&Mode::Browse).is_empty());
        assert!(!hints_for(&Mode::ConfirmExit(ExitConfirm { sel: 0 })).is_empty());
        assert!(!hints_for(&Mode::AddProvider(AddWizard::new())).is_empty()); // PickType
        let mut add = AddWizard::new();
        add.step = AddStep::Name;
        assert!(!hints_for(&Mode::AddProvider(add)).is_empty()); // text step
        for step in [
            ModelStep::PickRole,
            ModelStep::PickProvider,
            ModelStep::Fetching,
            ModelStep::PickModel,
        ] {
            let mut w = ModelWizard::new(vec!["p".to_string()]);
            w.step = step;
            assert!(!hints_for(&Mode::SetModel(w)).is_empty(), "{step:?}");
        }
        assert!(!hints_for(&Mode::Input {
            action: Action::UnsetModel,
            prompt: "x",
            buffer: LineEditor::default(),
        })
        .is_empty());
    }

    #[test]
    fn browse_hints_survive_a_status_message() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;
        // Add a provider through the wizard (sets status "added provider 'X'"),
        // then render: the status AND the key hints must both be visible.
        let mut app = App::new(ProvidersConfig::default());
        on_key(&mut app, KeyCode::Char('a'));
        on_key(&mut app, KeyCode::Enter); // api → name
        type_str(&mut app, "openai1");
        on_key(&mut app, KeyCode::Enter); // → base_url
        type_str(&mut app, "https://u/v1");
        on_key(&mut app, KeyCode::Enter); // → key
        type_str(&mut app, "sk");
        on_key(&mut app, KeyCode::Enter); // done → Browse, status set
        assert!(matches!(app.mode, Mode::Browse));
        let mut terminal = Terminal::new(TestBackend::new(90, 12)).expect("term");
        terminal.draw(|f| render(f, &app)).expect("draw");
        let content: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(content.contains("added provider"), "status result visible");
        assert!(
            content.contains("a: add") && content.contains("q: quit"),
            "key hints stay visible alongside the status"
        );
    }
}
