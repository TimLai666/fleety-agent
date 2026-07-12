# connection-profiles Specification

## Purpose

TBD - created by archiving change 'connection-profiles'. Update Purpose after archive.

## Requirements

### Requirement: Connection profiles are the single persistent source of the connection target

The connection target (which server + its token) SHALL live in one file, `~/.fleety/connections.toml`, holding a device-wide `device_id`, a `current` profile name, and named `profiles` each carrying `url`, an optional `token`, an optional `label`, and an optional server `fingerprint`. The file SHALL be written atomically (temp + rename) with `0600` permissions. Loading a missing file SHALL yield an empty set (not an error); loading a present-but-unparseable file SHALL return an explicit error rather than being silently treated as empty. `FLEETY_AGENT_URL` SHALL NOT be a registry setting — `config set FLEETY_AGENT_URL` returns an unknown-key error and the value is never seeded from `config.toml`.

#### Scenario: profiles round-trip with restricted permissions

- **WHEN** a profile is added and `connections.toml` is written then read back
- **THEN** the profile's url/token/label survive the round-trip and the file's permissions are `0600`

#### Scenario: a corrupt connections.toml is a hard error, not empty

- **WHEN** `connections.toml` exists but cannot be parsed
- **THEN** the resolver returns an explicit error and does not silently fall back as if there were no connection configured

#### Scenario: FLEETY_AGENT_URL is no longer a config key

- **WHEN** the user runs `fleety config set FLEETY_AGENT_URL ws://x`
- **THEN** it is rejected as an unknown setting (the connection target is managed via `fleety server`, not the registry)


<!-- @trace
source: connection-profiles
updated: 2026-07-10
code:
  - crates/fleety-cli/src/server.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/config.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: The fleety server command group manages named server profiles

The CLI SHALL provide a `fleety server` command group to manage profiles: `add <name> <url>` (with optional `--label`, `--pair <code>`, `--use`), `use <name>`, `list`, `show [<name>]`, `current`, `rename <old> <new>`, `remove <name>`, and `set-url <name> <url>`. `use` SHALL change only the `current` field. `list` SHALL mark the current profile and, when an env override is in effect, print a prominent notice at the top. Removing the current profile SHALL require switching to another first or an explicit `--force`. `fleety init <url>` SHALL be equivalent to `server add <name> <url> --use` plus enrollment, and `fleety pair <code>` SHALL pair the current profile and write the minted token back into that profile — both preserving their existing invocation forms for backward compatibility.

#### Scenario: add then use selects the connection

- **WHEN** the user runs `fleety server add home ws://h:8787 --use`
- **THEN** `fleety server current` prints `home` and later commands connect to `ws://h:8787`

#### Scenario: init and pair are sugar over profiles

- **WHEN** the user runs `fleety init ws://x` then `fleety pair CODE`
- **THEN** a `default` profile is created and switched to, and the minted token is written into that profile (not into a separate flat file)

#### Scenario: removing the current profile is guarded

- **WHEN** the user runs `fleety server remove <current>` without `--force`
- **THEN** it is rejected with a prompt to switch to another profile first


<!-- @trace
source: connection-profiles
updated: 2026-07-10
code:
  - crates/fleety-cli/src/server.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/config.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: CLI and daemon share one connection resolver with a single precedence

The CLI and the daemon SHALL resolve the connection target through one shared resolver in `fleety-tools`, with a single precedence: (1) a per-invocation override (`-s/--server <name>` selecting a profile, or `--url <ws>` for an unnamed direct connection); (2) the `FLEETY_AGENT_URL` environment variable as a transient override that is never written to any file; (3) the `current` profile's `url` (and token) from `connections.toml`; (4) mDNS discovery; (5) `ws://127.0.0.1:8787`. A per-invocation override SHALL NOT change `current` or affect the daemon. When the env override is in effect, `server list` / `status` SHALL surface it.

#### Scenario: override does not mutate persistent state

- **WHEN** the user runs `fleety -s office status` while `current` is `home`
- **THEN** the command talks to the `office` profile for that one invocation and `current` remains `home`

#### Scenario: env override is transient and surfaced

- **WHEN** `FLEETY_AGENT_URL` is set in the environment
- **THEN** it wins over the current profile for resolution, is never written into `connections.toml`, and `server list` shows a notice that an env override is active


<!-- @trace
source: connection-profiles
updated: 2026-07-10
code:
  - crates/fleety-cli/src/server.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/config.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: config.json migrates once and idempotently to connections.toml

On first run with a `config.json` present and no `connections.toml`, the runtime SHALL migrate: create a `default` profile from the old `agent_url`/`token`, lock `device_id` to the existing config.json value (never overwritten by a hostname-derived id), leave `url` empty for a url-less (mDNS-only) record so the resolver still falls to mDNS, and rename the old file to `config.json.migrated` (kept as backup, not deleted). Migration SHALL be idempotent (skipped when `connections.toml` already exists) and concurrency-safe: the writer SHALL create `connections.toml` with an exclusive-create lock so a CLI and a co-located daemon starting at once cannot each migrate and produce two different `device_id`s.

#### Scenario: one-time migration preserves identity

- **WHEN** a device with a `config.json` (agent_url + token + device_id) first runs any fleety/fleetyd command
- **THEN** `connections.toml` is created with a `default` profile carrying the same url/token, `device_id` is unchanged, and `config.json.migrated` appears as a backup

#### Scenario: concurrent first-start yields a single identity

- **WHEN** a CLI and a co-located daemon first start at the same time on a device that still has `config.json`
- **THEN** exactly one of them performs the migration and both end up with the same single `device_id` (no duplicate identity)

<!-- @trace
source: connection-profiles
updated: 2026-07-10
code:
  - crates/fleety-cli/src/server.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-tools/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/config.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Server fingerprints are pinned at pairing and on authenticated connections

When pairing succeeds (a `Welcome` that mints a token), the client SHALL pin the server's advertised identity fingerprint into the current profile alongside the token. On any later successfully authenticated connection whose `Welcome` carries a fingerprint: a profile without one SHALL be back-filled (trust-on-authenticated-connect, so devices enrolled before fingerprints exist need no re-pairing); a profile whose pinned fingerprint differs SHALL NOT be overwritten — the client SHALL warn that the server identity changed and point at re-pairing.

#### Scenario: pairing pins the fingerprint

- **WHEN** a device pairs with a server that advertises an identity fingerprint
- **THEN** the current profile stores that fingerprint next to the minted token

#### Scenario: an enrolled device back-fills its pin

- **WHEN** a device paired before fingerprints existed connects successfully and the Welcome carries a fingerprint
- **THEN** the profile gains that fingerprint without re-pairing

#### Scenario: an identity change warns and never overwrites

- **WHEN** an authenticated connection reports a fingerprint different from the pinned one
- **THEN** the client warns that the server identity changed, keeps the pinned value, and suggests re-pairing


<!-- @trace
source: sticky-heal-fingerprint
updated: 2026-07-12
code:
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-server/src/mdns.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/update.rs
  - docs/env.md
  - crates/fleety-protocol/src/lib.rs
  - scripts/install-server.sh
  - crates/fleety-server/src/main.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: Sticky connections heal by fingerprint when the address moves

When connecting to the current profile's URL fails, the profile carries a pinned fingerprint, and mDNS is not disabled, the client SHALL run one discovery scan and consider ONLY advertisers whose fingerprint exactly equals the pinned value: on a match at a different URL it SHALL persist the new URL to the profile, report that the server moved, and reconnect with the existing token; a match at the same URL, no match, or fingerprint-less advertisers SHALL leave the profile untouched and surface the original connection failure. The stored token SHALL never be sent to an advertiser whose fingerprint does not match the pin. A successful connection SHALL never trigger a scan (sticky behavior is unchanged on the happy path). The CLI SHALL heal at most once per command invocation; the daemon SHALL attempt the same heal inside its reconnect loop before each backoff sleep.

#### Scenario: the server moves to a new IP

- **WHEN** the saved URL stops answering and a scan finds an advertiser with the pinned fingerprint at a new URL
- **THEN** the profile's URL is updated, the client reports the move, and the connection proceeds with the existing token

#### Scenario: a different server on the LAN is never adopted

- **WHEN** the saved URL stops answering and the scan finds only advertisers with different or absent fingerprints
- **THEN** the profile is unchanged, no token is sent to any of them, and the original failure is reported

#### Scenario: healthy connections never scan

- **WHEN** the current profile's URL answers
- **THEN** no discovery scan runs and the connection proceeds as today

<!-- @trace
source: sticky-heal-fingerprint
updated: 2026-07-12
code:
  - crates/fleety-daemon/src/poll_updates.rs
  - crates/fleety-tools/Cargo.toml
  - crates/fleety-server/src/mdns.rs
  - crates/fleety-tools/src/connection.rs
  - crates/fleety-server/src/conn.rs
  - README.md
  - crates/fleety-cli/src/main.rs
  - crates/fleety-daemon/src/main.rs
  - crates/fleety-tools/src/update.rs
  - docs/env.md
  - crates/fleety-protocol/src/lib.rs
  - scripts/install-server.sh
  - crates/fleety-server/src/main.rs
tests:
  - crates/fleety-cli/tests/cli_smoke.rs
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
-->

---
### Requirement: The local server is a first-class default switchable profile

When the CLI runs guided `fleety init` (no URL, on a TTY) it SHALL probe for a local server (connect to `ws://127.0.0.1:<port>`, port taken from `FLEETY_ADDR` or the default) with a short timeout before scanning the LAN. When a local server answers, it SHALL be presented at the top of the picker as the default choice, marked as local, and selecting it SHALL save a `local` profile pointing at it, make it current, and SKIP the pairing-code prompt (the connection is loopback-trusted). LAN-discovered servers SHALL still be listed below it. Switching to another server SHALL use the existing profile mechanism (`fleety server use <name>`), and switching back SHALL use the `local` profile; configuration and every other command land on whichever profile is current — unchanged. When no local server answers, guided init SHALL behave exactly as before (LAN scan, or usage guidance).

#### Scenario: local server is the default pick and needs no pairing

- **WHEN** guided `fleety init` runs on a host whose local server answers
- **THEN** `local` is listed first as the default, selecting it saves and uses the `local` profile, and no pairing code is requested

#### Scenario: local coexists with LAN servers and is switchable

- **WHEN** a local server and LAN servers are both present
- **THEN** the picker lists `local` first and the LAN servers below, and the user can pick either; a later `fleety server use` switches the current profile

#### Scenario: no local server falls back to the existing flow

- **WHEN** guided init runs on a host with no local server
- **THEN** the probe times out quietly and the LAN scan / usage guidance runs as before

<!-- @trace
source: local-server-trust
updated: 2026-07-12
code:
  - crates/fleety-server/src/http.rs
  - crates/fleety-server/src/conn.rs
  - scripts/install-server.sh
  - docs/env.md
  - crates/fleety-protocol/src/lib.rs
  - crates/fleety-cli/src/main.rs
  - crates/fleety-server/src/main.rs
tests:
  - crates/fleety-daemon/tests/fleetyd_smoke.rs
  - crates/fleety-cli/tests/cli_smoke.rs
-->