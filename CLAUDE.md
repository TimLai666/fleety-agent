<!-- SPECTRA:START v1.0.2 -->

# Spectra Instructions

This project uses Spectra for Spec-Driven Development(SDD). Specs live in `openspec/specs/`, change proposals in `openspec/changes/`.

## Use `/spectra-*` skills when:

- A discussion needs structure before coding → `/spectra-discuss`
- User wants to plan, propose, or design a change → `/spectra-propose`
- Tasks are ready to implement → `/spectra-apply`
- There's an in-progress change to continue → `/spectra-ingest`
- User asks about specs or how something works → `/spectra-ask`
- Implementation is done → `/spectra-archive`
- Commit only files related to a specific change → `/spectra-commit`

## Workflow

discuss? → propose → apply ⇄ ingest → archive

- `discuss` is optional — skip if requirements are clear
- Requirements change mid-work? Plan mode → `ingest` → resume `apply`

## Parked Changes

Changes can be parked（暫存）— temporarily moved out of `openspec/changes/`. Parked changes won't appear in `spectra list` but can be found with `spectra list --parked`. To restore: `spectra unpark <name>`. The `/spectra-apply` and `/spectra-ingest` skills handle parked changes automatically.

<!-- SPECTRA:END -->

# Fleety — project notes for Claude

Workspace-wide engineering rules live in the code (`#![warn(clippy::unwrap_used,
clippy::expect_used)]`, never-crash errors-as-messages, `agent-core` depends on
no Fleety crate). This file holds things that aren't derivable from the code.

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
