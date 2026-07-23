<!-- SPECTRA:START v1.0.2 -->

# Spectra Instructions

This project uses Spectra for Spec-Driven Development(SDD). Specs live in `openspec/specs/`, change proposals in `openspec/changes/`.

## Use `$spectra-*` skills when:

- A discussion needs structure before coding → `$spectra-discuss`
- User wants to plan, propose, or design a change → `$spectra-propose`
- Tasks are ready to implement → `$spectra-apply`
- There's an in-progress change to continue → `$spectra-ingest`
- User asks about specs or how something works → `$spectra-ask`
- Implementation is done → `$spectra-archive`
- Commit only files related to a specific change → `$spectra-commit`

## Workflow

discuss? → propose → apply ⇄ ingest → archive

- `discuss` is optional — skip if requirements are clear
- Requirements change mid-work? `ingest` → resume `apply`

## Parked Changes

Changes can be parked（暫存）— temporarily moved out of `openspec/changes/`. Parked changes won't appear in `spectra list` but can be found with `spectra list --parked`. To restore: `spectra unpark <name>`. The `$spectra-apply` and `$spectra-ingest` skills handle parked changes automatically.

<!-- SPECTRA:END -->

# Change completeness — update every parallel surface

When you change a behavior, find and update **every place that implements or
exposes it**. A feature that lives in one surface but not its siblings is a bug,
not a smaller feature. Before finishing a change, walk these parallel-surface
families in this repo and confirm each is either updated or genuinely N/A:

- **Connection / server selection** — the guided `fleety init` picker
  (`fleety-cli/src/main.rs`), the three-region config panel Connection region
  (`config_panel.rs`), `fleety server` subcommands, and the shared resolver
  (`fleety-tools/src/connection.rs`). New "which server / how we pick it"
  behavior belongs in all of them, not just `init`.
- **Client connection helpers** — the CLI one-shot path (`open` /
  `connect_hello`) **and** the daemon reconnect loop (`fleety-daemon/src/main.rs`).
  Sticky healing, fingerprint pinning, auth-rejection handling, etc. must land
  in both.
- **Protocol frame changes** (`fleety-protocol/src/lib.rs`) — the server emit
  site, **every** client `match ServerMsg` arm, and the smoke-test constructors
  (`cli_smoke.rs`, `fleetyd_smoke.rs`). A new `Welcome`/reply field breaks
  compilation across all of them.
- **The three binaries** — `fleety` / `fleety-server` / `fleetyd` share verbs
  (`update`, `config`, service lifecycle) and dependency provisioning; a change
  to one shared behavior usually needs the others.
- **Install scripts** — `scripts/install.sh`, `install.ps1`, `install-server.sh`
  (target maps, asset names, closing guidance) move together.
- **Docs + registry for any `FLEETY_*` var or command** — `docs/env.md`, the
  config registry (`fleety-tools/src/config.rs`), `README.md`, and the relevant
  `openspec/specs/*` — a new knob or command is not done until these agree.

If a change touches one member of a family, state in the change why the others
are updated or exempt — don't silently leave them inconsistent.

# Fleety — project notes

Workspace-wide engineering rules live in the code (`#![warn(clippy::unwrap_used,
clippy::expect_used)]`, never-crash errors-as-messages, `agent-core` depends on
no Fleety crate). This file holds things that aren't derivable from the code.

## Spectra pitfalls (verified 2026-07-11, spectra 2.3.1)

- **Never delete `.spectra/touched/<change>.json` before `spectra archive`.** The
  CLI reads it at archive time to inject `@trace` blocks into the main specs, and
  it never cleans the file up itself — delete it only after a successful archive.
  The generated skill files (`.claude/.agents/.opencode` spectra-archive copies)
  used to get this order backwards; they are locally patched, but `spectra update`
  (even without `--force`) regenerates them and silently reverts local patches —
  re-check the step order after any spectra update. Not yet reported upstream:
  `spectra feedback` only prints the message (transmits nothing) and points at
  github.com/kaochenlong/Spectra/issues, which is not publicly accessible as of
  2026-07-11 (nor is the spectra-app/spectra URL from the config comments) —
  re-report if the repo ever becomes reachable.
- **`spectra archive` run inside a git worktree** false-positives with "Change 'X'
  exists in both the main repository and a worktree" — both printed paths are the
  same directory, compared with mismatched `\` vs `/` separators. Workaround: run
  the archive from the main checkout's cwd; it then operates on the worktree copy
  correctly (verified: main checkout stays untouched).

## Follow-ups

### [2026-07-23] — Reconnect control needs an explicit version boundary

- **Where:** `crates/fleety-daemon/src/main.rs`
- **What:** The durable reconnect journal replaces the former request/ACK files,
  but the ready record does not advertise which control protocol the running
  Daemon understands. A mixed-version CLI and `fleetyd` can therefore disagree
  about the files to read without a negotiated error. Ready publication also
  lacks file/directory fsync and process-start identity, so a crash or PID reuse
  can leave ownership unreadable or falsely live.
- **Suggestion:** Version the ready/control contract and define migration or
  explicit incompatibility behavior, durable publication, and PID-start proof
  before the next release that can mix binary versions.
- **Status:** pending

### [2026-07-23] — Reconnect requests lack lifecycle operations

- **Where:** `crates/fleety-daemon/src/main.rs`, `crates/fleety-cli`
- **What:** A timed-out durable nonce remains authoritative until its result is
  observed. Users cannot inspect it by nonce, cancel it, deliberately supersede
  it, or rely on a documented retention/garbage-collection policy. Reconnect
  and connection mutation locks also refuse unsafe elapsed-time reclamation, so
  a crashed lock owner needs an explicit, owner-aware recovery command.
- **Suggestion:** Design explicit status, cancel/supersede, retention, and owner
  drift observability together with safe stale-lock inspection/cleanup so
  recovery does not depend on retrying commands in a particular order.
- **Status:** pending

### [2026-07-23] — Automatic mDNS can still establish an untrusted control session

- **Where:** `crates/fleety-tools/src/connection.rs`,
  `crates/fleety-daemon/src/main.rs`
- **What:** Automatic mDNS no longer receives stored profile credentials or
  mutates credentialed endpoints, but a fresh/uncredentialed `fleetyd` can still
  accept a `Welcome` and `RunTool` frames from the selected advertiser. An
  explicit `FLEETY_TOKEN` or `FLEETY_PAIRING_CODE` without an explicit endpoint
  can also follow automatic discovery. Pairing over an unauthenticated WebSocket
  can exchange credentials but does not by itself prove endpoint identity
  against an active relay. These are outside task 5.53's stored-token boundary,
  but TXT metadata still cannot authenticate an operational control session.
- **Suggestion:** Define whether automatic mDNS is picker-only or may establish
  an unprivileged session. If picker-only, require an explicit URL/profile before
  sending any token, pairing code, or accepting control frames; cover rogue
  `Welcome`/`RunTool` and unsolicited token persistence in daemon smoke tests.
- **Status:** pending

## Releasing

**Before cutting a release, update the bundled Insyra — the library and the two
vendored skills.** The `fleety-insyra` sidecar ([`sidecars/fleety-insyra`](sidecars/fleety-insyra))
embeds `github.com/HazelnutParadise/insyra`, and `fleety-server` embeds two
upstream skills, each embedded as a **whole directory** (via `include_dir` — the
file set isn't hardcoded; new upstream files flow through, only `SKILL.md` is
guaranteed): the Fleety-adapted `fleety-use-insyra-dsl`
([`builtin-skills/fleety-use-insyra-dsl/`](crates/fleety-server/builtin-skills/fleety-use-insyra-dsl),
upstream `skills/use-insyra-cli`; its `SKILL.md` is the pristine upstream copy and
a Fleety-authored `HEADER.md` is folded onto it at seed) and the verbatim `insyra`
([`builtin-skills/insyra/`](crates/fleety-server/builtin-skills/insyra),
upstream `skills/insyra`). We want releases to ship the latest of all three, kept
in lockstep. CI's release workflow does this automatically: `go get -u …insyra@latest`
+ `go get -u ./...` + `go mod tidy` (bumps Insyra **and** its sub-packages; CI's
`setup-go: stable` keeps the Go toolchain current, and `go mod tidy` raises the
go.mod `go` directive when a dep needs it), then mirrors **each skill's whole
upstream directory from the resolved release tag — never `main`** (preserving our
`HEADER.md`), then build + smoke test. If you ever cut a release outside that
workflow, in `sidecars/fleety-insyra/` run `go get -u
github.com/HazelnutParadise/insyra@latest && go get -u ./... && go mod tidy`,
mirror each skill dir from `raw.githubusercontent.com/HazelnutParadise/insyra/<release-tag>/skills/{use-insyra-cli,insyra}/`
(keep `HEADER.md`), and commit the updated `go.mod`/`go.sum`/skills. A breaking
Insyra change will surface as a sidecar build/test failure — fix the sidecar,
don't pin around it silently.
