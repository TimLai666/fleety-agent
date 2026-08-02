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
  used to get this order backwards. They now carry a runnable
  `# SPECTRA_SAFE_ARCHIVE_START/END` block that
  `scripts/check-spectra-archive-instructions.sh` executes in CI against a fake
  `spectra`, proving a failed archive keeps its tracking file and a successful
  one removes it. `spectra update` still regenerates these files and will drop
  the block, but now CI says so instead of the loss being silent — restore the
  block rather than only re-reading the prose. Not yet reported upstream:
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

### [2026-08-02] — A `localhost` endpoint costs ~2 s on Windows, and the Server binds v4 only

- **Where:** `crates/fleety-server/src/main.rs` (`FLEETY_ADDR`, default
  `0.0.0.0:8787`), reached through any profile whose URL spells the host
  `localhost`
- **What:** on a dual-stack Windows host `localhost` resolves to `::1` before
  `127.0.0.1`, and a connect to `[::1]:<port>` with nothing bound there takes
  **~2.04 s** to fall through to the v4 address (measured against one
  127.0.0.1-bound listener: `localhost` 2.0365 s, `127.0.0.1` 1.7 ms). The
  Server's default bind is v4 only, so a Windows user who types
  `ws://localhost:8787` pays that on every connect. Worse, it exceeds every
  per-candidate budget in the sweep — `open_budget_within` halves the caller's
  share before the transport even starts — so such an endpoint does not merely
  feel slow, it is *never* reachable as a candidate. `DEFAULT_URL` sidesteps
  this by being the IPv4 literal; a hand-typed or pasted `localhost` does not.
- **Suggestion:** listen dual-stack, or normalise `localhost` to `127.0.0.1` in
  `fleety init` and say why. Note the tests can no longer warn about this: since
  2026-08-02 `start_roaming_budget_server` binds a best-effort companion
  listener on `[::1]:<same port>`, so the roaming tests answer on whichever
  family `localhost` prefers and no longer surface the v4-only gap.
- **Status:** resolved by the `localhost-dual-stack-reachability` change
  (2026-08-02): the server grows a best-effort same-port IPv6 companion for the
  two IPv4 default forms (`bind_with_companion` in `fleety-server/src/main.rs`),
  and the transport dials a host spelled exactly `localhost` as `127.0.0.1`
  (`dial_target` in `fleety-tools/src/transport.rs`) so old v4-only servers stay
  fast too. A v6-only server is reached by spelling `ws://[::1]:8787`.

### [2026-08-01] — CI's clippy gate cannot be run locally on Windows

- **Where:** `crates/fleety-tools/src/connection.rs:306` (`dir`),
  `crates/fleety-tools/src/service.rs:934` (`created`)
- **What:** Both bindings are read only inside a `#[cfg(unix)]` block, so on
  Windows they are genuinely unused and `cargo clippy -- -D warnings` errors.
  CI runs on `ubuntu-latest`, where both are used, so the gate is green there —
  the failure is invisible to CI and hits only Windows developers, who then
  cannot run `cargo clippy --workspace --all-targets -- -D warnings` (CI's exact
  command) before pushing. Because dependents build `fleety-tools` first, it
  also breaks a clippy run scoped to `fleety-server` or the CLI. Present on
  `main` at `6188624`, independent of any local change.
- **Suggestion:** mark both with `#[cfg_attr(not(unix), allow(unused))]` (or
  `cfg`-gate the binding itself) so the lint gate is runnable on every platform
  the project is developed on, not just CI's.
- **Status:** pending

### [2026-07-28] — Reconnect journal: a crash between the drift receipt and the reap bricks startup

- **Where:** `crates/fleety-daemon/src/main.rs` — `reject_frozen_authenticated_reconnect`
  writes the receipt and then reaps the journal as two steps; on restart
  `recover_reconnect_for_instance_at` builds its own differently-worded failure
  and `preserve_reconnect_receipt_at` rejects it as conflicting.
- **What:** a crash between those two steps leaves a failure receipt and an
  unproven accepted journal for the same nonce. Every subsequent start then
  fails `ControlGuard::claim` and exits non-zero — deterministically, with no
  way out but deleting files by hand. The existing test passes only because it
  seeds a receipt carrying the identical message the retry then constructs, so
  the cross-path mismatch is never exercised. `design.md` already states the
  intended rule ("cleanup retry treats that receipt as authoritative and never
  recreates a terminal-only journal"); it is not implemented on this path.
- **Suggestion:** treat any existing terminal receipt for that nonce as
  authoritative during recovery — reap the journal and return — instead of
  constructing a second failure to publish.
- **Status:** resolved by `d6dbefe` (2026-08-02) and the archived
  `reconnect-control-resilience` change. The receipt-authority, torn-append,
  writable-rollback, bounded-retry, quarantine, and settlement paths now have
  regression coverage. Windows-native runtime behavior remains untested on the
  current macOS host.

### [2026-07-28] — An older binary silently drops a profile's secure-channel state

- **Where:** `crates/fleety-tools/src/connection.rs` (`Profile`), `connections.toml`
- **What:** `endpoints`, `configured_url`, and `secure` are all
  `#[serde(default, skip_serializing_if = …)]` and `Connections` carries no
  schema version. A pre-5.65 `fleety` or `fleetyd` reads the file, ignores those
  fields, and drops them on the next write — any write, including `connection
  use` or a TOFU pin. The profile then forgets that its Server proved it can
  open the encrypted channel and accepts the cleartext path again, with no
  warning. No attacker is needed: a rollback, a stale binary on `PATH`, or a CLI
  and daemon on different versions is enough.
- **Suggestion:** version `Connections` so a newer binary can tell that
  something downgraded the file, and decide explicitly whether to refuse the
  write or re-derive the latch. Note that a version field alone does not stop
  the loss — an old binary drops that too — so the value is detection, not
  prevention.
- **Status:** partially resolved by `d6dbefe` (2026-08-02). Current writers emit
  a store marker and profile generation evidence, and refuse incompatible state
  before credential use or rewrite. A legacy binary can still discard fields it
  does not understand before the newer binary detects the loss, so prevention
  still requires upgrading all Fleety binaries. Residual risk remains pending.

### [2026-07-28] — Reconnect budget is tight for cross-network roaming

- **Where:** `crates/fleety-daemon/src/main.rs` (`RECONNECT_SWEEP_BUDGET`,
  `RECONNECT_HANDSHAKE_WAIT`, `RECONNECT_ACK_WAIT`)
- **What:** an owner-requested reconnect must settle inside the caller's 5 s
  wait, so the whole candidate sweep gets 4.5 s: 3 s for the configured endpoint
  and the remainder split across alternatives. Roaming's main scenario is
  leaving a LAN for an overlay, where RTT is far higher than the LAN the numbers
  were tuned on, so a legitimate alternative can time out. The ordinary
  (non-reconnect) path uses `CONNECT_ENDPOINT_WAIT` (15 s) and is unaffected.
- **Suggestion:** widen the sweep and the caller's wait together, or let the
  caller carry its own budget in the request so the two cannot drift.
- **Status:** resolved by `d6dbefe` (2026-08-02) and the archived
  `reconnect-control-resilience` change. The caller wait and candidate sweep now
  share one documented budget, with deterministic slow and silent candidate
  coverage. Real high-latency overlay networks remain untested here.

### [2026-07-27] — `server_smoke` command tests fail on a spawn deadline, and say the wrong thing

- **Where:** `crates/fleety-server/tests/server_smoke.rs`
- **What:** `run_command_in` gives a spawned `fleety-server` three seconds to
  exit, then panics with "started the server instead of exiting". The message
  describes a behaviour that did not happen — it is a timeout. On a loaded
  machine the ~142 MB debug binary does not reliably start and exit inside that
  window, and *which* test in the family fails rotates between runs. Verified
  unrelated to any source change by reproducing it with the working tree
  stashed, and verified the binary itself is correct by running
  `fleety-server config list unexpected` by hand (immediate, correct usage
  error). `cargo clean -p fleety-server` made it pass once, then it returned.
- **Suggestion:** Raise the deadline, or scale it from an env var, and reword the
  panic to say the command did not exit within N seconds. Worth checking whether
  the worktree and the main checkout fighting over one `target/` contributes.
- **Status:** pending. The command remains a flaky spawn-deadline test candidate;
  the current local workspace run passing once does not remove the recorded
  deadline risk.

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
- **Status:** resolved by `d6dbefe` (2026-08-02) and the archived
  `reconnect-control-resilience` change. The control contract now carries an
  explicit version, process-start identity, owner generation, and durable
  publication checks, with malformed or conflicting records failing closed.

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
- **Status:** resolved by `d6dbefe` (2026-08-02) and the archived
  `reconnect-control-resilience` change. Nonce status, cancel, supersede,
  retention, control inspection, and owner-safe stale recovery are exposed and
  covered by CLI, service, and daemon tests.

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
- **Status:** resolved for automatic discovery on current `main`. The resolver
  treats mDNS as display-only, and `fleetyd_smoke` verifies that a discovered
  advertiser cannot receive credentials, open a control session, or persist a
  rogue token. Explicit endpoint pairing remains governed by its separate
  authenticated-pairing path.

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

## Vendored Rust source

Three crates are vendored from grok-build, all Apache-2.0, all pinned to one
upstream snapshot (`SOURCE_REV` `d02693a`), all with `src/` byte-identical and a
README carrying provenance plus the re-sync procedure:

| Crate | Upstream | Used by |
|---|---|---|
| `crates/fleety-textarea` | `xai-ratatui-textarea` | the Chat composer |
| `crates/fleety-markdown` | `xai-grok-markdown` | assistant reply rendering |
| `crates/fleety-markdown-core` | `xai-grok-markdown-core` | `fleety-markdown` only |

The rules below were written for `fleety-textarea` and apply to all three.

**Mermaid already works and needs nothing more vendored.**
`fleety-markdown/src/mermaid.rs` is a self-contained Unicode line-art renderer
that the parser calls for every closed ` ```mermaid ` fence, so diagrams render
on any terminal with no graphics protocol and no subprocess. Upstream's separate
`xai-grok-mermaid` crate is a *different, higher-fidelity* path — SVG via
`mermaid-to-svg` + `dagre_rust` + `graphlib_rust` + `ordered_hashmap` +
`xai-tty-utils` (~29k lines across six crates), rasterised with resvg/tiny-skia
and shown through the Kitty or iTerm2 graphics protocol, blank everywhere else.
Do not vendor it under the impression that Fleety lacks mermaid support; it is
a fidelity upgrade for two terminals, and it is a separate decision.

`crates/fleety-markdown` has two extra wrinkles. Its palette is **not**
upstream's: `MarkdownStyle::default()` is entirely unstyled because upstream
fills it from a theme layer it does not publish, so the chat colours — and the
decision to keep a bare newline as a line break instead of collapsing it per
CommonMark — live in `crates/fleety-cli/src/markdown.rs`. Change appearance
there, never in the vendored `src/`. And its `Cargo.toml` depends on the core
crate under upstream's **extern name**
(`xai-grok-markdown-core = { package = "fleety-markdown-core", … }`) precisely so
`src/` needs no edit.

`crates/fleety-textarea` is a byte-identical copy of `xai-ratatui-textarea` from
[`xai-org/grok-build`](https://github.com/xai-org/grok-build) (Apache-2.0),
pinned to one upstream snapshot. **Unlike Insyra, it is deliberately not kept at
latest** — do not add it to the release auto-update flow. Upstream accepts no
external patches, so anything fixed here can never be sent back; keeping `src/`
unmodified is what makes a re-sync a directory replacement instead of a patch
replay. Provenance, the recorded drift, and the re-sync procedure live in
[`crates/fleety-textarea/README.md`](crates/fleety-textarea/README.md).

Two things about it are not local to that crate:

- **It is edition 2024, and it is why the workspace MSRV is 1.85.** `fleety-cli`
  depends on it, so its floor is the whole build's floor; the workspace
  `rust-version` was raised from 1.80 to match. Nothing in CI checks that
  number (`dtolnay/rust-toolchain@stable`, no `rust-toolchain.toml`), so
  lowering it again is not something a gate will catch. The `clap = "=4.5.4"` /
  `clap_complete = "=4.5.2"` pins were taken to defend the old 1.80 floor and
  are now only inertia — safe to revisit whenever someone re-tests a newer Clap.
  Raising the MSRV also un-suppressed clippy's `unnecessary_map_or` in three
  places (`fleety-tools/src/transport.rs`, `fleety-cli/src/auth.rs`,
  `fleety-cli/src/main.rs`); expect more MSRV-gated lints to appear the next
  time this number moves.
- **Its `[lints.clippy]` allow-list** exists only because `src/` is unpatched.
  Re-check it after every re-sync; it should shrink, never grow silently.

It is wired into the Chat composer only. The Settings and Provider panels still
use the single-line `LineEditor` in `crates/fleety-cli/src/input.rs`, whose
multi-line half was deleted when Chat stopped using it — reach for the composer,
not for re-growing `LineEditor`, if one of those panels ever needs wrapping,
undo, or selection.

**Chat must own the terminal — do not run it backgrounded.** `Viewport::Inline`
asks the terminal where the cursor is and reads the answer from stdin; a
backgrounded `fleety chat &` cannot read the tty and spins there forever,
before any frame is drawn. The old alternate-screen path never noticed because
`Viewport::Fullscreen` asks nothing. This bit once during manual verification
and looks exactly like a hung handshake, so check the job control first.

Chat runs in an **inline viewport** (`fleety-inline`), not the alternate screen:
the conversation is written into the terminal's scrollback and Fleety never
redraws it. Settings and Provider keep their own `ratatui::init()` full-screen
terminals — the three lifecycles were always separate. Routes that need the
screen (Conversations, modals, the palette) grow the viewport to full height and
use `workspace::render`; Chat uses `workspace::render_inline`, whose chrome is
one header line instead of seven rows.

The whole path is testable without a terminal. `main.rs`'s `sync_terminal` is
the one seam where Fleety state becomes terminal output, and it takes nothing
from the event loop or the transport, so `src/test_terminal.rs` runs it over a
`Backend + Write` capture backend and asserts on the bytes actually emitted —
including that an unclosed fence stays in the viewport until it closes. A
physical TTY is only needed to judge how it *looks*. Keep that seam a function:
inline it back into the draw loop and this all becomes untestable again.

The startup banner goes out through `App::announce`, which queues it ahead of
the conversation in the same outbox `take_emissions` drains. That is deliberate:
it lands in the scrollback once and is replayed by a resize like everything
else. Anything else that never changes belongs there too, not in the viewport.

Two invariants hold the model together. `App::take_emissions` is the only way
content leaves Fleety, and everything it returns is appended to `App::history`,
which is what a resize replays — bypass it and a resize loses that content. And
a streaming reply is only handed over as far as `markdown::settled_prefix_len`,
because an open fence still renders differently once it closes.

Fleety enables no mouse reporting: `WorkspaceInput` carries keyboard events
only, and selection plus scrolling are the terminal's own. This was deliberate —
see `openspec/changes/inline-chat-viewport`, which removes the `tui-mouse-input`
capability outright. Codex takes the same position and never sends
`EnableMouseCapture`; grok takes the opposite one, but pairs it with a full
transcript-selection implementation Fleety does not have.

It is wired into the Chat composer only. The Settings and Provider panels still
use the single-line `LineEditor` in `crates/fleety-cli/src/input.rs`, whose
multi-line half was deleted when Chat stopped using it — reach for the composer,
not for re-growing `LineEditor`, if one of those panels ever needs wrapping,
undo, or selection.

Mouse reporting is enabled for the duration of the Chat workspace loop
(`EnableMouseCapture` in `crates/fleety-cli/src/main.rs`, released on both loop
exits and by a `Once`-installed panic hook, because `ratatui::init`'s own hook
does not know about it). Consequences worth remembering:

- **Only Chat reads mouse events.** `WorkspaceInput::recv` filters them out, so
  every other route is unaware; `recv_event` is the Chat-only entry point. A new
  route that wants clicks opts in by switching to `recv_event`, not by changing
  `recv`.
- **Hit-testing reads the geometry the last frame recorded** (`App`'s
  `conversation_area` / `composer_area` `Cell`s). Anything that draws Chat
  through a new path must record those too or clicks land nowhere.
- **Transcript selection is deliberately the terminal's job** (Shift+drag), not
  Fleety's. Grok's own selection code is not portable here — it is built on a
  block/table scrollback model that Fleety's flat `Paragraph` does not produce.
  Document Shift+drag rather than re-implementing it.
