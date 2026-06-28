## Context

The CLI already advertises where the user is: `Hello { device_id }` identifies the machine, and every `UserMessage` carries `OriginContext { hostname, os, cwd }` (the cwd captured by the CLI's `origin()`). The server parses both — it registers `device_id` in the routing hub and logs `origin` — but tool execution ignores cwd: filesystem/command/git tools are built once against the server's `FLEETY_WORKSPACE` (or the server process's cwd). Device-targeted tools already route through the existing bridge (`device_exec` to that device's fleetyd). So the missing piece is purely server-side: derive a per-conversation root + executing device from the origin and point that conversation's tools there, instead of at the server host.

## Goals / Non-Goals

**Goals:**
- A conversation's filesystem/command/git tools default to the originating CLI's `origin.cwd`, executing on the originating device.
- The resolved root + device persist with the conversation, so resume and later turns are stable.
- `cwd` is validated as untrusted input; the existing `FLEETY_FS_SCOPE` + sensitive-path guard still apply; `full_access` is preserved.
- Absent/unusable origin falls back to today's server-side workspace behavior (no regression).

**Non-Goals:**
- No protocol change (cwd/device already on the wire).
- No new device transport: reuse the existing registry/bridge; non-executor origin falls back.
- No change to tool implementations or the agent loop, only their root + target device.
- No new sandbox model beyond the existing scope/guard.

## Decisions

### Per-conversation workspace binding resolved once, from the first message

On a conversation's first `UserMessage`, resolve a `WorkspaceBinding { root, device }` from `origin.cwd` + the connection's `device_id`, persist it with the conversation, and reuse it for every later turn and on resume. Resolving once (not per message) avoids drift if a later message carries a different cwd, and makes resume deterministic. A later message's differing cwd is ignored for routing (logged), keeping a conversation anchored to one working tree.

**Alternative:** re-resolve per message — rejected (a conversation could hop directories mid-thread; resume would be ambiguous).

### Routing: originating device first, server-host fast path, else fallback

The executing target is the device that opened the connection (`device_id` from `Hello`):
- If that device is the **server host itself** (local dev / CLI on the same box as the server), bind tools directly to `root` on the server (no bridge hop).
- If that device is a **registered executor** (its fleetyd is connected via the bridge), bind the conversation's filesystem/command/git tools to run on that device rooted at `root` (reusing the existing device-exec path).
- If the origin device is **not reachable as an executor** (no fleetyd there), fall back to the server-side workspace (below) and log that on-origin execution is unavailable.

**Alternative:** always execute on the server host using the path string — rejected (the path is on the user's machine, not the server; only correct when they are the same host).

### cwd is untrusted: validate, normalize, keep the guard

`origin.cwd` comes from a client and must not be trusted blindly. Resolution: reject empty/relative/non-absolute values; normalize; never use it to escape the active `FLEETY_FS_SCOPE` posture; the sensitive-path guard continues to refuse critical paths regardless of root. `full_access` is preserved — the agent may still operate outside `root` exactly as today — but the *default* root the tools present to the model becomes `root`.

**Alternative:** trust cwd verbatim — rejected (path injection / surprising roots).

### Fallback preserves today's behavior

When there is no usable binding (older CLI with no `origin`, blank/invalid cwd, or a non-executor origin device), the server uses the existing default: `FLEETY_WORKSPACE` if set, else the server process cwd, executing on the server host. This is the current code path, so older clients and server-local tasks are unaffected. Precedence: **origin.cwd (on origin device) then FLEETY_WORKSPACE then server cwd**.

### Persistence + resume

The conversation's `WorkspaceBinding` is stored alongside its existing conversation record (in storage). Resume loads it and rebinds tools to the same root/device, so reconnecting to a long-running coding conversation keeps working in the same tree. If the origin device is offline at resume time, fall back (and log) rather than failing the resume.

## Implementation Contract

**Behavior:** A CLI launched in directory D on device X, talking to the server, produces a conversation whose filesystem/command/git tools read/write/run in D on X. Opening another CLI in directory E on device Y yields an independent conversation rooted at E on Y. With no origin (old CLI) or an unreachable origin device, tools behave as today (server workspace). Resume rebinds to the stored root/device. Invalid/at-risk paths are refused by the existing guard; nothing panics.

**Interfaces / data shapes:**
- `WorkspaceBinding { root: PathBuf, device: Option<DeviceId> }` (device `None` = server host).
- A pure resolver `resolve_binding(origin, conn_device, server_host_device, fallback_root) -> WorkspaceBinding` — given the origin context, the connection's device, who the server host is, and the fallback root, returns the binding. Pure and unit-testable (no I/O): covers absolute-cwd accept, blank/relative reject to fallback, origin-is-server-host to local, origin-is-other-device to remote, no-origin to fallback.
- Conn threads the binding into the per-conversation tool registry construction (filesystem/command/git get `root` and, when `device` is set, the device-exec routing already used by `device_exec`).
- Storage gains a conversation field for the binding (root + optional device), written on first message, read on resume.

**Failure modes:** blank/relative/non-absolute cwd to fallback + log. Origin device not a connected executor to fallback + log. cwd points at a guarded/sensitive location: tool calls refused by the existing guard (binding still recorded). Storage write of the binding fails: log, continue with the in-memory binding (don't fail the turn). Resume with offline origin device: fallback + log. Never panic; never block the turn.

**Acceptance criteria:**
- Pure unit tests for `resolve_binding`: absolute cwd on origin=server-host gives local root; absolute cwd on origin=other device gives remote binding with that device; blank/relative/no-origin gives fallback root, device None; precedence (origin beats FLEETY_WORKSPACE beats server cwd) asserted via the fallback_root the caller passes.
- Conn-level test (with an injectable/registered device) that a conversation's tool root reflects the resolved binding, and that a second conversation with a different origin is independent.
- Resume test: a stored binding is reloaded and reused.
- Backward-compat test: a `UserMessage` with no `origin` yields the fallback binding (today's behavior).
- Content review: docs/env.md documents the precedence; security note on untrusted cwd + retained guard.
- agent-core unaffected (`cargo tree -p agent-core` has no `fleety-*`); fmt + clippy --workspace -D warnings + test --workspace green.

**Scope boundaries:**
- In: server-side per-conversation workspace/device resolution + persistence + resume rebinding, untrusted-cwd validation, fallback, docs, pure resolver + conn/storage tests.
- Out: protocol changes, tool implementation changes, new device transport/enrollment, sandbox-model changes, CLI changes (it already sends origin).

## Risks / Trade-offs

- [cwd from client is untrusted] to validate/normalize; keep FLEETY_FS_SCOPE + sensitive-path guard; full_access unchanged but default root is the validated cwd.
- [origin device has no fleetyd executor] to fall back to server workspace + log; don't invent a transport.
- [a conversation's later message carries a different cwd] to ignore for routing (resolve once), log; conversation stays anchored to one tree.
- [resume when origin device offline] to fall back + log, don't fail resume.
- [storage schema addition] additive optional field; old conversations without it use fallback.
- [multi-conversation isolation] binding is per-conversation, not global; two CLIs on different dirs/devices stay independent.
