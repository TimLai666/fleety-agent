# Fleety

Fleety is a cross-device, full-access agent and device-fleet assistant. Summon the
agent from any device; it knows where the message came from, what each device can
do, and routes each task to the device best able to finish it.

> **Status: M0 — workspace skeleton.** Compiles and runs; no agent service yet.
> Design lives in [`docs/spec-v0.md`](docs/spec-v0.md); the agent system prompt is
> in [`prompts/`](prompts/).

## Workspace

| Crate | Role |
|---|---|
| [`crates/agent-core`](crates/agent-core) | Generic agent core (errors, never-crash primitives, observability). The future standalone framework — depends on no Fleety crate. |
| [`crates/fleety-protocol`](crates/fleety-protocol) | Wire types shared by CLI / daemon / server. |
| [`crates/fleety-server`](crates/fleety-server) | Fleety Agent server (`fleety-server`). |
| [`crates/fleety-daemon`](crates/fleety-daemon) | Device background service (`fleetyd`). |
| [`crates/fleety-cli`](crates/fleety-cli) | CLI / TUI (`fleety`). |

Dependency rule: everything may depend on `agent-core`; `agent-core` depends on
nothing Fleety-specific, so it can later be extracted to its own repo and mounted
back as a git submodule.

## Build

```sh
cargo build --workspace
cargo test --workspace
cargo run -p fleety-server
```

## Design docs

- [`docs/spec-v0.md`](docs/spec-v0.md) — v0 scope, architecture, milestones M0–M11
- [`docs/tools.md`](docs/tools.md) — agent tool surface
- [`prompts/`](prompts/) — protocol / memory / policy / rules (the agent system prompt)

## License

Apache-2.0 (see [`LICENSE`](LICENSE)).
