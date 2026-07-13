## Context

The current config surface uses ConfigTarget as a location selector, while the registry already records the owning runtime through Scope. This creates contradictory paths: the CLI local path writes Cli and Shared values directly, server ConfigExec is unfiltered, Device targets are rejected, and provider edit still has an explicit local-file mode. The server hub also stores one sender per device id for every connection, so an interactive CLI session can replace the long-lived fleetyd sender needed for device routing.

The existing WebSocket bridge already provides correlated RunTool and ToolResult/ToolError calls with timeout handling. Reusing that path avoids a second cross-platform IPC transport and lets remote device configuration use the authenticated server connection that fleetyd already maintains.

## Goals / Non-Goals

**Goals:**

- Route every CLI config mutation to the runtime that owns the setting.
- Make server and daemon persistence happen inside fleety-server and fleetyd respectively.
- Keep CLI-owned values local while moving Shared ownership to fleetyd because those values affect device runtime behavior.
- Expose Connection, CLI, Daemon, and Server as separate concepts in text and interactive flows.
- Return non-zero status for rejected, unavailable, malformed, and unknown operations.
- Prevent ordinary CLI sessions from replacing daemon routing sessions.

**Non-Goals:**

- Remove direct owner commands such as fleety-server config or fleetyd config when invoked on their own host.
- Introduce a second local IPC socket, named pipe, or new dependency.
- Change config.toml or providers.toml file formats.
- Make daemon configuration work while fleetyd is offline.
- Redesign unrelated ask, voice, audit, rollback, or ACP behavior beyond shared usage and exit-status defects found during this audit.

## Decisions

### Scope determines the owning runtime

Add explicit scope sets: CLI_SCOPES contains Cli, DAEMON_SCOPES contains Daemon and Shared, and SERVER_SCOPES contains Server. Automatic get/set/unset routing looks up the key before any I/O. Provider, model, credential, and provider editor operations always belong to Server. Explicit targets are validated against this ownership table and a mismatch fails before a request or file write.

Shared moves from the CLI local path to fleetyd because its keys control device tools, transport, timezone, and dependency behavior used by the long-lived device runtime. The CLI still observes the same file on later invocations, but it never mutates a Shared value itself.

Alternative rejected: keep local as Cli plus Shared. That leaves daemon changes coupled to an unrelated CLI file write and violates runtime ownership.

### Device configuration reuses the daemon tool bridge

ConfigExec, ConfigSnapshot, and ConfigApply with ConfigTarget::Device are handled by the server by routing an internal, non-advertised RunTool operation to the target fleetyd sender and awaiting the correlated reply. fleetyd recognizes only the reserved internal config operation before its public on-device tool registry and performs scoped config work itself.

The daemon returns structured ConfigEntry values and revision data for snapshot, and validates base revision plus scope and value for apply. Errors return through ConfigResult with actionable kind/message/remediation. No client or server path catches a routing failure by opening the target config file.

Alternative rejected: spawn fleetyd config from fleety. That starts another process which still writes the file directly and does not prove the running daemon accepted the change.

Alternative rejected: add platform-specific local IPC. The existing authenticated bridge already supplies addressing, correlation, timeout, and cross-device operation.

### Only daemon-capable sessions occupy the routing hub

A Hello carrying a successfully parsed local_tools_json marks a daemon-capable device runtime. Only that connection is inserted into Hub and DeviceTools. Interactive CLI connections keep their own outbound sender for replies but never replace the daemon route. Disconnect cleanup removes a hub entry only when it still points to the disconnecting sender, preventing an older session from deleting a newer route.

Alternative rejected: change CLI device identity. Device identity is also used for ownership and conversation history, so inventing a second identity would create a larger semantic break.

### Config command routing is automatic and explicit targets are owner selectors

The CLI parser returns Auto, Server, Daemon, Cli, or Device(id). local remains an accepted compatibility alias for Cli, but help and output use cli. Auto get/set/unset routes by registry scope. Auto provider/model routes to Server. Auto list renders owner-labelled CLI, Daemon, and Server sections; unavailable remote owners are reported and produce a non-zero result rather than silently presenting a partial list as complete.

--target daemon resolves to this machine's stable device id. --target <device-id> addresses another connected fleetyd and accepts only Daemon or Shared keys. --target server accepts only Server keys. --target cli accepts only Cli keys.

### The interactive panel has four owner regions

The settings panel becomes Connection / CLI / Daemon / Server. CLI values are the only config values persisted by the CLI process. Daemon and Server each maintain separate support state, revision, entries, staged changes, and apply target. If the server connection is unavailable, Connection and CLI remain usable while Daemon and Server clearly display unavailable. If fleetyd alone is unavailable, only the Daemon region is unavailable.

Provider and model drill-down always loads and saves the connected server. The former local provider editor route is rejected with migration guidance instead of writing providers.toml.

### Failures and usage are process failures

A ConfigResult with ok=false, a ServerMsg::Error, an unavailable owner, malformed --target, missing required argument, or unknown command returns Err or usage exit code rather than Ok. Bare group names and explicit help remain successful help requests. daemon up and down are normalized to start and stop. fleetyd accepts only no command and run-service as runtime entry points; any unknown command prints usage and exits non-zero instead of starting in foreground.

### Mutations use strict reads and cross-process locks

Config and connection read-modify-write operations acquire a same-directory lock, re-read while holding it, validate, and atomically replace the destination. Present-but-malformed files abort the operation without changing bytes. This closes both fail-soft data loss and stale-snapshot lost updates. FLEETY_DEVICE_ID is removed from the registry because connections.toml is already the authoritative stable identity.


## Implementation Contract

**Behavior**

- fleety config set FLEETY_ADDR VALUE sends ConfigExec or ConfigApply to the connected server and never opens the CLI host config file.
- fleety config set FLEETY_PRESENCE on and every Shared key send a Device target request for the current device id; fleetyd validates and persists the change. If fleetyd is not connected, the command fails and no file is changed by the CLI or server.
- fleety config set FLEETY_VOICE_AUDIO auto is handled by the CLI owner and only Cli scope is writable by that path.
- Explicit target and key-owner mismatches fail before mutation with the expected owner in the message.
- fleety config provider edit and all provider/model commands always target Server. --target local or --target cli for provider/model fails without opening or writing providers.toml.
- Every mutation strict-loads its owner file, and FLEETY_DEVICE_ID is rejected because stable identity is managed by connections.toml.
- The interactive settings panel visibly separates Connection, CLI, Daemon, and Server. Daemon and Server edits use their own snapshot/revision/apply flows.
- A CLI connection sharing the daemon device id does not replace or remove the daemon route.
- Every rejected remote config result, unknown command, and malformed usage returns non-zero. Help requests remain zero.
- No owner routing failure falls back to direct config.toml or providers.toml mutation.

**Interface / data shape**

- fleety CLI target names: auto default, server, daemon, cli, local compatibility alias, and arbitrary device id.
- ConfigTarget::Device(id) is supported for ConfigExec, ConfigSnapshot, and ConfigApply.
- Reserved daemon bridge operations carry config args or structured base_revision plus ConfigChange values and return JSON shapes that the server maps to ConfigResult or ConfigSnapshotResult. Reserved operations are not advertised as agent-callable device tools.
- fleety-tools exports CLI_SCOPES, DAEMON_SCOPES, and SERVER_SCOPES and owner lookup helpers.
- The panel stores independent daemon and server ConfigEntry collections, revisions, staged changes, and availability states.

**Failure modes**

- Server unreachable: server-owned operation fails with connection guidance; no local mutation.
- Daemon offline or displaced: daemon-owned operation fails as not connected; no fallback or mutation.
- Revision conflict: apply fails as conflict and retains staged edits for reload/retry.
- Scope mismatch: client and owner both reject the key.
- Malformed daemon response: server returns a config error and does not report success.
- Existing explicit environment values remain authoritative; successful persistence still reports that restart or environment removal is required.
- A corrupt config/provider/connection file is preserved and reported, never converted into an empty configuration.

**Acceptance criteria**

- Unit tests prove ownership mapping, target parsing, mismatch rejection, daemon-only hub registration, same-sender cleanup, daemon scoped exec/snapshot/apply, revision conflict, and failed routing without file writes.
- CLI smoke tests prove remote ConfigResult errors and invalid command/usage return non-zero, provider local target is rejected, and daemon aliases do not start foreground service.
- Headless panel tests prove four-region navigation and separate daemon/server staged state.
- cargo fmt --all -- --check, cargo clippy --workspace --all-targets -- -D warnings, and cargo test --workspace -- --test-threads=1 pass.
- A manual local smoke with fleety-server and fleetyd verifies server and daemon settings land at their owners only when both runtimes are available.

**Scope boundaries**

- In scope: config ownership, CLI target semantics, daemon bridge routing, hub collision protection, config panel owner regions, provider local fallback removal, usage and config exit status.
- In scope also includes strict mutation and config cross-process serialization found by the same ownership audit.
- Out of scope: unrelated command redesign, new config file formats, offline mutation queues, and local IPC.

## Risks / Trade-offs

- [Daemon settings now require a connected fleetyd] ? Fail explicitly and show start/pair guidance; never weaken the ownership invariant with fallback.
- [Shared keys change owner] ? Keep file format and values unchanged, only route future writes through fleetyd; document that local is now cli.
- [Hub registration change affects device tools] ? Add concurrent daemon plus CLI session regression tests and sender-identity cleanup.
- [One CLI list spans multiple owners] ? Label each section and fail incomplete reads so scripts cannot mistake partial output for a complete snapshot.
- [Older fleetyd cannot execute reserved config operations] ? Return an upgrade-required error from the server; do not direct-write.
- [RunTool timeout can outlive a side-effect] ? Config writes are short and atomic; return timeout as failure and rely on a fresh snapshot before retrying.

## Migration Plan

No data migration is required. Ship server, fleetyd, and fleety together under the existing fleet convergence path. New CLI to old fleetyd fails daemon-owned config with an upgrade instruction. Rolling back restores the former target behavior without changing stored TOML. Documentation and help are updated in the same release.

## Open Questions

None.
