## Context

Codex OAuth today is one global credential shared by every `oauth:codex` provider:

- Client: `fleety_tools::oauth::default_token_path()` resolves `~/.fleety/codex-oauth.json` (single file).
- Protocol (`fleety-protocol`): `config_protocol = 2`; the credential frames `CredentialPut { kind, payload_json }`, `CredentialStatus { kind }`, `CredentialDelete { kind }` key only by `kind` ("codex-oauth"). Replies: `CredentialResult`, `CredentialStatusResult`.
- Server (`fleety-server/src/conn.rs`): `credential_put` / `credential_status` / `credential_delete` operate on one token path; `providers.rs` builds the Codex Responses provider for any `oauth:codex` provider from that same global path.
- CLI (`fleety-cli/src/auth.rs`): `fleety auth login|status|logout` with no provider name.

This makes different Codex accounts per provider impossible, and the new guided provider editor can add an `oauth:codex` provider that can never be signed in from the UI.

## Goals / Non-Goals

In scope: bind Codex credentials to the provider name across client store, protocol, server store, and provider runtime; per-provider auth login/logout/status; provider-editor UX (persistent hints, edit existing provider, OAuth sign-in/out/switch for `oauth:codex`). Out of scope: migrating the old global credential (it is cleared), non-Codex OAuth, api-provider key storage, and the remote `config provider edit` OAuth flow.

## 決策一:憑證以 provider 名為 key(protocol 2 to 3)

`config_protocol` becomes `3`. The three credential request frames gain an optional provider:

- `CredentialPut { kind, provider: Option<String>, payload_json }`
- `CredentialStatus { kind, provider: Option<String> }`
- `CredentialDelete { kind, provider: Option<String> }`

`provider` is `Option<String>` (serde default `None`) so the wire stays parseable across versions. For `kind == "codex-oauth"` the new server REQUIRES `provider = Some(name)`; a `None` (an old CLI) is rejected with a `CredentialResult` / `CredentialStatusResult` error whose message is "this server stores Codex credentials per provider — update fleety and run auth login for the provider". The reply frames (`CredentialResult`, `CredentialStatusResult`) are unchanged in shape.

Rationale for `Option` over a required field or a new frame: keeps one frame set, lets non-Codex kinds (future) omit it, and makes the "old client" case a clean server-side rejection rather than a deserialize failure.

## 決策二:token 儲存在 server,per-provider(client 不落地)

Per the existing codex-oauth spec, the **client never stores tokens** — `login` runs the OAuth flow and delivers the tokens to the connected server, which persists them (0600) and refreshes them. So per-provider storage is a **server** concern, not a client one. `fleety_tools::oauth` gains `token_path_for(provider: &str) -> PathBuf` giving `<store-root>/codex-oauth/<provider>.json` (the store root being the same `~/.fleety` the current global path uses, resolved on the **server** host); the directory is created 0700 on Unix, files 0600 (same as today's single file). `FLEETY_CODEX_TOKENS`, when set, still overrides to a single explicit path for the single-provider test flow (unchanged semantics; used by tests). `default_token_path()` is retained only for the one-time legacy cleanup below and is otherwise unused by the per-provider paths. The client keeps its existing behavior of deleting/flagging a leftover legacy *local* token file; it stores nothing new.

Provider names are already validated by `providers.toml` (BTreeMap keys); the token filename uses the name verbatim. Names cannot contain path separators because `providers_config` validation rejects them, so no sanitisation beyond that is required (the design assumes the existing provider-name validation; a name with a separator reaching here would be a providers.toml validation bug, not this layer's concern).

## 決策三:清除舊全域憑證(不遷移)

On first run of the new version the **server** deletes its legacy global token file (the current `default_token_path()` on the server host) on startup if present, best-effort, logging one line. No credential is migrated; each provider must sign in fresh through the per-provider path. The client's existing leftover-legacy-*local*-file cleanup is unchanged (that file was never read by the current flow anyway). A shared helper `oauth::clear_legacy_global(path)` performs the delete idempotently.

## 決策四:CLI auth per-provider

`fleety auth` dispatch (`auth::run`):

- `login <provider> [--no-browser]` runs the existing OAuth browser+loopback flow and **delivers** the tokens to the connected server via `CredentialPut { kind: "codex-oauth", provider: Some(provider), payload_json }`; it does not persist anything on the CLI host (unchanged from today, except the added provider). A missing `<provider>` argument is a usage error naming an example.
- `logout <provider>` sends `CredentialDelete { kind: "codex-oauth", provider: Some(provider) }` so the server removes that provider's stored credential.
- `status [<provider>]` with a name sends `CredentialStatus { provider: Some(name) }` (presence + expiry). With no name it enumerates the `oauth:codex` providers from the connected server's provider config and queries each, printing one line per provider (signed in? expiry).
- Before any of these, if the server's `config_protocol < 3` (from `Welcome`), it errors: "this server does not support per-provider Codex credentials yet — update fleety-server" and opens no browser. No silent global fallback. (This tightens the existing protocol-2 check to protocol-3 for the per-provider flow.)

`login` / `logout` validate that `<provider>` exists and is of type `oauth:codex` in the connected server's provider config, erroring by name otherwise.

## 決策五:server 憑證存放 + oauth provider 解析(conn.rs / providers.rs)

`fleety-server/src/conn.rs`: `credential_put` / `credential_status` / `credential_delete` take the provider name and resolve the server's per-provider token path (the server's protected store dir plus `codex-oauth/<provider>.json`). A `codex-oauth` frame with `provider = None` returns the rejection from 決策一. `providers.rs` `build_codex_provider` resolves the token path from the provider's own name (the `ProviderSpec` already carries the name), so provider `tingzhen-codex` reads `codex-oauth/tingzhen-codex.json`. The server clears the legacy global store on startup (決策三).

## 決策六:provider 編輯器 — 常駐提示 + 編輯 + OAuth 動作

`provider_tui.rs`:

- **Persistent hints**: the bottom region renders two lines — a fixed key-hint line for the current mode (Browse / add-wizard step / set-model step / edit) plus a status line below it — so a message like "added provider 'X'" occupies the status line and never overwrites the hints (mirrors the fix already in `config_panel.rs`). Every screen (Browse, `AddWizard`, `ModelWizard`, the new edit flow) supplies its own hint text.
- **Edit an existing provider**: Browse gains `e` on the selected provider. For an `api` provider it opens an edit wizard prefilled with the current `base_url` plus masked `key` (name fixed); saving calls a new `ProviderEditor::set_provider(name, kind, base_url, key)` (upsert — replaces the existing entry without the add-time duplicate guard). For an `oauth:codex` provider, `e` opens an OAuth action submenu (below).
- **OAuth actions**: for a selected `oauth:codex` provider, offer sign in / sign out / switch account (switch = sign out then sign in). Because the OAuth flow is async, opens a browser, runs a loopback server, and needs the plain terminal, the editor cannot run it inline. Instead it saves the current config, sets an outcome `AuthRequest { action: AuthAction, provider: String }`, and quits the TUI. `provider_tui::run` returns `Result<Option<AuthRequest>>`; `config_panel::run_providers` becomes `async`, runs the auth action (`crate::auth::login` / `logout` for the provider) after `ratatui::restore()`, prints the result, then reopens the editor so the user continues. `AuthAction::Switch` runs logout then login.

`config_panel::run` already awaits; `run_providers` changes from `fn -> Result<()>` to `async fn -> Result<()>` and its call site is awaited.

## 決策七:back-compat 與錯誤路徑

- New CLI + old server (`config_protocol < 3`): `auth` commands and editor OAuth actions error with an "update fleety-server" message; nothing is written globally.
- Old CLI + new server: the old CLI sends a `codex-oauth` frame with no `provider`; the server rejects it with the update-your-CLI message. The old CLI's non-credential paths are unaffected.
- A provider named in an `auth` command that does not exist or is not `oauth:codex`: error by name.

## Implementation Contract

**Behavior:**

- `fleety auth login <provider>` signs that provider's Codex account in and stores the credential under the provider name on both client and server; two providers can hold two different accounts; re-running it on the same provider switches its account.
- `fleety auth logout <provider>` / `status <provider>` / `status` (all) act per provider.
- The provider editor: key hints are always visible; an existing api provider is editable; an `oauth:codex` provider can be signed in / out / switched from the editor via exit then run then reopen.
- Upgrading clears the old global Codex credential; providers re-login fresh.

**Interface / data shape:**

- Protocol: `config_protocol = 3`; `CredentialPut` / `CredentialStatus` / `CredentialDelete` carry `provider: Option<String>`; replies unchanged.
- `fleety_tools::oauth::token_path_for(&str) -> PathBuf` (server-side store), `clear_legacy_global(&Path)`.
- `ProviderEditor::set_provider(name, kind, base_url, key)` upsert.
- `provider_tui::run(&Path) -> Result<Option<AuthRequest>>`; `enum AuthAction { Login, Logout, Switch }`; `config_panel::run_providers` is async.

**Failure modes:**

- Server `config_protocol < 3` gives a per-provider auth error with an update-server message (no global write).
- `codex-oauth` credential frame without `provider` gives a server rejection with an update-CLI message.
- Unknown or non-oauth provider in an auth command errors by name.
- OAuth browser/loopback failure surfaces the existing auth error; the editor reopens with the failure in the status line.

**Acceptance criteria:**

- Unit tests: protocol round-trips the new `provider` field (present and `None`); server credential put/status/delete are per-provider (two providers isolated) and reject a `None`-provider codex frame; `token_path_for` maps name to path; `ProviderEditor::set_provider` upsert; a pure hint-for-screen function returns non-empty hints for every screen; `AuthAction` submenu navigation; a pure parse of the auth login argument (missing name gives a usage error).
- Manual: add two `oauth:codex` providers, sign in provider a then provider b with different accounts, confirm `status` lists both distinct; switch account on a; sign out a; edit an api provider base_url in the editor; confirm hints never disappear behind status text.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all -- --check` clean.

**Scope boundaries:**

- In: fleety-protocol credential frames plus config_protocol bump; fleety-tools oauth per-provider paths plus legacy clear; fleety-cli auth per-provider plus provider editor (hints/edit/oauth actions) plus config_panel async run_providers; fleety-server per-provider credential store plus oauth:codex resolution; the two smoke-test constructors; the four spec deltas; README/env/design-cli-config docs.
- Out: migration of the global credential; non-Codex OAuth; api key storage; multi-member model editing; remote config provider edit OAuth actions.

## Risks / Trade-offs

- **Protocol bump touches every credential call site plus smoke tests.** Mitigated by making `provider` an `Option` (parseable across versions) and centralising the "codex needs a provider" rule server-side.
- **Async OAuth from a sync TUI.** Mitigated by exit then run then reopen rather than running the browser flow under ratatui; the editor state is reloaded from the saved file on reopen.
- **No migration is a breaking change for anyone already logged in.** Accepted per decision ("clear and re-login"); the one-time legacy cleanup plus the clear per-provider error messages make the required action obvious.
- **Provider-name-as-filename** relies on existing providers.toml name validation; documented as an assumption, not re-validated here.
