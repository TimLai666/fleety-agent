# fleety-inline

A ratatui `Terminal` that draws into a small viewport at the bottom of the
screen and pushes finished content into the terminal's own scrollback, instead
of taking the whole screen with an alternate-screen buffer.

**This crate is vendored third-party source, not Fleety-authored code.** Treat
`src/` as read-only. See "Re-syncing from upstream" before changing anything in
it.

## Provenance

| | |
|---|---|
| Upstream project | [`xai-org/grok-build`](https://github.com/xai-org/grok-build) |
| Upstream path | `crates/codegen/xai-ratatui-inline` |
| Upstream package | `xai-ratatui-inline` 0.1.0 |
| Snapshot | `SOURCE_REV` `d02693a856a54f1030695b36b91d276e96b30b23` |
| Vendored on | 2026-07-26 |
| License | Apache-2.0, Copyright 2023-2026 SpaceXAI (see [`LICENSE`](LICENSE)) |

`src/terminal.rs` is itself **derived from ratatui's own `Terminal`** (MIT,
Copyright 2016-2022 Florian Dehau, 2023-2025 The Ratatui Developers) — its doc
comments are ratatui's verbatim. Both attributions travel with this directory.

## Changes from upstream

Required by Apache-2.0 §4(b).

- **`src/` is byte-identical to upstream.** No source modifications.
- `Cargo.toml` is Fleety-authored: the package is renamed to `fleety-inline`,
  workspace dependency inheritance is replaced with literal versions, and
  upstream's `examples/`, `benches/`, `tests/` and their dev-dependencies are
  dropped. The `scrolling-regions` feature is kept exactly as upstream declares
  it, because `src/` gates code on it.

Upstream publishes no `NOTICE` file, so Apache-2.0 §4(d) does not apply.

## What it adds over stock ratatui

- `emit_to_scrollback` — write finished content, as an ANSI string, above the
  viewport so it becomes real terminal history: scrollable with the terminal's
  own scrollbar, selectable with the terminal's own mouse, still on screen after
  the process exits.
- `set_frame_links` / `LinkSpan` — a per-cell OSC 8 hyperlink layer folded into
  the frame diff, so links are emitted and cleared by the same machinery that
  draws cells.
- `resize_purge_rerender` — on resize, reset the terminal and re-emit the whole
  history, rather than letting the terminal's own reflow corrupt it.
- `with_synchronized_output` — wrap a frame in begin/end synchronized update so
  it does not tear.
- `split_into_line_segments` — ANSI-aware, zero-copy splitting of already-styled
  text into terminal rows.

These are not separable. `set_frame_links` and the rest are methods on the
forked `Terminal`, so adopting any of them means adopting the inline model.

## Re-syncing from upstream

Because `src/` is unmodified, a re-sync is a directory replacement:

```bash
git clone --depth 1 https://github.com/xai-org/grok-build.git /tmp/grok-build
rm -rf crates/fleety-inline/src
cp -R /tmp/grok-build/crates/codegen/xai-ratatui-inline/src crates/fleety-inline/src
cp /tmp/grok-build/LICENSE crates/fleety-inline/LICENSE
cargo clippy -p fleety-inline --all-targets -- -D warnings
```

Then update the snapshot row above with the new `SOURCE_REV`, and re-check the
dependency versions and the feature list in `Cargo.toml` against upstream's —
upstream inherits its versions from a workspace, this crate pins them literally,
so they drift silently.

Upstream does not accept external pull requests, so fixes made here cannot be
sent back; keep local changes to `src/` at zero for as long as possible.
