# fleety-textarea

A multi-line text-input widget for [ratatui](https://ratatui.rs): selection,
undo/redo, mouse, inline elements, soft wrap, and a readline-style key map.

**This crate is vendored third-party source, not Fleety-authored code.** Treat
`src/` as read-only. See "Re-syncing from upstream" before changing anything in
it.

## Provenance

| | |
|---|---|
| Upstream project | [`xai-org/grok-build`](https://github.com/xai-org/grok-build) |
| Upstream path | `crates/codegen/xai-ratatui-textarea` |
| Upstream package | `xai-ratatui-textarea` 0.1.0 |
| Snapshot | `SOURCE_REV` `d02693a856a54f1030695b36b91d276e96b30b23` |
| Vendored on | 2026-07-26 |
| License | Apache-2.0, Copyright 2023-2026 SpaceXAI (see [`LICENSE`](LICENSE)) |

## Changes from upstream

Required by Apache-2.0 §4(b).

- **`src/` is byte-identical to upstream.** No source modifications.
- `Cargo.toml` is Fleety-authored: the package is renamed to `fleety-textarea`,
  workspace dependency inheritance replaces upstream's, and the upstream
  `examples/`, `[lints]`, and unused dev-dependencies are dropped.

Upstream publishes no `NOTICE` file, so Apache-2.0 §4(d) does not apply.

## Why edition 2024

Upstream uses let-chains (`if let Some(x) = e && cond`) at 9 sites, which the
2021 edition rejects. This crate therefore declares `edition = "2024"` and
`rust-version = "1.85"` rather than inheriting the workspace's 2021 / 1.80.

Editions are per-crate, so no other crate moved to 2024. The Rust *floor* is
shared, though: `fleety-cli` depends on this crate, so the workspace
`rust-version` was raised from 1.80 to 1.85 to match.

The alternative — rewriting the 9 let-chain sites for edition 2021 — would have
kept the 1.80 floor but made `src/` diverge from upstream, so every future
re-sync would have to re-apply those edits by hand. That trade was declined.

## Re-syncing from upstream

Because `src/` is unmodified, a re-sync is a directory replacement:

```bash
git clone --depth 1 https://github.com/xai-org/grok-build.git /tmp/grok-build
rm -rf crates/fleety-textarea/src
cp -R /tmp/grok-build/crates/codegen/xai-ratatui-textarea/src crates/fleety-textarea/src
cp /tmp/grok-build/LICENSE crates/fleety-textarea/LICENSE
cargo test -p fleety-textarea
```

Then update the snapshot row in the table above with the new `SOURCE_REV`, and
re-check the dependency versions in `Cargo.toml` against upstream's workspace
`Cargo.toml` — upstream inherits them, this crate pins them literally, so they
drift silently.

Upstream does not accept external pull requests, so fixes made here cannot be
sent back; keep local changes to `src/` at zero for as long as possible.
