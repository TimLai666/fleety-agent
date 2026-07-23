## Context

`fleety` currently parses its command tree through a large `match` in `crates/fleety-cli/src/main.rs`; group parsers and help text are split among `config.rs`, `server.rs`, `auth.rs`, `fleety-tools::config`, `fleetyd`, and `fleety-server`. The last seven days changed `main.rs` 25 times, which is evidence that the command boundary is a high-churn integration point. Live probes on 2026-07-15 found that `fleety config help`, `fleety ask --help`, and `fleety tui --help` fail non-zero even though the current owner-routed configuration spec requires group help to succeed.

The interactive surfaces are similarly split. `tui.rs` owns chat, `config_panel.rs` owns the four-region settings panel, and `provider_tui.rs` owns providers and models. They use separate state, footer, error, dirty-state, and navigation conventions. The config protocol already provides the correct owner boundary: Server and Daemon mutations travel through `ConfigSnapshot` and `ConfigApply`, while only CLI settings and connection profiles are local client state. The redesign SHALL preserve that boundary.

## Goals / Non-Goals

**Goals:**

- Make the first-run, daily chat, server switching, provider sign-in, model selection, configuration, diagnosis, and automation paths discoverable from one coherent command and TUI model.
- Make every action answer three questions before mutation: which profile is active, which runtime owns the value, and whether the operation was applied, staged, or rejected.
- Eliminate parser/help drift through one typed command definition and exhaustive parser tests.
- Preserve script compatibility while introducing clearer canonical task names.
- Keep partial read results useful without weakening write failure semantics.
- Provide deterministic headless verification for terminal sizes, Unicode, state transitions, and errors.

**Non-Goals:**

- Changing `ConfigSnapshot`／`ConfigApply` ownership or adding direct remote-file access.
- Making a multi-owner settings save atomic. Owners remain independent processes, so the UI SHALL present per-owner apply results.
- Replacing ratatui, changing the on-disk connection/config/provider formats, or redesigning the agent protocol beyond optional read-result metadata required for machine output.
- Removing legacy commands in this release.
- Turning `fleetyd` or `fleety-server` into ordinary end-user command surfaces; they remain host-operation binaries.

## Decisions

### Use one typed command definition and generated help

Adopt `clap` 4 derive for `fleety`, `fleetyd`, and `fleety-server`, plus `clap_complete` for shell completion. Parsing SHALL finish before logging, migration, config seeding, network I/O, or file writes, preserving the current side-effect-free help invariant. Each command is represented once and drives parsing, usage, aliases, suggestions, and help.

This is preferred over extending the hand-written dispatcher because the current dispatcher already has observable help drift and cannot generate exhaustive subgroup help or completion. A home-grown declarative schema avoids a dependency but recreates parser, suggestion, alias, and completion machinery and therefore has a higher long-term defect surface. The accepted exact pins are `clap 4.5.4` and `clap_complete 4.5.2`, whose package metadata declares Rust 1.74 support. Their measured release-size cost is accepted because it buys one generated parser/help/completion boundary. The repository's declared Rust 1.80 baseline is independently false in the pre-existing `agent-workflow`/Boa dependency chain; this change does not claim or attempt to repair that architecture-level MSRV mismatch.

### Organize canonical commands by user task

The canonical tree is:

```text
fleety [chat]
fleety ask [TEXT] [attachments...]
fleety conversations list|resume
fleety connection list|show|add|use|rename|remove|set-url
fleety provider list|add|edit|remove|login|logout|status
fleety model list|set|unset
fleety config [open]|list|get|set|unset
fleety status
fleety doctor
fleety pair <code>|pair-code
fleety audit list|show
fleety rollback list|apply
fleety daemon <lifecycle verb>
fleety completion <shell>
fleety acp [install]
fleety update
```

`fleety server` aliases `fleety connection`; `fleety tui` aliases `fleety chat`; `fleety auth ...` aliases the matching provider command; `fleety config provider|model ...` aliases the top-level provider/model commands. Aliases preserve arguments, stdout, exit status, and mutation target. Interactive terminals receive a one-line stderr deprecation notice; machine-readable output and non-TTY pipelines receive no warning unless `--warnings` is requested.

### Make context global, visible, and non-mutating

`--profile <name>` selects a saved profile for one invocation without changing `connections.toml`. The existing raw URL override remains available as `--server <ws-url>` for compatibility and is explicitly labeled transient. Command output that touches a remote owner includes the resolved profile, URL, server identity when known, and owner in human headers or a JSON `context` object. Secrets and tokens are never included.

Canonical commands infer owner from registry and domain. `--target` remains an expert override for config compatibility, but help presents `--owner server|daemon|cli|<device-id>` as the user-facing spelling and validates it before I/O. `--target` remains an alias.

### Define stable human and machine output modes

Human output is concise and task-oriented. `--json` emits one JSON value with `schema_version`, `ok`, `context`, `data`, and `errors`; `--quiet` suppresses non-result prose. Usage errors exit 2, runtime/owner failures exit 1, and success exits 0. A read that requests multiple owners returns all available owner data plus structured errors and exits 1 if any requested owner failed. A targeted read against one unavailable owner returns no synthetic default.

This avoids the current `config list` behavior that prints CLI data and then terminates at the first unavailable Daemon. It does not weaken mutation safety: writes are single-owner operations and any owner failure returns non-zero without local fallback.

### Use one terminal workspace shell

Bare `fleety` on a TTY opens the workspace; bare `fleety` on non-TTY prints help and exits 0. `fleety chat`, `fleety tui`, and `fleety config` enter the same shell at Chat or Settings respectively.

The shell owns a shared `WorkspaceState` with:

- `Route`: Chat, Conversations, Settings, ConnectionPicker, CommandPalette, Modal.
- `ConnectionState`: Connecting, Connected, Reconnecting, AuthenticationRequired, Offline.
- `Context`: selected profile, endpoint, server identity/version, daemon/device, provider/model, conversation id.
- `OwnerState<T>`: Loading, Available(T), Dirty(T), Applying(T), Conflict(T), Failed(error), Unavailable(reason).
- `Notice`: severity, summary, details, remediation, persistence policy.

The header always shows profile, connection state, model, and active route. The footer is generated from route and modal state. Esc closes the current modal or returns one route; it never has unrelated meanings within the same state. Ctrl+C cancels an in-flight turn first and exits only while idle or after confirmation. `?` opens contextual help and Ctrl+K opens the command palette.

### Make the settings center owner-aware and transactional per owner

Settings navigation is Connection, CLI, Daemon, Server, and Providers & Models. Each page names its destination in user terms, for example `Server settings · office · ws://host:8787`, never `providers.toml`. Rows display label, current value, source, effect timing, and owner; raw `FLEETY_*` keys remain visible as secondary identifiers and searchable aliases.

Edits are staged uniformly. CLI changes are not written on each Enter; Apply writes them through the CLI owner. Daemon and Server Apply use `ConfigApply`. Providers & Models use the connected Server's structured provider payload. Apply acts only on the active owner and reports Saved, Restart required, Conflict, or Failed. Cross-owner dirty pages show separate badges and cannot be represented as one atomic save.

Switching profile while the current Server or Daemon page is dirty opens Apply／Discard／Cancel. Apply must succeed before switching. Discard clears only the old profile's staged remote state. After selection is persisted, the old transport is closed, the selected profile is connected, and fresh Server and Daemon snapshots are loaded. A failed reconnect leaves remote pages unavailable and never reuses the old connection.

### Integrate provider, OAuth, and model discovery

Provider rows show type, endpoint class, authentication state, and roles. OAuth providers show Signed in, Not signed in, Checking, Expired, or Unavailable. Login/logout/status are actions on the selected provider. Model selection follows Provider → catalog loading → searchable models → role confirmation. Catalog failure keeps Retry and Enter model ID actions visible, with the backend error in expandable details.

The same application service backs TUI and non-interactive commands so they cannot diverge in owner routing, OAuth state, model fetch, validation, or error wording.

### Add diagnosis and completion as first-class discovery tools

`fleety doctor` runs bounded read-only checks for the CLI version, current profile resolution, server connection/identity/version, local Daemon installation and connection, config protocol level, Provider configuration, OAuth state, and active model. Each check is PASS, WARN, or FAIL with one remediation command. Any FAIL makes the command non-zero. `--json` uses the common output envelope.

`fleety completion <bash|zsh|fish|powershell|elvish>` writes completion to stdout without touching user files. Installation remains the shell's responsibility and help prints a shell-specific example.

### Verify terminal behavior through state matrices and snapshots

Parser tests enumerate top-level and subgroup `help`, aliases, invalid trailing args, global option placement, JSON envelope, and exit codes. TUI tests render at 120×30, 80×24, 50×16, and below-minimum sizes, with ASCII, CJK, emoji, long URLs, masked secrets, dirty state, errors, and reconnect states. Below the minimum, the shell renders a stable resize message rather than overlapping panes or panicking.

Smoke tests use fake WebSocket owners to prove reads, writes, conflicts, profile switching, no-fallback behavior, and that aliases send byte-equivalent protocol messages to canonical commands.

## Implementation Contract

### Observable behavior

- Every command and subgroup accepts `help`, `--help`, and `-h`, exits 0, and performs no migration, network, or persistence side effect.
- An unknown command reports the nearest valid commands and exits 2.
- Bare `fleety` chooses workspace or help solely from terminal detection; it never starts chat in a pipeline.
- Every remote operation identifies its resolved profile and owner before confirmation and in its result.
- `config list` without owner fetches CLI, Daemon, and Server independently, renders every result, reports unavailable owners, and exits 1 on partial failure. `config list --owner cli` succeeds independently of remote availability.
- All mutations resolve exactly one owner before I/O. Daemon/Server/Provider/Model/OAuth mutations never call local config/provider save functions as fallback.
- Profile switching with remote dirty state requires Apply／Discard／Cancel and reloads snapshots only from the newly selected profile.
- A requested device owner remains part of workspace context through snapshot, staged Apply, profile-switch resolution, and reload; Settings never substitutes the local device ID.
- Invocation profile overrides remain the active Settings identity independently of the persisted current profile; only an explicit profile-switch transaction updates persistence.
- One workspace event stream owns all interactive terminal input, including OAuth return acknowledgement. Route/editor handoff is an acknowledged reader epoch barrier: the reader drains its terminal queue, advances the epoch, and only then allows the new route to accept keys.
- Every human/TUI presentation boundary sanitizes terminal controls. Displayed endpoints remove userinfo, query, and fragment while raw identity and JSON semantic values remain unchanged.
- The workspace remains usable when one remote owner is unavailable; unavailable routes show reason and remediation, while writes on them are disabled.
- Automatic mDNS never borrows stored profile credentials: TXT fingerprints do not authorize sending a stored token or persisting a discovered credentialed endpoint change. Such changes require explicit reselect and re-pair unless a future transport adds cryptographic identity proof.
- Reconnect control keeps one durable pending request until Daemon consumption, rejects a second unsettled request, and settles the consumed request exactly once only after `Welcome`, authentication, and saved-pin verification.
- Structured `ConfigApply` uses one authentication gate before any Server or Device owner dispatch. With authentication disabled, Server rejects Provider or non-`Keep` mutations, Device rejects every apply because its owner persists even empty/all-`Keep` payloads, and Local retains its `invalid` response; a rejection emits no `RunTool`.
- Provider snapshots preserve strictly parsed key-presence metadata through command and TUI views and render only `key=Set` or `key=Not set`; blank keys are invalid, and JSON adds structured boolean `data.providers[].key_present` state beside its compatibility output.

### Interface and data shape

- Human commands follow the canonical tree in this design, with documented aliases.
- Global options are `--profile`, `--server`, `--json`, `--quiet`, `--no-color`, and `--warnings`; config also accepts canonical `--owner` and legacy `--target`.
- JSON output is `{ "schema_version": 1, "ok": bool, "context": {...}, "data": ..., "errors": [...] }`. Each error contains `owner`, `kind`, `message`, and optional `remediation`.
- `WorkspaceState`, `OwnerState`, and route actions are pure state transitions; terminal and network loops consume emitted effects.
- No on-disk schema change is required.
- Reconnect control uses one append-only, nonce-addressed journal with `Submitted`／`Claimed`／`Settled` events. Submit, claim, settle, observe, reap, and Daemon-generation handoff share a short-lived cross-process lease; lock files are never reclaimed from elapsed time alone and each owner removes only its own token. Publisher timeout leaves the active nonce intact, a second request cannot append before terminal settlement is observed, and a torn final record is truncated before the next append. A failed append preserves an already-frozen failure decision for retry; an authenticated success becomes terminal only when its owner-locked append commits, otherwise the attempt freezes a failure after confirming no success committed. Exactly-once applies to the durable terminal result for one nonce, not to transport connection attempts.

### Failure modes

- Parsing and help failure: stderr plus exit 2 for invalid input, stdout plus exit 0 for help.
- Partial read: available data plus owner errors, exit 1, no fallback values.
- Mutation failure or conflict: staged state retained, error visible until dismissed or retried, exit 1 for command mode.
- Reconnect failure: old connection remains closed, remote owner states become Unavailable, local routes remain usable.
- Reconnect settlement failure: stop, deferred restart, resolve, connect, authentication, identity mismatch, and the shared connect＋Hello＋`Welcome` deadline settle a typed failure; graceful exit retries until its frozen terminal result is durable, and other acknowledgement write failures retain retryable settlement state.
- Discovery cannot prove identity: automatic mDNS does not inherit stored profile credentials and directs discovered credentialed endpoint changes to explicit re-pair instead of transparent healing.
- Malformed Provider key-presence metadata rejects the snapshot with an actionable protocol error; it is never partially accepted.
- Terminal too small: render one resize instruction and accept quit/help keys.

### Acceptance criteria

- Parser unit tests cover every command node and alias; CLI smoke tests assert stdout/stderr and exact exit classes.
- Side-effect tests compare temp-home bytes before and after all help and failed remote mutations.
- Protocol tests prove canonical and alias commands target the same owner and payload.
- Recording-owner tests prove requested remote device IDs survive every Settings snapshot/apply/reload frame, and injected event tests prove stale route input and OAuth acknowledgement cannot create a second reader or unintended Apply.
- Recording-owner auth tests prove an auth-disabled Device `ConfigApply` sends zero `RunTool` frames, while a pure owner matrix locks Server no-op, Device empty/all-`Keep`, Local, and auth-required behavior.
- Deterministic reconnect tests cover caller timeout, duplicate request rejection, delayed consumption, `Welcome` delay, identity mismatch, stop/restart exits, acknowledgement write failure, and exactly-once settlement.
- Resolver and CLI/Daemon connection tests prove automatic mDNS never carries a stored token and never persists a credentialed endpoint change from TXT metadata alone.
- Provider command and headless TUI tests prove strict key-presence parsing, add／set／keep／clear transitions, production snapshot-to-editor wiring, structured JSON state, and `key=Set`／`key=Not set` parity without secret bytes.
- OAuth delivery tests inject a browser process that exits non-zero immediately and prove the bounded launcher path reaches clipboard fallback without blocking.
- Repository checks prove every generated Spectra archive instruction retains `.spectra/touched/<change>.json` until `spectra archive` succeeds, and protocol history documents config protocol v5.
- Headless Settings/OAuth/notice render tests prove endpoint credentials and terminal controls are absent from human/TUI output while machine data remains semantically raw.
- Headless TUI snapshots cover the size/state matrix and contain no replacement glyph `�` or clipped key instruction at supported sizes.
- Full workspace tests, clippy with warnings denied, formatting, Spectra validation, and release build pass.
- A separate evaluation agent performs a heuristic review after each implementation round; all Critical/High findings are fixed or rejected with evidence before completion.

### Scope boundaries

In scope are the three binary parsers, Fleety command naming and output, the shared terminal workspace, settings/provider/model flows, owner routing and authentication enforcement, reconnect control, stored-profile credential policy for mDNS, diagnostics, completion, tests, and documentation. Out of scope are server agent behavior, tool registry UX, wire-level chat features, explicit `FLEETY_TOKEN` or `FLEETY_PAIRING_CODE` use without an explicit endpoint, configuration file schemas, automatic shell-file modification, and transparent endpoint healing without TLS or public-key identity proof.

## Risks / Trade-offs

- [Risk] `clap` and `clap_complete` increase compile time and binary size → Measure release artifacts before/after, disable unused features, and reject the dependency only if the measured cost exceeds the agreed threshold.
- [Risk] Canonical renames confuse existing users → Keep aliases for at least one minor release, show warnings only in interactive human mode, and document an exact mapping table.
- [Risk] One TUI shell creates a large state machine → Keep state transitions pure, split routes into modules, and centralize only shared context/effects rather than widget internals.
- [Risk] Partial reads can be mistaken for success → Human output labels `PARTIAL`; JSON sets `ok: false`; process exits 1 whenever any requested owner failed.
- [Risk] Applying dirty state before profile switch can change a server the user intended to leave → Require an explicit Apply／Discard／Cancel choice and display the old profile in the modal.
- [Risk] Owner labels can expose endpoints in logs → Never print tokens or secrets; permit `--quiet`; JSON context contains endpoint but no credentials.
- [Risk] Cross-platform terminals disagree on key events and width → Prefer broadly supported keys, use Unicode-width-aware layout, preserve textual command alternatives, and test Windows plus Unix CI.
- [Risk] Removing TXT-based sticky healing adds a manual re-pair step after a credentialed Server address changes → Prefer explicit recovery over sending a stored credential to an identity that mDNS cannot prove; revisit only with cryptographic proof.
- [Risk] A crash can leave a stale control or mutation lock → Never reclaim from elapsed time alone; report recoverable lock ownership now, and add an explicit owner-proven cleanup/status command before automating reclamation.

## Migration Plan

1. Add typed parsers behind compatibility aliases and lock help/exit behavior with tests before moving command implementations.
2. Introduce shared command services and JSON envelope, then route old and canonical names through the same functions.
3. Add `WorkspaceState` and shell navigation while preserving `fleety tui` and current chat behavior.
4. Move config and provider routes into the shell, then delete duplicated standalone render/state code only after parity tests pass.
5. Update documentation and completion examples; retain aliases and interactive warnings for at least one release.
6. Rollback is a code rollback only. No data migration is needed, and existing config files remain readable by older binaries.

## Resolved Questions

- Adding exact `clap 4.5.4` and `clap_complete 4.5.2` is accepted. The final Windows release artifacts increased by 12.23% for `fleety`, 3.54% for `fleetyd`, and 0.82% for `fleety-server`; the generated command contract and completion support justify this bounded cost. A real Cargo 1.80 check was attempted and reached the pre-existing `agent-workflow → boa_engine 0.21.1 → time ^0.3.44` boundary after temporarily making the grep chain parseable. All diagnostic dependency downgrades were reverted, so the change contains no hidden MSRV workaround.
