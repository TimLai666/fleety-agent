# Fleety — project notes for Claude

Workspace-wide engineering rules live in the code (`#![warn(clippy::unwrap_used,
clippy::expect_used)]`, never-crash errors-as-messages, `agent-core` depends on
no Fleety crate). This file holds things that aren't derivable from the code.

## Releasing

**Before cutting a release, update the bundled Insyra — both the library and the
skill.** The `fleety-insyra` sidecar ([`sidecars/fleety-insyra`](sidecars/fleety-insyra))
embeds `github.com/HazelnutParadise/insyra`, and `fleety-server` embeds the
upstream `use-insyra-cli` skill at
[`crates/fleety-server/builtin-skills/use-insyra-cli/SKILL.upstream.md`](crates/fleety-server/builtin-skills/use-insyra-cli/SKILL.upstream.md).
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
