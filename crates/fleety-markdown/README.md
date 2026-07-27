# fleety-markdown

CommonMark rendering to ratatui `Line`s, with `syntect` syntax highlighting for
fenced code blocks, tables, task lists, and hyperlinks.

**This crate is vendored third-party source, not Fleety-authored code.** Treat
`src/` as read-only. See "Re-syncing from upstream" before changing anything in
it. The sibling `fleety-markdown-core` is vendored from the same snapshot and
carries the same rules.

## Provenance

| | |
|---|---|
| Upstream project | [`xai-org/grok-build`](https://github.com/xai-org/grok-build) |
| Upstream paths | `crates/codegen/xai-grok-markdown`, `crates/codegen/xai-grok-markdown-core` |
| Upstream packages | `xai-grok-markdown` 0.1.0, `xai-grok-markdown-core` 0.1.0 |
| Snapshot | `SOURCE_REV` `d02693a856a54f1030695b36b91d276e96b30b23` |
| Vendored on | 2026-07-26 |
| License | Apache-2.0, Copyright 2023-2026 SpaceXAI (see [`LICENSE`](LICENSE)) |

## Changes from upstream

Required by Apache-2.0 §4(b).

- **`src/` and `assets/` are byte-identical to upstream.** No source
  modifications.
- `Cargo.toml` is Fleety-authored: the packages are renamed to
  `fleety-markdown` / `fleety-markdown-core`, workspace dependency inheritance
  is replaced with literal versions, and upstream's `benches/`, `bin/`, `fuzz/`,
  `[lints] workspace`, and the optional `playground` feature are dropped.
- The dependency on the core crate keeps upstream's **extern name** via
  `xai-grok-markdown-core = { package = "fleety-markdown-core", … }`. That
  rename is exactly what lets `src/` stay unmodified — the source still says
  `xai_grok_markdown_core`.

Upstream publishes no `NOTICE` file, so Apache-2.0 §4(d) does not apply.

## What Fleety supplies

`MarkdownStyle::default()` is entirely unstyled — upstream fills it from a theme
layer that is not part of this crate. The chat palette, and the decision to keep
a bare newline as a line break rather than collapsing it the way CommonMark
says to, both live in `crates/fleety-cli/src/markdown.rs`. That adapter is the
place to change how replies look; this crate is not.

The syntax-highlighting theme in `assets/tokyo-night.tmTheme` is upstream's, and
`crates/fleety-cli/src/markdown.rs` includes it by relative path. A re-sync that
moves or renames it breaks the build, which is the intent.

## Re-syncing from upstream

Because `src/` is unmodified, a re-sync is a directory replacement:

```bash
git clone --depth 1 https://github.com/xai-org/grok-build.git /tmp/grok-build
rm -rf crates/fleety-markdown/src crates/fleety-markdown/assets crates/fleety-markdown-core/src
cp -R /tmp/grok-build/crates/codegen/xai-grok-markdown/src crates/fleety-markdown/src
cp -R /tmp/grok-build/crates/codegen/xai-grok-markdown/assets crates/fleety-markdown/assets
cp -R /tmp/grok-build/crates/codegen/xai-grok-markdown-core/src crates/fleety-markdown-core/src
cargo test -p fleety-markdown
```

Then update the snapshot row above with the new `SOURCE_REV`, and re-check the
dependency versions in both `Cargo.toml` files against upstream's workspace
`Cargo.toml` — upstream inherits them, these crates pin them literally, so they
drift silently. Re-check the `[lints.clippy]` allow-list too; it is a record of
the drift and should shrink, never grow.

Upstream does not accept external pull requests, so fixes made here cannot be
sent back; keep local changes to `src/` at zero for as long as possible.
