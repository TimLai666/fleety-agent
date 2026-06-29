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

**Before cutting a release, update the bundled Insyra — both the library and the
skill.** The `fleety-insyra` sidecar ([`sidecars/fleety-insyra`](sidecars/fleety-insyra))
embeds `github.com/HazelnutParadise/insyra`, and `fleety-server` embeds the
upstream Insyra skill (vendored as the `fleety-use-insyra-dsl` builtin) at
[`crates/fleety-server/builtin-skills/fleety-use-insyra-dsl/SKILL.upstream.md`](crates/fleety-server/builtin-skills/fleety-use-insyra-dsl/SKILL.upstream.md).
We want releases to ship the latest of both, kept in lockstep. CI's release
workflow does this automatically (`go get -u …@latest && go mod tidy`, then
`curl` the skill from the Insyra repo at the resolved version, then build +
smoke test). If you ever cut a release outside that workflow, in
`sidecars/fleety-insyra/` run `go get -u github.com/HazelnutParadise/insyra@latest
&& go mod tidy`, refresh `SKILL.upstream.md` from
`raw.githubusercontent.com/HazelnutParadise/insyra/<version>/skills/use-insyra-cli/SKILL.md`,
and commit the updated `go.mod`/`go.sum`/skill. A breaking Insyra change will
surface as a sidecar build/test failure — fix the sidecar, don't pin around it
silently.
