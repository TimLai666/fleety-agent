## ADDED Requirements

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

### Requirement: CLI and daemon share one connection resolver with a single precedence

The CLI and the daemon SHALL resolve the connection target through one shared resolver in `fleety-tools`, with a single precedence: (1) a per-invocation override (`-s/--server <name>` selecting a profile, or `--url <ws>` for an unnamed direct connection); (2) the `FLEETY_AGENT_URL` environment variable as a transient override that is never written to any file; (3) the `current` profile's `url` (and token) from `connections.toml`; (4) mDNS discovery; (5) `ws://127.0.0.1:8787`. A per-invocation override SHALL NOT change `current` or affect the daemon. When the env override is in effect, `server list` / `status` SHALL surface it.

#### Scenario: override does not mutate persistent state

- **WHEN** the user runs `fleety -s office status` while `current` is `home`
- **THEN** the command talks to the `office` profile for that one invocation and `current` remains `home`

#### Scenario: env override is transient and surfaced

- **WHEN** `FLEETY_AGENT_URL` is set in the environment
- **THEN** it wins over the current profile for resolution, is never written into `connections.toml`, and `server list` shows a notice that an env override is active

### Requirement: config.json migrates once and idempotently to connections.toml

On first run with a `config.json` present and no `connections.toml`, the runtime SHALL migrate: create a `default` profile from the old `agent_url`/`token`, lock `device_id` to the existing config.json value (never overwritten by a hostname-derived id), leave `url` empty for a url-less (mDNS-only) record so the resolver still falls to mDNS, and rename the old file to `config.json.migrated` (kept as backup, not deleted). Migration SHALL be idempotent (skipped when `connections.toml` already exists) and concurrency-safe: the writer SHALL create `connections.toml` with an exclusive-create lock so a CLI and a co-located daemon starting at once cannot each migrate and produce two different `device_id`s.

#### Scenario: one-time migration preserves identity

- **WHEN** a device with a `config.json` (agent_url + token + device_id) first runs any fleety/fleetyd command
- **THEN** `connections.toml` is created with a `default` profile carrying the same url/token, `device_id` is unchanged, and `config.json.migrated` appears as a backup

#### Scenario: concurrent first-start yields a single identity

- **WHEN** a CLI and a co-located daemon first start at the same time on a device that still has `config.json`
- **THEN** exactly one of them performs the migration and both end up with the same single `device_id` (no duplicate identity)
