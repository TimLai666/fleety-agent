## Why

A user opens the CLI inside a project directory on their laptop and expects Fleety to act as a coding agent *for that project on that machine* — read, edit, run, and git that working tree. Today it can't: the CLI already sends `OriginContext { hostname, os, cwd }` on every `UserMessage` and a `device_id` on `Hello`, but the server only writes the origin to a log. The agent's file/command/git tools operate on the *server's* `FLEETY_WORKSPACE` (or the server's own cwd), not on the directory and device the user launched from. The wiring is half-built; this change connects the last segment so each conversation works in the originating device's directory.

## What Changes

- The server resolves a **per-conversation workspace root** from the first `UserMessage`'s `origin.cwd`, and binds that conversation's filesystem / command / git tools to run **on the originating device** (the one identified at `Hello`) rooted at that cwd — so opening the CLI in `~/proj` on a laptop edits `~/proj` on that laptop.
- The resolved root is **recorded with the conversation** so `resume` and follow-up turns reuse it (not re-derived per message, which could drift if a later message carries a different cwd).
- `cwd` is treated as **untrusted client input**: normalized/validated, and still subject to the existing `FLEETY_FS_SCOPE` posture and the sensitive-path guard. `full_access` is preserved (the agent may still reach outside the root); only the *default root* changes to `origin.cwd`.
- **Backward compatible**: when no usable `origin.cwd` is present (older CLI, missing/blank cwd, or the originating device has no executor), the server falls back to today's behavior (`FLEETY_WORKSPACE` / server cwd on the server host).

## Non-Goals

- No protocol change — `OriginContext.cwd` and `device_id` already exist on the wire.
- Not changing the agent loop, tool implementations, or the device bridge mechanism itself; only *where* (which device + root) a conversation's tools are pointed.
- Not adding a new device-enrollment path: routing to the originating device reuses the existing device registry / bridge; if that device isn't a registered executor, this falls back rather than inventing a new transport.
- Not sandboxing changes beyond reusing the existing `FLEETY_FS_SCOPE` + sensitive-path guard.

## Capabilities

### New Capabilities

- `session-workspace`: each conversation derives its working root and executing device from the originating CLI's `origin.cwd` + `device_id`, binding that conversation's filesystem/command/git tools to run on that device rooted there; untrusted-cwd validation, conversation-persistent root, and a safe fallback to the server-side workspace when origin is absent.

### Modified Capabilities

(none — existing capabilities' specs are unchanged; this adds the per-session resolution layer above them.)

## Impact

- Affected specs: new `session-workspace`.
- Affected code:
  - Modified: crates/fleety-server/src/conn.rs (resolve + persist per-conversation workspace root and executing device from the first message's origin; thread it into tool construction), crates/fleety-server/src/main.rs (workspace resolution becomes a fallback default rather than the sole source), crates/fleety-server/src/storage.rs (persist the conversation's resolved workspace root + device for resume), docs/env.md (document precedence: origin.cwd then FLEETY_WORKSPACE then server cwd)
  - New: none required (logic lands in existing server modules; a small helper module may be added under crates/fleety-server/src if conn.rs grows too large)
  - Removed: none
